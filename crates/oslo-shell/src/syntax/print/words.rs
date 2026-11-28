//! A word as bash's printer shows it: as written, except that `$(…)` and `<(…)` bodies are
//! printed again in bash's layout, `$'…'` is decoded into a single-quoted string, `$"…"` loses its
//! `$`, and a backslash-newline is gone.

use super::build::Builder;
use super::emit;
use rune::ast::{CommandList, CommandSubstitution, ProcessSubstitution};
use rune::{Element, Node, SyntaxKind};

/// The printed text of a word, or of any node whose text is printed as a word is.
pub(super) fn word(builder: &Builder, node: &Node) -> String {
    let mut out = String::new();
    let mut quoted = false;
    pieces(builder, node, &mut out, &mut quoted);
    out
}

fn pieces(builder: &Builder, node: &Node, out: &mut String, quoted: &mut bool) {
    let source = builder.tree.source();
    for child in node.children() {
        match child {
            Element::Token(token) => {
                let text = token.text(source);
                match token.kind() {
                    SyntaxKind::LineContinuation => {}
                    SyntaxKind::AnsiCQuoted if !*quoted => {
                        let body = text.strip_prefix("$'").unwrap_or(text);
                        let body = body.strip_suffix('\'').unwrap_or(body);
                        out.push_str(&single_quote(&crate::lexer::decode_ansi_c(body)));
                    }
                    SyntaxKind::DoubleQuote => {
                        *quoted = !*quoted;
                        out.push_str(text.strip_prefix('$').unwrap_or(text));
                    }
                    _ => out.push_str(text),
                }
            }
            Element::Node(inner) => match inner.kind() {
                SyntaxKind::CommandSubstitution => {
                    let substitution = CommandSubstitution::cast(inner);
                    match substitution {
                        Some(sub) if !sub.is_backticked() => {
                            out.push_str("$(");
                            out.push_str(&body_text(builder, sub.body()));
                            out.push(')');
                        }
                        _ => out.push_str(builder.text(inner)),
                    }
                }
                SyntaxKind::ArithmeticExpansion => {
                    let text = builder.text(inner);
                    let body = text.strip_prefix("$((").unwrap_or(text);
                    let body = body.strip_suffix("))").unwrap_or(body);
                    out.push_str("$((");
                    out.push_str(&substitutions_in(body));
                    out.push_str("))");
                }
                SyntaxKind::ProcessSubstitution => {
                    let Some(sub) = ProcessSubstitution::cast(inner) else {
                        continue;
                    };
                    out.push(if sub.reads_from_command() { '<' } else { '>' });
                    out.push('(');
                    out.push_str(&body_text(builder, sub.body()));
                    out.push(')');
                }
                _ => pieces(builder, inner, out, quoted),
            },
        }
    }
}

/// A substitution's body printed as bash's `print_comsub` prints it, with a space before a
/// leading `(` so `$( (…))` cannot read as arithmetic.
fn body_text(builder: &Builder, body: Option<CommandList<'_>>) -> String {
    let inner = builder.substitution();
    let Some(cmd) = inner.list(body) else {
        return String::new();
    };
    let text = emit::command_string(&cmd);
    match text.starts_with('(') {
        true => format!(" {text}"),
        false => text,
    }
}

/// Raw text with every `$(…)` in it printed again, for the arithmetic text rune keeps unparsed.
fn substitutions_in(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("$(") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        if after.starts_with('(') {
            out.push_str("$(");
            rest = after;
            continue;
        }
        let mut depth = 1usize;
        let close = after.char_indices().find_map(|(index, ch)| {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(index)
        });
        let Some(close) = close else {
            out.push_str(&rest[at..]);
            return out;
        };
        let parsed = rune::parse(&after[..close]);
        let script = rune::Script::of(parsed.tree());
        let inner = Builder::new(parsed.tree(), true);
        let printed = inner
            .list(script.commands())
            .map(|cmd| emit::command_string(&cmd));
        out.push_str("$(");
        out.push_str(&printed.unwrap_or_else(|| after[..close].to_string()));
        out.push(')');
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// An assignment word: `name=value`, `name+=value` or `name=(a b)`.
pub(super) fn assignment(builder: &Builder, node: &Node) -> String {
    let mut out = String::new();
    for child in node.children() {
        match child {
            Element::Token(token) if token.kind() != SyntaxKind::Whitespace => {
                out.push_str(token.text(builder.tree.source()));
            }
            Element::Token(_) => {}
            Element::Node(inner) if inner.kind() == SyntaxKind::ArrayValue => {
                let elements: Vec<String> = inner
                    .nodes()
                    .filter(|element| element.kind() == SyntaxKind::Word)
                    .map(|element| word(builder, element))
                    .collect();
                out.push('(');
                out.push_str(&elements.join(" "));
                out.push(')');
            }
            Element::Node(inner) => out.push_str(&word(builder, inner)),
        }
    }
    out
}

/// bash's `sh_single_quote`.
pub(super) fn single_quote(text: &str) -> String {
    if text == "'" {
        return "\\'".to_string();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}
