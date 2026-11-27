//! The slice of YAML a carapace spec is written in, and nothing else.
//!
//! ```yaml
//! name: mycmd
//! flags:
//!   -v=: flag with value
//!   --optarg?: optional argument
//! completion:
//!   flag:
//!     v: ["$files", "two\twith description"]
//!   positional:
//!     - ["$list(,)", "1", "2"]
//! ```
//!
//! Block mappings and sequences, flow `[…]` and `{…}`, the three scalar quotings, `|` and `>`
//! blocks, and `#` comments. That is every construct the schema can produce and every one the
//! generators emit.
//!
//! # What it refuses, and why refusing is the point
//!
//! Anchors (`&a`), aliases (`*a`), tags (`!!str`), and a second document. A general YAML parser is
//! a large dependency in a binary that measures itself in kilobytes, and a *partial* one that
//! quietly mis-reads what it does not know is worse than either — so anything outside the subset is
//! an error naming the line, not a guess. A spec is read once and cached; the cost of being strict
//! is a message, and the cost of guessing is a completion that inserts the wrong word.

/// A parsed document. The map keeps its order: `flags` is offered in the order it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Scalar(String),
    List(Vec<Node>),
    Map(Vec<(String, Node)>),
}

impl Node {
    pub fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Node::Scalar(text) => Some(text),
            _ => None,
        }
    }

    /// The members of a sequence. A lone scalar counts as a sequence of one, which is how
    /// `aliases: a` and `aliases: [a]` come to mean the same thing.
    pub fn items(&self) -> Vec<&Node> {
        match self {
            Node::List(items) => items.iter().collect(),
            Node::Scalar(text) if text.is_empty() => Vec::new(),
            other => vec![other],
        }
    }

    pub fn pairs(&self) -> Vec<(&str, &Node)> {
        match self {
            Node::Map(pairs) => pairs.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            _ => Vec::new(),
        }
    }

    pub fn truthy(&self) -> bool {
        matches!(self.text(), Some("true" | "yes" | "on"))
    }
}

pub fn parse(source: &str) -> Result<Node, String> {
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    // A leading `---` is the one document marker a spec ever carries. A second one means a second
    // document, which this does not read.
    if lines.first().is_some_and(|line| line.trim() == "---") {
        lines.remove(0);
    }
    let mut parser = Parser {
        lines,
        at: 0,
        depth: 0,
    };
    let node = parser.value(0)?;
    parser.skip_blank();
    match parser.at < parser.lines.len() {
        true => Err(parser.problem("more than one document, or text outside the top level")),
        false => Ok(node),
    }
}

/// How deep a document may nest before it is refused.
///
/// Far past anything either converter emits — the 1,168 shipped specs reach four — and far short
/// of a stack.
const MAX_DEPTH: usize = 32;

struct Parser {
    lines: Vec<String>,
    at: usize,
    depth: usize,
}

impl Parser {
    fn problem(&self, what: &str) -> String {
        format!("line {}: {what}", self.at + 1)
    }

