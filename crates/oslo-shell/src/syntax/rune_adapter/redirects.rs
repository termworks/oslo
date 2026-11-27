//! Redirection conversion: files, fd duplication, here-documents and here-strings.

use super::commands::{holds_a_process_substitution, text_of, word_of};
use crate::lexer::{TildeMode, parse_heredoc_body};
use crate::syntax::lower::{convert_braced_words_from_str, single_word_with_tilde};
use oslo_base::ast as oslo_ast;
use oslo_base::error::Result;
use rune::{Node, SyntaxKind, Tree};

/// The redirections hanging off a compound command, which sit beside it rather than inside it.
pub(super) fn redirects_of(node: &Node) -> Vec<&Node> {
    node.nodes()
        .filter(|child| child.kind() == SyntaxKind::Redirect)
        .collect()
}

pub(super) fn convert_redirects(
    tree: &Tree,
    nodes: Vec<&Node>,
) -> Result<Vec<oslo_ast::Redirection>> {
    let mut out = Vec::new();
    for node in nodes {
        out.extend(convert_redirect(tree, node)?);
    }
    Ok(out)
}

/// Convert one redirection.
///
/// Returns a `Vec` because `&>file` expands to two oslo redirections — there is no single node
/// meaning "stdout and stderr".
pub(super) fn convert_redirect(tree: &Tree, node: &Node) -> Result<Vec<oslo_ast::Redirection>> {
    let operator = node
        .tokens()
        .find(|token| token.kind().is_redirect_operator())
        .map(|token| token.kind());
    // A leading run of digits names the descriptor: `2>&1`.
    let fd = node
        .tokens()
        .find(|token| token.kind() == SyntaxKind::Text)
        .and_then(|token| token.text(tree.source()).parse::<i32>().ok());
    let target_text = node
        .node(SyntaxKind::Word)
        .map(|word| text_of(tree, word))
        .unwrap_or_default();
    // Built through `word_of` rather than from the text: `cmd < <(gen)` redirects from a process
    // substitution, whose value is the name of a pipe and not anything the word lexer can read.
    // A target is brace-expanded, and more than one word from it is an ambiguous redirect.
    let mut ambiguous = false;
    let target = match node.node(SyntaxKind::Word) {
        Some(word) if target_text.contains('{') && !holds_a_process_substitution(word) => {
            let mut words = convert_braced_words_from_str(target_text)?;
            match words.len() {
                1 => words.remove(0),
                0 => word_of(tree, word)?,
                _ => {
                    ambiguous = true;
                    oslo_ast::Word::from_literal(target_text)
                }
            }
        }
        Some(word) => word_of(tree, word)?,
        None => oslo_ast::Word::from_literal(""),
    };

    let kind = match operator {
        Some(SyntaxKind::Less) => oslo_ast::RedirectKind::Input,
        Some(SyntaxKind::Great) => oslo_ast::RedirectKind::Output,
        Some(SyntaxKind::GreatGreat) => oslo_ast::RedirectKind::Append,
        Some(SyntaxKind::LessGreat) => oslo_ast::RedirectKind::ReadWrite,
        Some(SyntaxKind::LessAmp) => oslo_ast::RedirectKind::DupInput,
        Some(SyntaxKind::GreatAmp) => oslo_ast::RedirectKind::DupOutput,
        Some(SyntaxKind::GreatPipe) => oslo_ast::RedirectKind::Clobber,

        // `<<< word` is a here-document whose body is one ordinary word, expansions and all.
        // Re-lexing it as a word is what keeps the quoting: expansion does the quote removal that
        // a textual strip used to do wrongly (`<<< a"b"c` lost nothing, `<<< "a"x"b"` lost the
        // wrong quotes).
        // Only a leading tilde expands there: `<<< a=~` keeps its tilde.
        Some(SyntaxKind::LessLessLess) => {
            let body = single_word_with_tilde(target_text, TildeMode::Leading)?;
            return Ok(vec![oslo_ast::Redirection {
                fd,
                kind: oslo_ast::RedirectKind::Heredoc,
                target: oslo_ast::Word::from_literal(""),
                heredoc_content: Some(body),
                here_string: true,
                fd_var: None,
                ambiguous: false,
            }]);
        }

        Some(SyntaxKind::LessLess | SyntaxKind::LessLessDash) => {
            return convert_heredoc(tree, node, fd, target_text);
        }

        // `&>file` / `&>>file`: send stdout to the file, then point stderr at stdout.
        Some(kind @ (SyntaxKind::AmpGreat | SyntaxKind::AmpGreatGreat)) => {
            let append = kind == SyntaxKind::AmpGreatGreat;
            return Ok(vec![
                oslo_ast::Redirection {
                    fd: Some(1),
                    kind: match append {
                        true => oslo_ast::RedirectKind::Append,
                        false => oslo_ast::RedirectKind::Output,
                    },
                    target: target.clone(),
                    heredoc_content: None,
                    here_string: false,
                    fd_var: None,
                    ambiguous,
                },
                oslo_ast::Redirection {
                    fd: Some(2),
                    kind: oslo_ast::RedirectKind::DupOutput,
                    target: oslo_ast::Word::from_literal("1"),
                    heredoc_content: None,
                    here_string: false,
                    fd_var: None,
                    ambiguous: false,
                },
            ]);
        }
        _ => oslo_ast::RedirectKind::Output,
    };

    Ok(vec![oslo_ast::Redirection {
        fd,
        kind,
        target,
        heredoc_content: None,
        here_string: false,
        fd_var: None,
        ambiguous,
    }])
}

