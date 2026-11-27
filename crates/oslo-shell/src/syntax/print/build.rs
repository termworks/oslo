//! rune's tree, turned into the command tree bash's printer walks.
//!
//! The shapes follow bash's parser: a list is a left-leaning chain of connections, `elif` is an
//! `if` in the `else` branch, `|&` is `2>&1 |`, `for x` iterates `"$@"`, and a bare word in
//! `[[ ]]` is `-n word`.

use super::words;
use super::{Clause, Cmd, Connector, Kind, Redir};
use rune::ast::{Command, CommandList, ListItem};
use rune::{Element, Node, SyntaxKind, Tree};
use std::collections::HashMap;
use std::rc::Rc;

pub(super) struct Builder<'t> {
    pub(super) tree: &'t Tree,
    /// Inside `$(…)`, where a newline between commands is kept as a newline.
    comsub: bool,
    /// Each here-document's body, by where its `<<` starts.
    heredocs: Rc<HashMap<u32, String>>,
}

impl<'t> Builder<'t> {
    pub(super) fn new(tree: &'t Tree, comsub: bool) -> Self {
        Builder {
            tree,
            comsub,
            heredocs: Rc::new(heredoc_bodies(tree)),
        }
    }

    /// A builder for a command substitution in the same tree.
    pub(super) fn substitution(&self) -> Self {
        Builder {
            tree: self.tree,
            comsub: true,
            heredocs: Rc::clone(&self.heredocs),
        }
    }