    /// Move to the next line that holds something.
    fn skip_blank(&mut self) {
        while let Some(line) = self.lines.get(self.at) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                self.at += 1;
                continue;
            }
            return;
        }
    }

    fn indent(&self) -> usize {
        self.lines[self.at].len() - self.lines[self.at].trim_start().len()
    }

    /// The value beginning at or below `indent`.
    fn value(&mut self, indent: usize) -> Result<Node, String> {
        // **The one place the nesting is counted.** `value` → `map`/`list` → `nested` → `value` is
        // a cycle with nothing bounding it, and a file nested past the stack does not fail to parse
        // — it *aborts the process*, on the keystroke that first named that command. This module
        // promises a refusal naming the line, and an abort is not one.
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(self.problem("nested too deeply"));
        }
        let read = self.value_here(indent);
        self.depth -= 1;
        read
    }

    fn value_here(&mut self, indent: usize) -> Result<Node, String> {
        self.skip_blank();
        if self.at >= self.lines.len() || self.indent() < indent {
            return Ok(Node::Scalar(String::new()));
        }
        match self.lines[self.at].trim_start().starts_with("- ")
            || self.lines[self.at].trim_end() == "-"
        {
            true => self.list(self.indent()),
            false => self.map(self.indent()),
        }
    }

    fn list(&mut self, indent: usize) -> Result<Node, String> {
        let mut items = Vec::new();
        loop {
            self.skip_blank();
            if self.at >= self.lines.len() || self.indent() != indent {
                break;
            }
            let line = self.lines[self.at].clone();
            let trimmed = line.trim_start();
            if !trimmed.starts_with("- ") && trimmed.trim_end() != "-" {
                break;
            }
            let rest = trimmed[1..].trim_start();
            if rest.is_empty() {
                // `-` alone: the item is the block below it.
                self.at += 1;
                items.push(self.value(indent + 1)?);
                continue;
            }
            // `- name: foo` is a mapping whose first key shares the dash's line. Blanking the dash
            // turns it into an ordinary line at the column the key really sits in, and the mapping
            // parser needs to know nothing about sequences.
            let column = line.len() - rest.len();
            let blanked = format!("{}{rest}", " ".repeat(column));
            self.lines[self.at] = blanked;
            items.push(match is_mapping(rest) {
                true => self.map(column)?,
                false => {
                    self.at += 1;
                    scalar(rest)?
                }
            });
        }
        Ok(Node::List(items))
    }

    fn map(&mut self, indent: usize) -> Result<Node, String> {
        let mut pairs: Vec<(String, Node)> = Vec::new();
        loop {
            self.skip_blank();
            if self.at >= self.lines.len() || self.indent() != indent {
                break;
            }
            let line = self.lines[self.at].clone();
            let trimmed = line.trim_start();
            if trimmed.starts_with("- ") {
                break;
            }
            if trimmed.trim_end() == "---" {
                return Err(self.problem("more than one document"));
            }
            let Some(at) = key_end(trimmed) else {
                return Err(self.problem("expected `key: value`"));
            };
            let key = unquote(trimmed[..at].trim())?;
            let rest = trimmed[at + 1..].trim();
            self.at += 1;
            let value = match rest {
                "" => self.nested(indent)?,
                "|" | "|-" | "|+" | ">" | ">-" | ">+" => self.block(indent, rest)?,
                text => scalar(strip_comment(text))?,
            };
            pairs.push((key, value));
        }
        Ok(Node::Map(pairs))
    }

    /// The value under a key that had none on its own line.
    ///
    /// **A sequence may sit at the key's own indentation**, which is how every generated spec
    /// writes `commands:`. Anything else has to be deeper, or it is the next key.
    fn nested(&mut self, indent: usize) -> Result<Node, String> {
        self.skip_blank();
        if self.at < self.lines.len() && self.indent() == indent {
            let trimmed = self.lines[self.at].trim_start();
            if trimmed.starts_with("- ") || trimmed.trim_end() == "-" {
                return self.list(indent);
            }
        }
        self.value(indent + 1)
    }

    /// A `|` or `>` block|` or `>` block: every line indented past the key, with that indent removed.
    fn block(&mut self, indent: usize, style: &str) -> Result<Node, String> {
        let mut kept: Vec<String> = Vec::new();
        let mut inner: Option<usize> = None;
        while let Some(line) = self.lines.get(self.at) {
            if line.trim().is_empty() {
                kept.push(String::new());
                self.at += 1;
                continue;
            }
            let here = line.len() - line.trim_start().len();
            if here <= indent {
                break;
            }
            let inner = *inner.get_or_insert(here);
            kept.push(line.get(inner..).unwrap_or("").to_string());
            self.at += 1;
        }
        while kept.last().is_some_and(String::is_empty) {
            kept.pop();
        }
        let joined = match style.starts_with('>') {
            true => kept.join(" "),
            false => kept.join("\n"),
        };
        Ok(Node::Scalar(match style.contains('-') {
            true => joined,
            false => format!("{joined}\n"),
        }))
    }
}

/// Whether the text after a `-` starts a mapping rather than being a plain value.
fn is_mapping(text: &str) -> bool {
    !text.starts_with(['[', '{']) && key_end(text).is_some()
}

