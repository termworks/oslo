//! A port of bash's `print_cmd.c`: the state it keeps in globals and the order it writes in.
//!
//! Each method is named after the bash function it follows, and keeps its quirks: the trailing
//! space after `{`, the blank line after a here-document, `;` before `then` and `do`.

use super::{Clause, Cmd, Connector, Kind, Redir};

const INDENT: usize = 4;

/// Words bash prints a `function` keyword before, when one names a function.
const RESERVED: [&str; 22] = [
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "time", "{", "}", "!", "[[", "]]", "coproc",
];

struct Printer {
    out: String,
    indentation: usize,
    amount: usize,
    /// bash's `skip_this_indent`: how many coming commands start without their indentation.
    skip: usize,
    was_heredoc: bool,
    deferred: Vec<String>,
    printing_connection: usize,
    inside_function_def: usize,
    comsub: bool,
}

impl Printer {
    fn new(comsub: bool) -> Self {
        Printer {
            out: String::new(),
            indentation: 0,
            amount: INDENT,
            skip: 0,
            was_heredoc: false,
            deferred: Vec::new(),
            printing_connection: 0,
            inside_function_def: 0,
            comsub,
        }
    }

    fn put(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn indent(&mut self, amount: usize) {
        self.out.extend(std::iter::repeat_n(' ', amount));
    }

    fn newline(&mut self, text: &str) {
        self.put("\n");
        self.indent(self.indentation);
        self.put(text);
    }

    fn semicolon(&mut self) {
        if !self.out.ends_with(['&', '\n']) {
            self.put(";");
        }
    }

    /// `make_command_string_internal`.
    fn command(&mut self, cmd: Option<&Cmd>) {
        let Some(cmd) = cmd else {
            return;
        };
        match self.skip {
            0 => self.indent(self.indentation),
            _ => self.skip -= 1,
        }
        if let Some(time) = cmd.time {
            self.put(time);
        }
        if cmd.invert {
            self.put("! ");
        }
        match &cmd.kind {
            Kind::Empty => {}
            Kind::Simple(words) => {
                self.put(&words.join(" "));
                if !cmd.redirects.is_empty() {
                    if !words.is_empty() {
                        self.put(" ");
                    }
                    self.redirections(&cmd.redirects);
                }
                return;
            }
            Kind::Connection(first, connector, second) => {
                self.connection(first, *connector, second.as_deref())
            }
            Kind::For {
                keyword,
                name,
                words,
                body,
            } => {
                self.put(&format!("{keyword} {name} in {}", words.join(" ")));
                self.put(";");
                self.newline("do\n");
                self.loop_body(body.as_deref());
            }
            Kind::ArithFor { sections, body } => {
                self.put(&format!("for (({}))", sections.join("; ")));
                self.newline("do\n");
                self.loop_body(body.as_deref());
            }
            Kind::Case { word, clauses } => {
                self.put(&format!("case {word} in "));
                self.case_clauses(clauses);
                self.newline("esac");
            }
            Kind::Loop {
                keyword,
                test,
                body,
            } => {
                self.put(&format!("{keyword} "));
                self.skip += 1;
                self.command(test.as_deref());
                self.deferred_heredocs("");
                self.semicolon();
                self.put(" do\n");
                self.indentation += self.amount;
                self.command(body.as_deref());
                self.deferred_heredocs("");
                self.indentation -= self.amount;
                self.semicolon();
                self.newline("done");
            }
            Kind::If {
                test,
                then,
                otherwise,
            } => self.if_command(test.as_deref(), then.as_deref(), otherwise.as_deref()),
            Kind::Arith(expr) => self.put(&format!("(({expr}))")),
            Kind::Cond(text) => self.put(&format!("[[ {text} ]]")),
            Kind::Group(list) => self.group(list.as_deref()),
            Kind::Subshell(list) => {
                self.put("( ");
                self.skip += 1;
                self.command(list.as_deref());
                self.deferred_heredocs("");
                self.put(" )");
            }
            Kind::Function { name, body } => self.function_def(name, body),
            Kind::Coproc { name, body } => {
                self.put("coproc ");
                if let Some(name) = name {
                    self.put(&format!("{name} "));
                }
                self.skip += 1;
                self.command(Some(body));
            }
        }
        if !cmd.redirects.is_empty() {
            self.put(" ");
            self.redirections(&cmd.redirects);
        }
    }

    fn connection(&mut self, first: &Cmd, connector: Connector, second: Option<&Cmd>) {
        self.skip += 1;
        self.printing_connection += 1;
        self.command(Some(first));
        match connector {
            Connector::Amp | Connector::Pipe => {
                let text = if connector == Connector::Amp {
                    " &"
                } else {
                    " |"
                };
                self.print_deferred_heredocs(text);
                if connector == Connector::Pipe || second.is_some() {
                    self.put(" ");
                    self.skip += 1;
                }
            }
            Connector::And | Connector::Or => {
                let text = if connector == Connector::And {
                    " && "
                } else {
                    " || "
                };
                self.print_deferred_heredocs(text);
                if second.is_some() {
                    self.skip += 1;
                }
            }
            Connector::Semi | Connector::Newline => {
                let separator = match (self.comsub, connector) {
                    (true, Connector::Newline) => "\n",
                    _ => ";",
                };
                if self.deferred.is_empty() {
                    match self.was_heredoc {
                        false => self.put(separator),
                        true => self.was_heredoc = false,
                    }
                } else {
                    let text = if self.inside_function_def > 0 {
                        ""
                    } else {
                        ";"
                    };
                    self.print_deferred_heredocs(text);
                }
                if self.inside_function_def > 0 {
                    self.put("\n");
                } else {
                    if connector == Connector::Semi {
                        self.put(" ");
                    }
                    self.skip += 1;
                }
            }
        }
        self.command(second);
        self.deferred_heredocs("");
        self.printing_connection -= 1;
    }

    fn loop_body(&mut self, body: Option<&Cmd>) {
        self.indentation += self.amount;
        self.command(body);
        self.deferred_heredocs("");
        self.semicolon();
        self.indentation -= self.amount;
        self.newline("done");
    }

    fn case_clauses(&mut self, clauses: &[Clause]) {
        self.indentation += self.amount;
        for (at, clause) in clauses.iter().enumerate() {
            if !(self.comsub && at == 0) {
                self.newline("");
            }
            self.put(&clause.patterns.join(" | "));
            self.put(")\n");
            self.indentation += self.amount;
            self.command(clause.action.as_deref());
            self.indentation -= self.amount;
            self.deferred_heredocs("");
            self.newline(clause.terminator);
        }
        self.indentation -= self.amount;
    }

    fn if_command(&mut self, test: Option<&Cmd>, then: Option<&Cmd>, otherwise: Option<&Cmd>) {
        self.put("if ");
        self.skip += 1;
        self.command(test);
        self.semicolon();
        self.put(" then\n");
        self.indentation += self.amount;
        self.command(then);
        self.deferred_heredocs("");
        self.indentation -= self.amount;
        if let Some(otherwise) = otherwise {
            self.semicolon();
            self.newline("else\n");
            self.indentation += self.amount;
            self.command(Some(otherwise));
            self.deferred_heredocs("");
            self.indentation -= self.amount;
        }
        self.semicolon();
        self.newline("fi");
    }

    fn group(&mut self, list: Option<&Cmd>) {
        self.put("{ ");
        match self.inside_function_def {
            0 => self.skip += 1,
            _ => {
                self.put("\n");
                self.indentation += self.amount;
            }
        }
        self.command(list);
        self.deferred_heredocs("");
        if self.inside_function_def > 0 {
            self.put("\n");
            self.indentation -= self.amount;
            self.indent(self.indentation);
        } else {
            self.semicolon();
            self.put(" ");
        }
        self.put("}");
    }

    /// `print_function_def`: a definition nested in a body.
    fn function_def(&mut self, name: &str, body: &Cmd) {
        self.put(&format!("function {name} () \n"));
        self.indent(self.indentation);
        self.put("{ \n");
        self.inside_function_def += 1;
        self.indentation += self.amount;
        let (inner, redirects) = split_group(body);
        self.command(inner);
        self.deferred_heredocs("");
        self.indentation -= self.amount;
        self.inside_function_def -= 1;
        self.close_function(redirects);
    }

    fn close_function(&mut self, redirects: &[Redir]) {
        match redirects.is_empty() {
            true => self.newline("}"),
            false => {
                self.newline("} ");
                self.redirections(redirects);
            }
        }
    }

    /// `print_redirection_list`: here-document bodies wait for the end of the line.
    fn redirections(&mut self, redirects: &[Redir]) {
        let mut heredocs = Vec::new();
        self.was_heredoc = false;
        for (at, redirect) in redirects.iter().enumerate() {
            match redirect {
                Redir::Plain(text) => self.put(text),
                Redir::Heredoc { header, body } => {
                    self.put(header);
                    heredocs.push(body.clone());
                }
            }
            if at + 1 < redirects.len() {
                self.put(" ");
            }
        }
        if heredocs.is_empty() {
            return;
        }
        match self.printing_connection {
            0 => self.heredoc_bodies(heredocs),
            _ => self.deferred = heredocs,
        }
    }

    fn heredoc_bodies(&mut self, bodies: Vec<String>) {
        self.put("\n");
        for body in bodies {
            self.put(&body);
            self.was_heredoc = true;
        }
    }

    /// `print_deferred_heredocs`.
    fn print_deferred_heredocs(&mut self, text: &str) {
        let shown = !text.is_empty() && text != ";";
        if shown {
            self.put(text);
        }
        if !self.deferred.is_empty() {
            let bodies = std::mem::take(&mut self.deferred);
            self.heredoc_bodies(bodies);
            if shown {
                self.put(" ");
            }
            self.was_heredoc = true;
        }
    }

    /// `PRINT_DEFERRED_HEREDOCS`.
    fn deferred_heredocs(&mut self, text: &str) {
        if !self.deferred.is_empty() {
            self.print_deferred_heredocs(text);
        }
    }
}

/// A group body is printed as the function's braces, its redirections after the closing one.
fn split_group(body: &Cmd) -> (Option<&Cmd>, &[Redir]) {
    match &body.kind {
        Kind::Group(list) => (list.as_deref(), &body.redirects),
        _ => (Some(body), &[]),
    }
}

/// `named_function_string`: `name () { … }` the way `declare -f` prints it, or with `multiline`
/// off the way an exported function's value is written.
pub(super) fn function(name: &str, body: &Cmd, multiline: bool) -> String {
    let mut printer = Printer::new(false);
    if !name.is_empty() {
        if RESERVED.contains(&name) {
            printer.put("function ");
        }
        printer.put(name);
        printer.put(" ");
    }
    printer.put("() ");
    match multiline {
        true => {
            printer.put("\n");
            printer.indentation += printer.amount;
        }
        false => {
            printer.indentation = 1;
            printer.amount = 0;
        }
    }
    printer.inside_function_def += 1;
    printer.put(if multiline { "{ \n" } else { "{ " });
    let (inner, redirects) = split_group(body);
    printer.command(inner);
    printer.deferred_heredocs("");
    printer.indentation = 0;
    printer.amount = INDENT;
    printer.inside_function_def -= 1;
    printer.close_function(redirects);
    printer.out
}

/// `print_comsub`: a command substitution's body.
pub(super) fn command_string(cmd: &Cmd) -> String {
    let mut printer = Printer::new(true);
    printer.command(Some(cmd));
    printer.out
}