    pub(super) fn text(&self, node: &Node) -> &'t str {
        self.tree.source().slice(node.span())
    }

    pub(super) fn list(&self, list: Option<CommandList<'_>>) -> Option<Box<Cmd>> {
        let mut acc: Option<Cmd> = None;
        let mut pending = Connector::Semi;
        let mut trailing_amp = false;
        for item in list?.items() {
            let cmd = self.item(item);
            acc = Some(match acc {
                None => cmd,
                Some(prev) => connect(prev, pending, Some(cmd)),
            });
            let terminator = item.terminator();
            trailing_amp = terminator == Some(SyntaxKind::Amp);
            pending = match terminator {
                Some(SyntaxKind::Amp) => Connector::Amp,
                Some(SyntaxKind::Newline) if self.comsub => Connector::Newline,
                _ => Connector::Semi,
            };
        }
        if trailing_amp {
            acc = acc.map(|cmd| connect(cmd, Connector::Amp, None));
        }
        acc.map(Box::new)
    }

    fn item(&self, item: ListItem<'_>) -> Cmd {
        if let Some(and_or) = item.and_or() {
            return self.and_or(and_or.syntax());
        }
        if let Some(pipeline) = item.pipeline() {
            return self.pipeline(pipeline.syntax());
        }
        item.command()
            .map(|command| self.command(command))
            .unwrap_or_default()
    }

    fn and_or(&self, node: &Node) -> Cmd {
        let mut acc: Option<Cmd> = None;
        let mut pending = Connector::And;
        for child in node.children() {
            match child {
                Element::Token(token) => match token.kind() {
                    SyntaxKind::AndAnd => pending = Connector::And,
                    SyntaxKind::PipePipe => pending = Connector::Or,
                    _ => {}
                },
                Element::Node(inner) => {
                    let cmd = match inner.kind() {
                        SyntaxKind::Pipeline => self.pipeline(inner),
                        _ => match Command::cast(inner) {
                            Some(command) => self.command(command),
                            None => continue,
                        },
                    };
                    acc = Some(match acc {
                        None => cmd,
                        Some(prev) => connect(prev, pending, Some(cmd)),
                    });
                }
            }
        }
        acc.unwrap_or_default()
    }

    fn pipeline(&self, node: &Node) -> Cmd {
        let mut acc: Option<Cmd> = None;
        let mut with_stderr = false;
        let mut invert = false;
        let mut timed = false;
        for child in node.children() {
            match child {
                Element::Token(token) => match token.kind() {
                    SyntaxKind::Bang => invert = !invert,
                    SyntaxKind::Time => timed = true,
                    SyntaxKind::Pipe => with_stderr = false,
                    SyntaxKind::PipeAmp => with_stderr = true,
                    _ => {}
                },
                Element::Node(inner) => {
                    let Some(command) = Command::cast(inner) else {
                        continue;
                    };
                    let cmd = self.command(command);
                    acc = Some(match acc {
                        None => cmd,
                        Some(mut prev) => {
                            if with_stderr {
                                prev.redirects.push(Redir::Plain("2>&1".to_string()));
                            }
                            connect(prev, Connector::Pipe, Some(cmd))
                        }
                    });
                }
            }
        }
        let mut cmd = acc.unwrap_or_default();
        if timed {
            cmd.time = Some(match take_posix_flag(&mut cmd) {
                true => "time -p ",
                false => "time ",
            });
        }
        cmd.invert = invert;
        cmd
    }

    pub(super) fn command(&self, command: Command<'_>) -> Cmd {
        let node = command.syntax();
        let kind = match command {
            Command::Simple(_) => return self.simple(node),
            Command::If(command) => {
                let mut otherwise = self.list(command.else_branch());
                let elifs: Vec<_> = command.elif_clauses().collect();
                for elif in elifs.into_iter().rev() {
                    otherwise = Some(Box::new(Cmd {
                        kind: Kind::If {
                            test: self.list(elif.condition()),
                            then: self.list(elif.then_branch()),
                            otherwise,
                        },
                        ..Cmd::default()
                    }));
                }
                Kind::If {
                    test: self.list(command.condition()),
                    then: self.list(command.then_branch()),
                    otherwise,
                }
            }
            Command::Loop(command) => Kind::Loop {
                keyword: if command.is_until() { "until" } else { "while" },
                test: self.list(command.condition()),
                body: self.list(command.body()),
            },
            Command::For(command) if command.kind() == SyntaxKind::ArithForCommand => {
                Kind::ArithFor {
                    sections: arith_sections(self.text(node)),
                    body: self.list(command.body()),
                }
            }
            Command::For(command) => {
                let has_in = node.tokens().any(|token| token.kind() == SyntaxKind::In);
                let words = match has_in {
                    true => command
                        .items()
                        .map(|word| words::word(self, word.syntax()))
                        .collect(),
                    false => vec!["\"$@\"".to_string()],
                };
                Kind::For {
                    keyword: match command.kind() {
                        SyntaxKind::SelectCommand => "select",
                        _ => "for",
                    },
                    name: command
                        .variable()
                        .map(|word| self.text(word.syntax()).to_string())
                        .unwrap_or_default(),
                    words,
                    body: self.list(command.body()),
                }
            }
            Command::Case(command) => Kind::Case {
                word: command
                    .word()
                    .map(|word| words::word(self, word.syntax()))
                    .unwrap_or_default(),
                clauses: command
                    .items()
                    .map(|item| Clause {
                        patterns: item
                            .patterns()
                            .map(|word| words::word(self, word.syntax()))
                            .collect(),
                        action: self.list(item.body()),
                        terminator: match item.terminator() {
                            Some(SyntaxKind::SemiAmp) => ";&",
                            Some(SyntaxKind::SemiSemiAmp) => ";;&",
                            _ => ";;",
                        },
                    })
                    .collect(),
            },
            Command::Subshell(command) => Kind::Subshell(self.list(command.body())),
            Command::Group(command) => Kind::Group(self.list(command.body())),
            Command::Arithmetic(command) => {
                Kind::Arith(command.expression(self.tree.source()).to_string())
            }
            Command::Conditional(_) => Kind::Cond(self.conditional(node)),
            Command::Coproc(coproc) => {
                let body = coproc.command();
                let compound = body.is_some_and(|body| !matches!(body, Command::Simple(_)));
                Kind::Coproc {
                    name: match coproc.name() {
                        Some(word) => Some(self.text(word.syntax()).to_string()),
                        None if compound => Some("COPROC".to_string()),
                        None => None,
                    },
                    body: Box::new(body.map(|body| self.command(body)).unwrap_or_default()),
                }
            }
            Command::Function(function) => Kind::Function {
                name: function
                    .name()
                    .map(|word| self.text(word.syntax()).to_string())
                    .unwrap_or_default(),
                body: Box::new(
                    function
                        .body()
                        .map(|body| self.command(body))
                        .unwrap_or_default(),
                ),
            },
        };
        let redirects = node
            .nodes()
            .filter(|child| child.kind() == SyntaxKind::Redirect)
            .map(|child| self.redirect(child))
            .collect();
        Cmd {
            kind,
            redirects,
            ..Cmd::default()
        }
    }

    fn simple(&self, node: &Node) -> Cmd {
        let mut parts = Vec::new();
        let mut redirects = Vec::new();
        for child in node.nodes() {
            match child.kind() {
                SyntaxKind::Assignment => parts.push(words::assignment(self, child)),
                SyntaxKind::Word => parts.push(words::word(self, child)),
                SyntaxKind::Redirect => redirects.push(self.redirect(child)),
                _ => {}
            }
        }
        Cmd {
            kind: Kind::Simple(parts),
            redirects,
            ..Cmd::default()
        }
    }

    /// One redirection, in bash's spelling of it; `named` is a `{name}` written before it.
    fn redirect(&self, node: &Node) -> Redir {
        let mut fd = String::new();
        let mut op = None;
        for token in node.tokens() {
            match token.kind() {
                kind if kind.is_redirect_operator() => op = Some(kind),
                SyntaxKind::Whitespace => {}
                _ if op.is_none() => fd.push_str(token.text(self.tree.source())),
                _ => {}
            }
        }
        let target = node
            .nodes()
            .find(|child| child.kind() == SyntaxKind::Word)
            .map(|word| words::word(self, word))
            .unwrap_or_default();
        let Some(op) = op else {
            return Redir::Plain(self.text(node).to_string());
        };
        if matches!(op, SyntaxKind::LessLess | SyntaxKind::LessLessDash) {
            return self.heredoc(node, &fd, op == SyntaxKind::LessLessDash, &target);
        }
        Redir::Plain(redirection_text(op, &fd, &target))
    }

    fn heredoc(&self, node: &Node, fd: &str, strip: bool, delimiter: &str) -> Redir {
        let quoted = delimiter.contains(['\'', '"', '\\']);
        let bare: String = delimiter
            .chars()
            .filter(|c| !matches!(c, '\'' | '"' | '\\'))
            .collect();
        let fd = if fd == "0" { "" } else { fd };
        let dash = if strip { "-" } else { "" };
        let shown = match quoted {
            true => words::single_quote(&bare),
            false => delimiter.to_string(),
        };
        let raw = self
            .heredocs
            .get(&node.span().start)
            .cloned()
            .unwrap_or_default();
        let mut body: String = match strip {
            true => raw
                .split_inclusive('\n')
                .map(|line| line.trim_start_matches('\t'))
                .collect(),
            false => raw,
        };
        body.push_str(&bare);
        body.push('\n');
        Redir::Heredoc {
            header: format!("{fd}<<{dash}{shown}"),
            body,
        }
    }

    /// The inside of `[[ … ]]`, as bash's `print_cond_node` prints it.
    fn conditional(&self, node: &Node) -> String {
        node.nodes()
            .next()
            .map(|expr| self.cond_node(expr, false, true))
            .unwrap_or_default()
    }

    fn cond_node(&self, node: &Node, invert: bool, term: bool) -> String {
        let not = if invert { "! " } else { "" };
        let children: Vec<&Node> = node.nodes().collect();
        match node.kind() {
            SyntaxKind::CondNot => children
                .first()
                .map(|inner| self.cond_node(inner, !invert, true))
                .unwrap_or_default(),
            // rune keeps a chain of one operator as one node with every operand under it.
            SyntaxKind::CondAnd | SyntaxKind::CondOr if children.len() >= 2 => {
                let op = if node.kind() == SyntaxKind::CondAnd {
                    " && "
                } else {
                    " || "
                };
                let operands: Vec<String> = children
                    .iter()
                    .map(|child| self.cond_node(child, false, true))
                    .collect();
                format!("{not}{}", operands.join(op))
            }
            SyntaxKind::CondGroup => {
                let inner = children
                    .first()
                    .map(|inner| self.cond_node(inner, false, true))
                    .unwrap_or_default();
                format!("{not}( {inner} )")
            }
            SyntaxKind::CondUnary if children.len() == 2 => format!(
                "{not}{} {}",
                self.cond_operand(children[0]),
                self.cond_operand(children[1])
            ),
            SyntaxKind::CondBinary if children.len() == 2 => {
                let op: String = node
                    .tokens()
                    .filter(|token| token.kind() != SyntaxKind::Whitespace)
                    .map(|token| token.text(self.tree.source()))
                    .collect();
                format!(
                    "{not}{} {op} {}",
                    self.cond_operand(children[0]),
                    self.cond_operand(children[1])
                )
            }
            _ if term => format!("{not}-n {}", self.cond_operand(node)),
            _ => self.cond_operand(node),
        }
    }

    fn cond_operand(&self, node: &Node) -> String {
        match node.nodes().find(|child| child.kind() == SyntaxKind::Word) {
            Some(word) if node.kind() == SyntaxKind::CondWord => words::word(self, word),
            _ => words::word(self, node),
        }
    }
}

