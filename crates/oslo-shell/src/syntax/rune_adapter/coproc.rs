//! `coproc [NAME] command`, which rune reads as an ordinary command whose name is `coproc`.
//!
//! Before lowering, every such keyword (and its `NAME`, when a compound command follows it) is
//! overwritten with blanks of the same length, and the script is parsed again. The command left
//! behind then parses as what it is — `{ …; }` included — at unchanged offsets, and the offset its
//! node starts at is recorded here, so lowering can wrap that one command in
//! [`oslo_ast::CompoundCommand::Coproc`].

use oslo_base::ast as oslo_ast;
use oslo_base::error::{Result, ShellError};
use rune::ast::SimpleCommand;
use rune::{SyntaxKind, Tree};
use std::cell::RefCell;

const KEYWORD: &str = "coproc";
/// The array a coprocess publishes its descriptors in when the script names none.
const DEFAULT_NAME: &str = "COPROC";

/// A coprocess whose body starts at `at` and whose descriptors go into `name`.
pub(super) struct Site {
    at: u32,
    name: String,
}

thread_local! {
    static SITES: RefCell<Vec<Site>> = const { RefCell::new(Vec::new()) };
}

/// The script with every `coproc` header blanked out, and the coprocesses found in it.
///
/// `None` when the script holds no `coproc` keyword at all, which is nearly always.
pub(super) fn prepare(script: &str) -> Option<(String, Vec<Site>)> {
    if !script.contains(KEYWORD) {
        return None;
    }
    let mut text = script.to_string();
    let mut sites = Vec::new();
    loop {
        let parsed = rune::parse(&text);
        let Some(start) = keyword_at(parsed.tree(), parsed.tree().root()) else {
            break;
        };
        let (name, body) = header(&text, start as usize);
        blank(&mut text, start as usize, body);
        sites.push(Site {
            at: body as u32,
            name,
        });
    }
    (!sites.is_empty()).then_some((text, sites))
}

/// Run `lower` with `sites` visible to [`wrap`], and refuse a `coproc` that had no command.
pub(super) fn with_sites<T>(sites: Vec<Site>, lower: impl FnOnce() -> Result<T>) -> Result<T> {
    let outer = SITES.with(|cell| cell.replace(sites));
    let result = lower();
    let left = SITES.with(|cell| cell.replace(outer));
    match result {
        Ok(_) if !left.is_empty() => Err(ShellError::SyntaxError(
            "coproc: a command to run was expected".to_string(),
        )),
        other => other,
    }
}

/// The coprocess name recorded for a command starting at `at`, taken so it wraps only once.
pub(super) fn take_site(at: u32) -> Option<String> {
    SITES.with(|cell| {
        let mut sites = cell.borrow_mut();
        let index = sites.iter().position(|site| site.at == at)?;
        Some(sites.remove(index).name)
    })
}

/// `command` as the body of a coprocess publishing into `name`.
pub(super) fn wrap(name: String, command: oslo_ast::Command) -> oslo_ast::Command {
    oslo_ast::Command::Compound {
        kind: oslo_ast::CompoundCommand::Coproc {
            name,
            body: Box::new(command),
        },
        redirections: Vec::new(),
    }
}

/// Where the first `coproc` in command position starts, outside any closed substitution.
fn keyword_at(tree: &Tree, node: &rune::Node) -> Option<u32> {
    let substitution = matches!(
        node.kind(),
        SyntaxKind::CommandSubstitution | SyntaxKind::ProcessSubstitution
    );
    if substitution && tree.source().slice(node.span()).ends_with(')') {
        return None;
    }
    if let Some(command) = SimpleCommand::cast(node)
        && let Some(word) = command.name()
        && word.span().start == node.span().start
        && tree.source().slice(word.span()) == KEYWORD
    {
        return Some(word.span().start);
    }
    node.nodes().find_map(|child| keyword_at(tree, child))
}

/// The coprocess name, and the offset its command starts at, for a keyword at `start`.
///
/// A word after `coproc` is the name only when a compound command follows it, as in bash:
/// `coproc NAME { …; }` names the array, `coproc cat file` runs `cat` and uses `COPROC`.
fn header(text: &str, start: usize) -> (String, usize) {
    let after = skip_blanks(text, start + KEYWORD.len());
    let ident = text[after..]
        .bytes()
        .enumerate()
        .take_while(|(i, b)| {
            b.is_ascii_alphabetic() || *b == b'_' || (*i > 0 && b.is_ascii_digit())
        })
        .count();
    if ident > 0 {
        let body = skip_blanks(text, after + ident);
        if opens_compound(&text[body..]) {
            return (text[after..after + ident].to_string(), body);
        }
    }
    (DEFAULT_NAME.to_string(), after)
}

fn skip_blanks(text: &str, mut at: usize) -> usize {
    while text[at..].starts_with([' ', '\t']) {
        at += 1;
    }
    at
}

/// Whether `rest` begins with a compound command.
fn opens_compound(rest: &str) -> bool {
    if rest.starts_with('(') {
        return true;
    }
    ["{", "[[", "if", "while", "until", "for", "case", "select"]
        .iter()
        .any(|word| {
            rest.strip_prefix(word).is_some_and(|tail| {
                tail.is_empty()
                    || tail.starts_with([' ', '\t', '\n', ';'])
                    || *word == "for" && tail.starts_with('(')
            })
        })
}

/// Overwrite `text[from..to]` with spaces, keeping every byte offset where it was.
///
/// The range is the keyword, blanks and an ASCII name, so one space per byte is exact.
fn blank(text: &mut String, from: usize, to: usize) {
    text.replace_range(from..to, &" ".repeat(to - from));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_before_a_compound_command_is_the_name() {
        let text = "coproc NAME { cat; }";
        assert_eq!(header(text, 0), ("NAME".to_string(), 12));
    }

    #[test]
    fn a_simple_command_uses_the_default_name() {
        let text = "coproc cat file";
        assert_eq!(header(text, 0), ("COPROC".to_string(), 7));
        assert_eq!(header("coproc { cat; }", 0), ("COPROC".to_string(), 7));
    }

    #[test]
    fn the_header_is_blanked_at_the_same_length() {
        let (text, sites) = prepare("coproc N { cat; }\necho x").expect("a coproc");
        assert_eq!(text, "         { cat; }\necho x");
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].at, 9);
    }
}
