//! Turning a piece of AST back into something like the text the user typed.
//!
//! R7.2: the job table has to label its entries, because `jobs` and the `[1]+ Done` notice both
//! name the job by its command. The parser keeps no source spans, so the label is reconstructed
//! from the tree.
//!
//! It is deliberately *approximate*, and the approximation is one-directional: what comes out is
//! never re-parsed or re-executed, only printed. Quoting is therefore rendered as the user wrote
//! it where that is knowable (`'x'` stays quoted) and elided where it is not, and a construct with
//! no useful short form — a whole `if`, a `case` — collapses to its keyword. Getting a label
//! slightly wrong costs a cosmetic difference in one line of `jobs` output; trying to be exact
//! would mean a second, unrunnable copy of the grammar.

use oslo_base::ast::*;

/// The label a background or stopped job carries in the job table.
pub(crate) fn describe_and_or(and_or: &AndOrList) -> String {
    let mut text = describe_pipeline(&and_or.first);
    for (op, pipeline) in &and_or.rest {
        let sep = match op {
            AndOrOp::And => " && ",
            AndOrOp::Or => " || ",
        };
        text.push_str(sep);
        text.push_str(&describe_pipeline(pipeline));
    }
    text
}

pub(crate) fn describe_pipeline(pipeline: &Pipeline) -> String {
    let stages: Vec<String> = pipeline.commands.iter().map(describe_command).collect();
    let body = stages.join(" | ");
    match (pipeline.negated, pipeline.timed) {
        (true, true) => format!("! time {}", body),
        (true, false) => format!("! {}", body),
        (false, true) => format!("time {}", body),
        (false, false) => body,
    }
}

pub(crate) fn describe_command(command: &Command) -> String {
    match command {
        Command::Simple(simple) => describe_simple(simple),
        Command::Compound { kind, .. } => describe_compound(kind),
        Command::FunctionDef { name, .. } => format!("{}()", name),
    }
}

/// `coproc …` as `jobs` shows it: the name only when the script gave a compound command one.
pub(crate) fn describe_coproc(name: &str, body: &Command) -> String {
    let text = describe_command(body);
    match matches!(body, Command::Simple(_)) || name == "COPROC" {
        true => format!("coproc {text}"),
        false => format!("coproc {name} {text}"),
    }
}

fn describe_simple(simple: &SimpleCommand) -> String {
    let mut parts: Vec<String> = simple.assignments.iter().map(describe_assignment).collect();
    parts.extend(simple.words.iter().map(describe_word));
    parts.join(" ")
}

fn describe_assignment(assignment: &Assignment) -> String {
    let name = match &assignment.target {
        AssignmentTarget::Name(name) => name.clone(),
        AssignmentTarget::Element { name, index } => format!("{}[{}]", name, describe_word(index)),
    };
    let op = if assignment.append { "+=" } else { "=" };
    let value = match &assignment.value {
        AssignmentValue::Scalar(word) => describe_word(word),
        AssignmentValue::Array(elements) => {
            let items: Vec<String> = elements.iter().map(|e| describe_word(&e.value)).collect();
            format!("({})", items.join(" "))
        }
    };
    format!("{}{}{}", name, op, value)
}

/// A compound command is named by its keyword: a job label is one line, and a `while` loop is not.
fn describe_compound(kind: &CompoundCommand) -> String {
    match kind {
        CompoundCommand::If { .. } => "if ...".to_string(),
        CompoundCommand::While { .. } => "while ...".to_string(),
        CompoundCommand::Until { .. } => "until ...".to_string(),
        CompoundCommand::For { var_name, .. } => format!("for {} ...", var_name),
        CompoundCommand::ArithmeticFor { .. } => "for ((...))".to_string(),
        CompoundCommand::Select { var_name, .. } => format!("select {} ...", var_name),
        CompoundCommand::Coproc { name, body } => describe_coproc(name, body),
        CompoundCommand::Case { .. } => "case ...".to_string(),
        CompoundCommand::Arithmetic(expr) => format!("(( {} ))", expr),
        CompoundCommand::Subshell(list) => format!("( {} )", describe_list(list)),
        CompoundCommand::Group(list) => format!("{{ {}; }}", describe_list(list)),
    }
}

fn describe_list(list: &CommandList) -> String {
    let items: Vec<String> = list
        .items
        .iter()
        .map(|item| {
            let text = describe_and_or(&item.and_or);
            if item.op == ListOp::Background {
                format!("{} &", text)
            } else {
                text
            }
        })
        .collect();
    items.join("; ")
}

pub(crate) fn describe_word(word: &Word) -> String {
    word.parts.iter().map(describe_part).collect()
}

fn describe_part(part: &WordPart) -> String {
    match part {
        WordPart::Literal(text) => text.clone(),
        WordPart::Escaped(text) => format!("\\{}", text),
        WordPart::SingleQuoted(text) => format!("'{}'", text),
        WordPart::DoubleQuoted(parts) => {
            let inner: String = parts.iter().map(describe_part).collect();
            format!("\"{}\"", inner)
        }
        // The operator inside `${...}` is dropped: `${x:-d}` prints as `$x`. A label is not a
        // program, and reproducing the operand grammar here would duplicate `expand::param`.
        WordPart::Variable { name, .. } => format!("${}", name),
        WordPart::ArrayRef { name, .. } => format!("${{{}[...]}}", name),
        WordPart::CommandSubstitution(text) => format!("$({})", text),
        WordPart::FunctionSubstitution { body, reply } => {
            format!("${{{}{body}}}", if *reply { "|" } else { "" })
        }
        WordPart::ProcessSubstitution {
            reads_from_command,
            command,
        } => format!("{}({command})", if *reads_from_command { '<' } else { '>' }),
        WordPart::Arithmetic(text) => format!("$(({}))", text),
        WordPart::Tilde(rest) => format!("~{}", rest),
    }
}

#[cfg(test)]
mod tests {
    use super::describe_and_or;
    use crate::syntax::parse_bash_script;

    fn label(src: &str) -> String {
        let list = parse_bash_script(src).expect("parse");
        describe_and_or(&list.items[0].and_or)
    }

    /// The common case: the label is the command, close enough to read at a prompt.
    #[test]
    fn a_simple_command_reads_back_as_itself() {
        assert_eq!(label("sleep 10"), "sleep 10");
        assert_eq!(label("grep -n 'x y' file"), "grep -n 'x y' file");
        assert_eq!(label("x=1 env"), "x=1 env");
    }

    /// Pipelines and and-or lists keep their operators, because a job is the whole list.
    #[test]
    fn operators_survive() {
        assert_eq!(label("a | b | c"), "a | b | c");
        assert_eq!(label("a && b || c"), "a && b || c");
        assert_eq!(label("! a"), "! a");
    }

    /// A compound command collapses to its keyword rather than its body.
    #[test]
    fn a_compound_command_is_named_not_reproduced() {
        assert_eq!(label("while true; do :; done"), "while ...");
        assert_eq!(label("( sleep 1; echo x )"), "( sleep 1; echo x )");
    }

    /// Expansions are shown in a recognisable short form; nothing here is ever re-parsed.
    #[test]
    fn expansions_are_shown_approximately() {
        assert_eq!(label("echo $HOME"), "echo $HOME");
        assert_eq!(label("echo ${x:-d}"), "echo $x");
        assert_eq!(label("echo \"a $b\""), "echo \"a $b\"");
    }
}