fn connect(first: Cmd, connector: Connector, second: Option<Cmd>) -> Cmd {
    Cmd {
        kind: Kind::Connection(Box::new(first), connector, second.map(Box::new)),
        ..Cmd::default()
    }
}

/// `time -p` parses as `time` before a command whose first word is `-p`.
fn take_posix_flag(cmd: &mut Cmd) -> bool {
    match &mut cmd.kind {
        Kind::Connection(first, _, _) => take_posix_flag(first),
        Kind::Simple(parts) if parts.first().is_some_and(|word| word == "-p") => {
            parts.remove(0);
            true
        }
        _ => false,
    }
}

/// The three sections of `for ((…))`, as bash prints them: leading blanks trimmed, `1` if empty.
fn arith_sections(text: &str) -> [String; 3] {
    let inner = text.find("((").map_or("", |at| &text[at + 2..]);
    let inner = inner.find("))").map_or(inner, |at| &inner[..at]);
    let mut sections = inner.splitn(3, ';').map(|section| {
        let section = section.trim_start();
        match section.trim().is_empty() {
            true => "1".to_string(),
            false => section.to_string(),
        }
    });
    let mut next = || sections.next().unwrap_or_else(|| "1".to_string());
    [next(), next(), next()]
}

/// A redirection other than a here-document, as bash's `print_redirection` spells it.
fn redirection_text(op: SyntaxKind, fd: &str, target: &str) -> String {
    let unless = |default: &str| if fd == default { "" } else { fd };
    let or = |default: &'static str| if fd.is_empty() { default } else { fd };
    let number = !target.is_empty() && target.bytes().all(|b| b.is_ascii_digit());
    let moved = target
        .strip_suffix('-')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    match op {
        SyntaxKind::Less => format!("{}< {target}", unless("0")),
        SyntaxKind::Great => format!("{}> {target}", unless("1")),
        SyntaxKind::GreatGreat => format!("{}>> {target}", unless("1")),
        SyntaxKind::GreatPipe => format!("{}>| {target}", unless("1")),
        SyntaxKind::LessGreat => format!("{}<> {target}", unless("0")),
        SyntaxKind::LessLessLess => format!("{}<<< {target}", unless("0")),
        SyntaxKind::AmpGreat => format!("&> {target}"),
        SyntaxKind::AmpGreatGreat => format!("&>> {target}"),
        SyntaxKind::LessAmp if target == "-" => format!("{}>&-", or("0")),
        SyntaxKind::GreatAmp if target == "-" => format!("{}>&-", or("1")),
        SyntaxKind::LessAmp if number || moved => format!("{}<&{target}", or("0")),
        SyntaxKind::GreatAmp if number || moved => format!("{}>&{target}", or("1")),
        SyntaxKind::LessAmp => format!("{}<&{target}", unless("0")),
        SyntaxKind::GreatAmp => format!("{}>&{target}", unless("1")),
        _ => format!("{fd}{target}"),
    }
}

