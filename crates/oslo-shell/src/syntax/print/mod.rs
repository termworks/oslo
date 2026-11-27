//! Printing a function definition back the way bash prints it, for `declare -f`, `type` and the
//! value `export -f` puts in the environment.
//!
//! The source text is re-parsed with rune, whose tree keeps everything bash's printer reads:
//! here-document bodies, quoting, `[[ ]]` expressions, `time`, `|&` and unexpanded braces. The
//! tree is turned into a command tree shaped like bash's own ([`build`]), which is then printed
//! by a port of bash's `print_cmd.c` ([`emit`]), down to its trailing spaces.

mod build;
mod emit;
mod words;

/// One command, as bash's printer sees it.
#[derive(Debug, Default)]
struct Cmd {
    kind: Kind,
    /// `time ` or `time -p `, printed before everything else.
    time: Option<&'static str>,
    invert: bool,
    redirects: Vec<Redir>,
}

#[derive(Debug, Default)]
enum Kind {
    #[default]
    Empty,
    Simple(Vec<String>),
    Connection(Box<Cmd>, Connector, Option<Box<Cmd>>),
    For {
        keyword: &'static str,
        name: String,
        words: Vec<String>,
        body: Option<Box<Cmd>>,
    },
    ArithFor {
        sections: [String; 3],
        body: Option<Box<Cmd>>,
    },
    Case {
        word: String,
        clauses: Vec<Clause>,
    },
    Loop {
        keyword: &'static str,
        test: Option<Box<Cmd>>,
        body: Option<Box<Cmd>>,
    },
    If {
        test: Option<Box<Cmd>>,
        then: Option<Box<Cmd>>,
        otherwise: Option<Box<Cmd>>,
    },
    Arith(String),
    Cond(String),
    Group(Option<Box<Cmd>>),
    Subshell(Option<Box<Cmd>>),
    Function {
        name: String,
        body: Box<Cmd>,
    },
}

#[derive(Debug)]
struct Clause {
    patterns: Vec<String>,
    action: Option<Box<Cmd>>,
    terminator: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Connector {
    Semi,
    Newline,
    Amp,
    And,
    Or,
    Pipe,
}

#[derive(Debug, Clone)]
enum Redir {
    Plain(String),
    /// A here-document: its `<<EOF` header and its body, delimiter line included.
    Heredoc {
        header: String,
        body: String,
    },
}

/// The definition in `source` printed as bash prints it, named `name`; `multiline` is the
/// `declare -f` layout, and without it the one-line-per-command layout of an exported function.
pub fn function_text(source: &str, name: &str, multiline: bool) -> Option<String> {
    let parsed = rune::parse(source);
    let tree = parsed.tree();
    let node = first_function(tree.root())?;
    let body = rune::ast::FunctionDef::cast(node)?.body()?;
    let body = build::Builder::new(tree, false).command(body);
    Some(emit::function(name, &body, multiline))
}

fn first_function(node: &rune::Node) -> Option<&rune::Node> {
    if node.kind() == rune::SyntaxKind::FunctionDef {
        return Some(node);
    }
    node.nodes().find_map(first_function)
}

#[cfg(test)]
mod tests;