/// The offset of the `:` that ends a key, or `None` when the line has no key.
///
/// A `:` inside quotes or inside a flow collection is not it, and a `:` with no space after it is
/// part of the value — `http://x` is a URL, not a key called `http`.
fn key_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut depth = 0usize;
    for (at, ch) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some(b'"'), b'\\') => escaped = true,
            (Some(open), ch) if *ch == open => quote = None,
            (Some(_), _) => {}
            (None, b'\'' | b'"') => quote = Some(*ch),
            (None, b'[' | b'{') => depth += 1,
            (None, b']' | b'}') => depth = depth.saturating_sub(1),
            (None, b':') if depth == 0 && (at + 1 == bytes.len() || bytes[at + 1] == b' ') => {
                return Some(at);
            }
            _ => {}
        }
    }
    None
}

/// Everything before an unquoted `#` that follows whitespace.
fn strip_comment(text: &str) -> &str {
    let bytes = text.as_bytes();
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    for (at, ch) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, ch) {
            (Some(b'"'), b'\\') => escaped = true,
            (Some(open), ch) if *ch == open => quote = None,
            (Some(_), _) => {}
            (None, b'\'' | b'"') => quote = Some(*ch),
            (None, b'#') if at > 0 && bytes[at - 1] == b' ' => return text[..at].trim_end(),
            _ => {}
        }
    }
    text
}

/// One value written on a single line: a flow collection, a quoted string, or a plain word.
pub fn scalar(text: &str) -> Result<Node, String> {
    scalar_at(text, 0)
}

/// The same, counting how deep the flow collections have gone.
///
/// **A second unbounded recursion, and the cheaper one to reach**: `[[[[…` needs no indentation and
/// no newlines, so a single 13KB line is enough to take the stack — and with it the shell, on a
/// keystroke. Same limit, same refusal.
fn scalar_at(text: &str, depth: usize) -> Result<Node, String> {
    let text = text.trim();
    if depth > MAX_DEPTH {
        return Err("nested too deeply".to_string());
    }
    if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        return Ok(Node::List(
            split_flow(inner)?
                .iter()
                .map(|part| scalar_at(part, depth + 1))
                .collect::<Result<_, _>>()?,
        ));
    }
    if let Some(inner) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        let mut pairs = Vec::new();
        for part in split_flow(inner)? {
            let Some(at) = key_end(&part).or_else(|| part.find(':')) else {
                return Err(format!("expected `key: value` in `{{{inner}}}`"));
            };
            pairs.push((
                unquote(part[..at].trim())?,
                scalar_at(&part[at + 1..], depth + 1)?,
            ));
        }
        return Ok(Node::Map(pairs));
    }
    Ok(Node::Scalar(unquote(text)?))
}

/// The members of a flow collection, split on the commas that are not inside one.
fn split_flow(inner: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut depth = 0usize;
    for ch in inner.chars() {
        // **A `\"` does not end a double-quoted scalar**, and a scanner that thinks it does splits
        // the value at the next comma. 274 of the shipped specs carry one — every description that
        // quotes something — so this is the difference between a value list and three broken
        // fragments of one. Single quotes have no escapes in YAML, which is why this is `"` only.
        if escaped {
            escaped = false;
            current.push(ch);
            continue;
        }
        match (quote, ch) {
            (Some('"'), '\\') => {
                escaped = true;
                current.push(ch);
            }
            (Some(open), ch) if ch == open => {
                quote = None;
                current.push(ch);
            }
            (Some(_), _) => current.push(ch),
            (None, '\'' | '"') => {
                quote = Some(ch);
                current.push(ch);
            }
            (None, '[' | '{') => {
                depth += 1;
                current.push(ch);
            }
            (None, ']' | '}') => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            (None, ',') if depth == 0 => {
                parts.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    if quote.is_some() {
        return Err(format!("unterminated quote in `{inner}`"));
    }
    if !current.trim().is_empty() || !parts.is_empty() {
        parts.push(current);
    }
    Ok(parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect())
}

/// A scalar with its quotes resolved, and the constructs this does not read refused.
fn unquote(text: &str) -> Result<String, String> {
    let text = text.trim();
    if let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        // Single quotes are literal, and `''` is one apostrophe.
        return Ok(inner.replace("''", "'"));
    }
    if let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        return Ok(escaped(inner));
    }
    if text.starts_with(['&', '*', '!']) {
        return Err(format!("`{text}`: anchors, aliases and tags are not read"));
    }
    Ok(text.to_string())
}

/// The escapes a double-quoted scalar can carry. `\t` matters more than the rest: it is what
/// separates a carapace value from its description.
fn escaped(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
#[path = "yaml/tests.rs"]
mod tests;