/// Every here-document's body text, keyed by where its `<<` starts: the nth `<<` takes the nth
/// body, which is the order the shell reads them in.
fn heredoc_bodies(tree: &Tree) -> HashMap<u32, String> {
    let mut openers = Vec::new();
    let mut bodies = Vec::new();
    collect(tree.root(), tree, &mut openers, &mut bodies);
    openers.into_iter().zip(bodies).collect()
}

fn collect(node: &Node, tree: &Tree, openers: &mut Vec<u32>, bodies: &mut Vec<String>) {
    let mut pending: Option<String> = None;
    for child in node.children() {
        match child {
            Element::Token(token) if token.kind() == SyntaxKind::HeredocText => {
                pending = Some(token.text(tree.source()).to_string());
            }
            Element::Token(token) if token.kind() == SyntaxKind::HeredocEnd => {
                bodies.push(pending.take().unwrap_or_default());
            }
            Element::Node(inner) => {
                let opens = inner.kind() == SyntaxKind::Redirect
                    && inner.tokens().any(|token| {
                        matches!(
                            token.kind(),
                            SyntaxKind::LessLess | SyntaxKind::LessLessDash
                        )
                    });
                if opens {
                    openers.push(inner.span().start);
                }
                collect(inner, tree, openers, bodies);
            }
            Element::Token(_) => {}
        }
    }
    if let Some(body) = pending {
        bodies.push(body);
    }
}