/// `<<EOF` and `<<-EOF`, whose body sits further along the token stream than the operator does.
fn convert_heredoc(
    tree: &Tree,
    node: &Node,
    fd: Option<i32>,
    delimiter: &str,
) -> Result<Vec<oslo_ast::Redirection>> {
    let strip_tabs = node.token(SyntaxKind::LessLessDash).is_some();
    let mut content = heredoc_body(tree, node);

    // `<<-` strips leading tabs from every line, including the delimiter line. This runs on the
    // raw text, before lexing: a tab is stripped because of where it sits in the line, which is a
    // fact about the source and not about the parts it lexes into.
    if strip_tabs {
        content = content
            .lines()
            .map(|line| line.trim_start_matches('\t'))
            .collect::<Vec<_>>()
            .join("\n");
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
    }

    // A quoted delimiter (`<<'EOF'`) makes the whole body literal. The quotes are on the
    // delimiter, which is why the delimiter's *source text* is what decides.
    let expands = !delimiter.contains(['\'', '"', '\\']);
    let body = match expands {
        true => parse_heredoc_body(&content)?,
        false => oslo_ast::Word::from_literal(&content),
    };

    Ok(vec![oslo_ast::Redirection {
        fd,
        kind: match strip_tabs {
            true => oslo_ast::RedirectKind::HeredocStrip,
            false => oslo_ast::RedirectKind::Heredoc,
        },
        target: oslo_ast::Word::from_literal(delimiter.trim_matches(['\'', '"'])),
        heredoc_content: Some(body),
        here_string: false,
        fd_var: None,
        ambiguous: false,
    }])
}

/// The `name` of a `{name}` word written touching the redirection after it.
///
/// `name` is an identifier, optionally with a subscript; anything else is an ordinary word.
pub(super) fn fd_variable(tree: &Tree, word: &Node, next: Option<&Node>) -> Option<String> {
    let next = next.filter(|next| next.kind() == SyntaxKind::Redirect)?;
    if next.span().start != word.span().end || next.token(SyntaxKind::Text).is_some() {
        return None;
    }
    let inner = text_of(tree, word).strip_prefix('{')?.strip_suffix('}')?;
    let name = inner
        .split_once('[')
        .map_or(inner, |(name, rest)| match rest.ends_with(']') {
            true => name,
            false => "",
        });
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then(|| inner.to_string())
}

/// The body belonging to this here-document.
///
/// It is not inside the redirection: `cat <<EOF` is followed by the rest of its line and only then
/// by the body, so the two are nowhere near each other in the tree. They are paired by counting —
/// the nth `<<` on a line takes the nth body after it, which is the order the shell reads them in.
fn heredoc_body(tree: &Tree, node: &Node) -> String {
    let mut bodies = Vec::new();
    collect_bodies(tree.root(), &mut bodies);
    let mut openers = Vec::new();
    collect_openers(tree.root(), &mut openers);

    let index = openers.iter().position(|opener| *opener == node.span());
    match index.and_then(|index| bodies.get(index)) {
        Some(span) => tree.source().slice(*span).to_string(),
        None => String::new(),
    }
}

fn collect_openers(node: &Node, out: &mut Vec<rune::Span>) {
    for child in node.nodes() {
        if child.kind() == SyntaxKind::Redirect
            && child.tokens().any(|token| {
                matches!(
                    token.kind(),
                    SyntaxKind::LessLess | SyntaxKind::LessLessDash
                )
            })
        {
            out.push(child.span());
        }
        collect_openers(child, out);
    }
}

fn collect_bodies(node: &Node, out: &mut Vec<rune::Span>) {
    let mut pending: Option<rune::Span> = None;
    for child in node.children() {
        match child {
            rune::Element::Token(token) if token.kind() == SyntaxKind::HeredocText => {
                pending = Some(token.span());
            }
            rune::Element::Token(token) if token.kind() == SyntaxKind::HeredocEnd => {
                out.push(
                    pending
                        .take()
                        .unwrap_or(rune::Span::empty(token.span().start)),
                );
            }
            rune::Element::Node(inner) => collect_bodies(inner, out),
            rune::Element::Token(_) => {}
        }
    }
    // A body that ran to the end of the file has no terminator to close it.
    if let Some(span) = pending {
        out.push(span);
    }
}
