//! Turn a parsed [`Command`] back into shell source.
//!
//! `type f` on a function has to *show* the function, and the shell threw the source text away at
//! parse time — so the only thing left to print is the tree. The output is deliberately shaped
//! like bash's: a name-`()`-brace header, four spaces per nesting level, and `;` between
//! statements. Two of bash's oddities are reproduced on purpose rather than tidied up, because
//! `type` output is compared against bash byte for byte in the differential suite: the trailing
//! space after `f () ` and after the opening `{ `.
//!
//! Fidelity target: whatever the printer emits must re-parse to the same tree. Quoting is
//! therefore preserved from the tree (`'a  b'` stays single-quoted) rather than re-derived.

use oslo_base::ast::{
    AndOrList, AndOrOp, Assignment, AssignmentTarget, AssignmentValue, CaseItem, Command,
    CommandList, CompoundCommand, ListOp, ParamExpansion, Pipeline, RedirectKind, Redirection,
    SimpleCommand, Subscript, Word, WordPart,
};

const INDENT: &str = "    ";

/// Render `name () { … }` the way `type` and `set` print a function definition.
pub fn format_function(name: &str, body: &Command) -> String {
    let mut out = format!("{name} () \n{{ \n");
    // A function body is normally a `{ … }` group; a `f() ( … )` body is a subshell. bash prints
    // the brace wrapper either way and puts whatever the body is inside it, so unwrap only the
    // group.
    match body {
        Command::Compound {
            kind: CompoundCommand::Group(list),
            redirections,
        } if redirections.is_empty() => {
            for line in format_list(list, 1, false) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        other => {
            for line in format_command(other, 1) {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out.push('}');
    out
}

/// One line per statement, indented `depth` levels.
///
/// `semi_on_last` reproduces bash's split personality: statements inside `then`/`do`/`case` arms
/// all end in `;`, while the last statement of a braced group does not.
fn format_list(list: &CommandList, depth: usize, semi_on_last: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let last = list.items.len().saturating_sub(1);
    for (i, item) in list.items.iter().enumerate() {
        let mut rendered = format_and_or(&item.and_or, depth);
        let Some(tail) = rendered.last_mut() else {
            continue;
        };
        match item.op {
            ListOp::Background => tail.push_str(" &"),
            _ if i < last || semi_on_last => tail.push(';'),
            _ => {}
        }
        lines.append(&mut rendered);
    }
    lines
}

fn format_and_or(and_or: &AndOrList, depth: usize) -> Vec<String> {
    let mut lines = format_pipeline(&and_or.first, depth);
    for (op, pipeline) in &and_or.rest {
        let op = match op {
            AndOrOp::And => " &&",
            AndOrOp::Or => " ||",
        };
        let mut rhs = format_pipeline(pipeline, depth);
        // A single-line right-hand side joins the operator; a multi-line one (a compound command)
        // keeps its own lines and takes the operator on the tail of the left side.
        match (lines.last_mut(), rhs.len()) {
            (Some(tail), 1) => {
                tail.push_str(op);
                tail.push(' ');
                tail.push_str(rhs[0].trim_start());
            }
            (Some(tail), _) => {
                tail.push_str(op);
                lines.append(&mut rhs);
            }
            (None, _) => lines.append(&mut rhs),
        }
    }
    lines
}

fn format_pipeline(pipeline: &Pipeline, depth: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for (i, cmd) in pipeline.commands.iter().enumerate() {
        let mut rendered = format_command(cmd, depth);
        if i == 0 {
            if pipeline.negated
                && let Some(head) = rendered.first_mut()
            {
                let stripped = head.trim_start().to_string();
                *head = format!("{}! {}", INDENT.repeat(depth), stripped);
            }
            lines = rendered;
            continue;
        }
        match (lines.last_mut(), rendered.len()) {
            (Some(tail), 1) => {
                tail.push_str(" | ");
                tail.push_str(rendered[0].trim_start());
            }
            (Some(tail), _) => {
                tail.push_str(" |");
                lines.append(&mut rendered);
            }
            (None, _) => lines = rendered,
        }
    }
    lines
}

fn format_command(cmd: &Command, depth: usize) -> Vec<String> {
    let pad = INDENT.repeat(depth);
    match cmd {
        Command::Simple(simple) => vec![format!("{pad}{}", format_simple(simple))],
        Command::FunctionDef { name, body, .. } => format_function(name, body)
            .lines()
            .map(|l| format!("{pad}{l}"))
            .collect(),
        Command::Compound { kind, redirections } => {
            let mut lines = format_compound(kind, depth);
            if let Some(tail) = lines.last_mut() {
                for r in redirections {
                    tail.push(' ');
                    tail.push_str(&format_redirect(r));
                }
            }
            lines
        }
    }
}

fn format_compound(kind: &CompoundCommand, depth: usize) -> Vec<String> {
    let pad = INDENT.repeat(depth);
    let mut lines = Vec::new();
    match kind {
        CompoundCommand::If {
            condition,
            then_branch,
            elif_branches,
            else_branch,
        } => {
            lines.push(format!("{pad}if {}; then", joined(condition, depth)));
            lines.extend(format_list(then_branch, depth + 1, true));
            for (cond, body) in elif_branches {
                lines.push(format!("{pad}elif {}; then", joined(cond, depth)));
                lines.extend(format_list(body, depth + 1, true));
            }
            if let Some(body) = else_branch {
                lines.push(format!("{pad}else"));
                lines.extend(format_list(body, depth + 1, true));
            }
            lines.push(format!("{pad}fi"));
        }
        CompoundCommand::While { condition, body } => {
            lines.push(format!("{pad}while {}; do", joined(condition, depth)));
            lines.extend(format_list(body, depth + 1, true));
            lines.push(format!("{pad}done"));
        }
        CompoundCommand::Until { condition, body } => {
            lines.push(format!("{pad}until {}; do", joined(condition, depth)));
            lines.extend(format_list(body, depth + 1, true));
            lines.push(format!("{pad}done"));
        }
        CompoundCommand::For {
            var_name,
            items,
            body,
        } => {
            // `for x` with no word list iterates the positionals; bash prints it without `in`.
            match items {
                Some(words) => {
                    let words: Vec<String> = words.iter().map(format_word).collect();
                    lines.push(format!("{pad}for {var_name} in {};", words.join(" ")));
                }
                None => lines.push(format!("{pad}for {var_name};")),
            }
            lines.push(format!("{pad}do"));
            lines.extend(format_list(body, depth + 1, true));
            lines.push(format!("{pad}done"));
        }
        CompoundCommand::Select {
            var_name,
            items,
            body,
        } => {
            // bash spells the implicit word list out: `select y; do` prints as `in "$@"`.
            let words = match items {
                Some(words) => words.iter().map(format_word).collect::<Vec<_>>().join(" "),
                None => "\"$@\"".to_string(),
            };
            lines.push(format!("{pad}select {var_name} in {words};"));
            lines.push(format!("{pad}do"));
            lines.extend(format_list(body, depth + 1, true));
            lines.push(format!("{pad}done"));
        }
        CompoundCommand::Coproc { name, body } => {
            let mut inner = format_command(body, depth);
            let simple = matches!(**body, Command::Simple(_));
            if let Some(first) = inner.first_mut() {
                let text = first.trim_start().to_string();
                *first = match (simple, name.as_str()) {
                    (true, _) | (false, "COPROC") => format!("{pad}coproc {text}"),
                    (false, _) => format!("{pad}coproc {name} {text}"),
                };
            }
            lines.extend(inner);
        }
        CompoundCommand::Case { word, items } => {
            lines.push(format!("{pad}case {} in ", format_word(word)));
            for CaseItem {
                patterns,
                body,
                post_action,
            } in items
            {
                let pats: Vec<String> = patterns.iter().map(format_word).collect();
                lines.push(format!("{pad}{INDENT}{})", pats.join(" | ")));
                lines.extend(format_list(body, depth + 2, false));
                // The terminator the branch was written with: `;&` and `;;&` select different
                // branches from `;;`, so printing `;;` for all three would not re-parse.
                lines.push(format!("{pad}{INDENT}{}", post_action.terminator()));
            }
            lines.push(format!("{pad}esac"));
        }
        CompoundCommand::Arithmetic(expr) => lines.push(format!("{pad}(( {expr} ))")),
        CompoundCommand::ArithmeticFor {
            init,
            cond,
            step,
            body,
        } => {
            let section = |e: &Option<String>| e.clone().unwrap_or_default();
            lines.push(format!(
                "{pad}for (( {}; {}; {} )); do",
                section(init),
                section(cond),
                section(step)
            ));
            lines.extend(format_list(body, depth + 1, true));
            lines.push(format!("{pad}done"));
        }
        CompoundCommand::Subshell(list) => {
            lines.push(format!("{pad}( {} )", joined(list, depth)));
        }
        CompoundCommand::Group(list) => {
            lines.push(format!("{pad}{{ "));
            lines.extend(format_list(list, depth + 1, false));
            lines.push(format!("{pad}}}"));
        }
    }
    lines
}

/// A command list squeezed onto one line, for the `if …; then` / `( … )` positions.
fn joined(list: &CommandList, depth: usize) -> String {
    let mut out = String::new();
    for line in format_list(list, depth, false) {
        let line = line.trim_start();
        if !out.is_empty() {
            // A closing brace needs a command terminator in front of it or `{ echo a }` re-parses
            // as the command `{` with two arguments.
            let needs_semi = line.starts_with('}') && !out.ends_with(';');
            out.push_str(if needs_semi { "; " } else { " " });
        }
        out.push_str(line);
    }
    out
}

pub(crate) fn format_simple(simple: &SimpleCommand) -> String {
    let mut parts: Vec<String> = Vec::new();
    for a in &simple.assignments {
        parts.push(format_assignment(a));
    }
    parts.extend(simple.words.iter().map(format_word));
    parts.extend(simple.redirections.iter().map(format_redirect));
    parts.join(" ")
}

/// Render an assignment back to source, in whichever of its four shapes it has.
fn format_assignment(a: &Assignment) -> String {
    let target = match &a.target {
        AssignmentTarget::Name(name) => name.clone(),
        AssignmentTarget::Element { name, index } => format!("{name}[{}]", format_word(index)),
    };
    let op = if a.append { "+=" } else { "=" };
    let value = match &a.value {
        AssignmentValue::Scalar(word) => format_word(word),
        AssignmentValue::Array(elements) => {
            let rendered: Vec<String> = elements
                .iter()
                .map(|e| match &e.index {
                    Some(index) => format!("[{}]={}", format_word(index), format_word(&e.value)),
                    None => format_word(&e.value),
                })
                .collect();
            format!("({})", rendered.join(" "))
        }
    };
    format!("{target}{op}{value}")
}

fn format_redirect(r: &Redirection) -> String {
    let fd = r.fd.map(|n| n.to_string()).unwrap_or_default();
    let target = format_word(&r.target);
    match r.kind {
        // The duplicating and here-document forms take their operand with no space, the way they
        // must be written to re-parse as the same thing.
        RedirectKind::DupInput => format!("{fd}<&{target}"),
        RedirectKind::DupOutput => format!("{fd}>&{target}"),
        RedirectKind::Heredoc => format!("{fd}<<{target}"),
        RedirectKind::HeredocStrip => format!("{fd}<<-{target}"),
        RedirectKind::Input => format!("{fd}< {target}"),
        RedirectKind::Output => format!("{fd}> {target}"),
        RedirectKind::Append => format!("{fd}>> {target}"),
        RedirectKind::ReadWrite => format!("{fd}<> {target}"),
        RedirectKind::Clobber => format!("{fd}>| {target}"),
    }
}

pub fn format_word(word: &Word) -> String {
    word.parts.iter().map(format_part).collect()
}

fn format_part(part: &WordPart) -> String {
    match part {
        WordPart::Literal(s) => s.clone(),
        // The lexer already removed the backslashes, so they have to go back on or the word
        // re-parses as something else (`\*` would glob).
        WordPart::Escaped(s) => s.chars().map(|c| format!("\\{c}")).collect(),
        WordPart::SingleQuoted(s) => format!("'{s}'"),
        WordPart::DoubleQuoted(parts) => {
            let inner: String = parts
                .iter()
                .map(|p| match p {
                    WordPart::SingleQuoted(s) => s.clone(),
                    other => format_part(other),
                })
                .collect();
            format!("\"{inner}\"")
        }
        WordPart::Variable {
            name,
            expansion_type,
        } => format_param(name, expansion_type),
        // A subscripted reference has to stay braced even in the plain case: `$a[0]` re-parses as
        // `$a` followed by the literal `[0]`.
        WordPart::ArrayRef {
            name,
            subscript,
            expansion_type,
        } => {
            let subscripted = format!("{name}[{}]", format_subscript(subscript));
            match expansion_type {
                ParamExpansion::Normal => format!("${{{subscripted}}}"),
                other => format_param(&subscripted, other),
            }
        }
        WordPart::CommandSubstitution(src) => format!("$({src})"),
        WordPart::FunctionSubstitution { body, reply } => {
            format!("${{{}{body}}}", if *reply { "|" } else { "" })
        }
        WordPart::ProcessSubstitution {
            reads_from_command,
            command,
        } => format!("{}({command})", if *reads_from_command { '<' } else { '>' }),
        WordPart::Arithmetic(src) => format!("$(({src}))"),
        WordPart::Tilde(rest) => format!("~{rest}"),
    }
}

fn format_subscript(subscript: &Subscript) -> String {
    match subscript {
        Subscript::All => "@".to_string(),
        Subscript::Joined => "*".to_string(),
        Subscript::Index(word) => format_word(word),
    }
}

fn format_param(name: &str, expansion: &ParamExpansion) -> String {
    let colon = |test_null: bool| if test_null { ":" } else { "" };
    match expansion {
        ParamExpansion::Normal => format!("${name}"),
        ParamExpansion::Length => format!("${{#{name}}}"),
        ParamExpansion::DefaultValue {
            default,
            assign_if_unset,
            test_null,
        } => {
            let op = if *assign_if_unset { "=" } else { "-" };
            format!(
                "${{{name}{}{op}{}}}",
                colon(*test_null),
                format_word(default)
            )
        }
        ParamExpansion::UseAlternative {
            alternative,
            test_null,
        } => format!(
            "${{{name}{}+{}}}",
            colon(*test_null),
            format_word(alternative)
        ),
        ParamExpansion::ErrorIfUnset { message, test_null } => {
            format!("${{{name}{}?{}}}", colon(*test_null), format_word(message))
        }
        ParamExpansion::RemoveSuffix { pattern, longest } => {
            let op = if *longest { "%%" } else { "%" };
            format!("${{{name}{op}{}}}", format_word(pattern))
        }
        ParamExpansion::RemovePrefix { pattern, longest } => {
            let op = if *longest { "##" } else { "#" };
            format!("${{{name}{op}{}}}", format_word(pattern))
        }
        ParamExpansion::Substring { offset, length } => match length {
            Some(len) => format!("${{{name}:{}:{}}}", format_word(offset), format_word(len)),
            None => format!("${{{name}:{}}}", format_word(offset)),
        },
        ParamExpansion::Replace {
            pattern,
            replacement,
            scope,
        } => {
            use oslo_base::ast::ReplaceScope;
            let op = match scope {
                ReplaceScope::First => "/",
                ReplaceScope::All => "//",
                ReplaceScope::Prefix => "/#",
                ReplaceScope::Suffix => "/%",
            };
            format!(
                "${{{name}{op}{}/{}}}",
                format_word(pattern),
                format_word(replacement)
            )
        }
        ParamExpansion::CaseConvert {
            pattern,
            upper,
            all,
        } => {
            let op = match (upper, all) {
                (true, true) => "^^",
                (true, false) => "^",
                (false, true) => ",,",
                (false, false) => ",",
            };
            let pat = pattern.as_ref().map(format_word).unwrap_or_default();
            format!("${{{name}{op}{pat}}}")
        }
        ParamExpansion::CaseToggle { pattern, all } => {
            let op = if *all { "~~" } else { "~" };
            let pat = pattern.as_ref().map(format_word).unwrap_or_default();
            format!("${{{name}{op}{pat}}}")
        }
        ParamExpansion::Transform(op) => format!("${{{name}@{op}}}"),
        // The `!` belongs to the reference, so the inner form is rendered against a name that
        // already carries it — `${!v:-d}` rather than `${!v}:-d`.
        ParamExpansion::Indirect(inner) => format_param(&format!("!{name}"), inner),
    }
}

#[cfg(test)]
mod tests {
    use super::format_function;
    use oslo_base::ast::Command;

    fn func(src: &str) -> String {
        let list = crate::syntax::parse_bash_script(src).expect("parses");
        for item in &list.items {
            for cmd in &item.and_or.first.commands {
                if let Command::FunctionDef { name, body, .. } = cmd {
                    return format_function(name, body);
                }
            }
        }
        panic!("no function definition in {src:?}");
    }

    /// The exact bytes bash prints, trailing spaces included — the differential suite compares
    /// `type f` output against bash without normalising whitespace.
    #[test]
    fn simple_body_matches_bash_layout() {
        assert_eq!(func("f() { echo hi; }"), "f () \n{ \n    echo hi\n}");
    }

    #[test]
    fn statements_are_semicolon_separated_except_the_last() {
        assert_eq!(
            func("f() { echo a; echo b; }"),
            "f () \n{ \n    echo a;\n    echo b\n}"
        );
    }

    #[test]
    fn quoting_and_redirections_survive() {
        assert_eq!(
            func("f() { echo 'a  b' \"x$y\" >out; }"),
            "f () \n{ \n    echo 'a  b' \"x$y\" > out\n}"
        );
    }

    #[test]
    fn pipelines_and_and_or_stay_on_one_line() {
        assert_eq!(
            func("f() { echo a && echo b | cat; }"),
            "f () \n{ \n    echo a && echo b | cat\n}"
        );
    }

    /// Round trip: whatever comes out has to parse back to the same tree, or `type` is printing
    /// a function the shell would not run.
    #[test]
    fn output_reparses_to_the_same_tree() {
        for src in [
            "f() { echo hi; }",
            "f() { echo a; echo b; }",
            "f() { if true; then echo a; fi; }",
            "f() { while false; do echo x; done; }",
            "f() { for i in 1 2 3; do echo $i; done; }",
            "f() { case $x in a|b) echo m ;; *) echo n ;; esac; }",
            "f() { x=1 y=2 cmd arg >out 2>&1; }",
            "f() { ! echo a | grep b; }",
            "f() { echo \"${x:-d}\" ${y#p} $((1 + 2)); }",
            "f() { if { true; }; then echo a; fi; }",
            "f() { g() { echo inner; }; g; }",
        ] {
            let printed = func(src);
            let reparsed = crate::syntax::parse_bash_script(&printed)
                .unwrap_or_else(|e| panic!("{printed}\n=> {e}"));
            let original = crate::syntax::parse_bash_script(src).expect("parses");
            assert_eq!(reparsed, original, "round trip changed {src:?}:\n{printed}");
        }
    }
}
