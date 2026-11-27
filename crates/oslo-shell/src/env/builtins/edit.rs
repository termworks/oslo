//! `funced` and `vared` — change a function or a variable without retyping it.
//!
//! Both answer the same complaint: the thing you want to change is already in the shell, and the
//! only way to change it was to write it out again from the beginning. A twenty-line function with
//! one wrong word had to be re-entered or kept in a file it did not otherwise need.
//!
//! ```sh
//! funced deploy          # the definition, in $EDITOR; reloaded when you save
//! funced --save deploy   # and kept, so the next session has it
//! vared PATH             # the value, in oslo's own line editor
//! ```
//!
//! Two builtins rather than one, because they edit different things in different places: a
//! function is many lines and belongs in an editor, a variable is one line and belongs in the line
//! editor that is already on the screen.

use crate::env::Environment;
use oslo_base::error::Result;

/// `funced [--save] NAME` — edit a shell function in `$EDITOR`.
///
/// The definition is written out in the shape `type` prints, which is the shape that re-parses to
/// the same tree — so what comes back is read by the same parser that read the original, and a
/// round trip through the editor with no change is a no-op rather than a reformatting.
///
/// **A function that does not parse leaves the old one alone.** The edited text is parsed before
/// anything is replaced, so a mistake costs the edit and not the function: the shell you are
/// standing in still has the definition that worked.
pub fn builtin_funced(env: &mut Environment, args: &[String]) -> Result<i32> {
    let mut save = false;
    let mut name = None;
    for argument in args.iter().skip(1) {
        match argument.as_str() {
            "--save" | "-s" => save = true,
            other if other.starts_with('-') => {
                crate::env::complain(
                    args,
                    other,
                    &format!("funced: {other}: not an option"),
                    "no option of that name",
                    Some("`funced [--save] NAME`"),
                );
                return Ok(2);
            }
            other => name = Some(other.to_string()),
        }
    }
    let Some(name) = name else {
        crate::env::complain(
            args,
            "funced",
            "funced: needs the name of a function",
            "no function named",
            Some("`funced NAME` opens that function in $EDITOR"),
        );
        return Ok(2);
    };

    // An unknown name opens an empty definition rather than refusing: writing a new function is
    // the same act as changing an old one, and refusing would send you to a file to start it in.
    let source = match env.function_definition(&name) {
        Some(definition) => definition,
        None => format!("{name}() {{\n    \n}}\n"),
    };

    let Some(edited) = edit_in_editor(env, &name, &source)? else {
        return Ok(1);
    };
    if edited.trim() == source.trim() {
        return Ok(0);
    }

    // Parsed before anything is replaced. A definition that does not parse must not take the
    // working one with it.
    let parsed = match crate::syntax::parse_bash_script(&edited) {
        Ok(parsed) => parsed,
        Err(problem) => {
            eprintln!("{}funced: {name}: {problem}", crate::env::origin_now());
            eprintln!("funced: the function is unchanged");
            return Ok(2);
        }
    };
    let status = crate::exec::eval_command_list(env, &parsed)?;
    if status != 0 {
        return Ok(status);
    }
    if save {
        return Ok(save_function(env, &name, &edited));
    }
    Ok(0)
}

/// Write the definition where autoloading will find it next session.
///
/// The same directory `NAME.sh` is autoloaded from, so `funced --save` and a hand-written file are
/// the same thing — there is no second place a function can live.
fn save_function(env: &Environment, name: &str, source: &str) -> i32 {
    let Some(directory) = crate::exec::simple::autoload::directory(env) else {
        eprintln!("funced: there is nowhere to save to; set $OSLO_FUNCTIONS");
        return 1;
    };
    if let Err(problem) = std::fs::create_dir_all(&directory) {
        eprintln!("funced: {}: {problem}", directory.display());
        return 1;
    }
    let file = directory.join(format!("{name}.sh"));
    match std::fs::write(&file, source) {
        Ok(()) => {
            println!("funced: saved {}", file.display());
            0
        }
        Err(problem) => {
            eprintln!("funced: {}: {problem}", file.display());
            1
        }
    }
}

/// Put `source` in front of an editor, and hand back what came out.
///
/// `None` when the editor failed or was killed — which is a cancel, not an empty function.
fn edit_in_editor(env: &Environment, name: &str, source: &str) -> Result<Option<String>> {
    let mut path = std::env::temp_dir();
    path.push(format!("oslo-funced-{name}-{}.sh", std::process::id()));
    if let Err(problem) = std::fs::write(&path, source) {
        eprintln!("funced: {}: {problem}", path.display());
        return Ok(None);
    }

    // A command line, not a path: `EDITOR="code --wait"` is ordinary, so it is split.
    let chosen = env
        .get_var("VISUAL")
        .or_else(|| env.get_var("EDITOR"))
        .unwrap_or("vi")
        .to_string();
    let mut words = chosen.split_whitespace();
    let Some(program) = words.next() else {
        eprintln!("funced: $EDITOR is empty");
        return Ok(None);
    };
    let ran = std::process::Command::new(program)
        .args(words)
        .arg(&path)
        .status();

    let edited = match ran {
        Ok(status) if status.success() => std::fs::read_to_string(&path).ok(),
        Ok(_) => {
            eprintln!("funced: the editor exited with an error; nothing was changed");
            None
        }
        Err(problem) => {
            eprintln!("funced: {program}: {problem}");
            None
        }
    };
    // Removed either way: it held a definition that is now either loaded or discarded.
    let _ = std::fs::remove_file(&path);
    Ok(edited)
}

/// `vared NAME` — edit a variable's value in oslo's own line editor.
///
/// **The line editor rather than `$EDITOR`**, because a value is one line: opening a whole editor
/// for it is the thing that makes people not bother, and oslo already owns a line editor that can
/// be handed a starting value. Cancelling leaves the variable exactly as it was.
pub fn builtin_vared(env: &mut Environment, args: &[String]) -> Result<i32> {
    let Some(name) = args.get(1) else {
        crate::env::complain(
            args,
            "vared",
            "vared: needs the name of a variable",
            "no variable named",
            Some("`vared PATH` opens its value for editing"),
        );
        return Ok(2);
    };
    if args.len() > 2 {
        let extra = &args[2];
        crate::env::complain(
            args,
            extra,
            &format!("vared: {extra}: takes one variable"),
            "nothing reads this",
            Some("`vared NAME`"),
        );
        return Ok(2);
    }

    let current = env.get_var(name).unwrap_or_default().to_string();
    let spec = oslo_ui::ask::Input {
        prompt: format!("{name}="),
        default: Some(current.clone()),
        ..Default::default()
    };
    match oslo_ui::ask::input(&spec) {
        oslo_ui::ask::Answer::Given(value) => {
            if value != current {
                env.set_var(name, &value, false);
            }
            Ok(0)
        }
        // Cancelled leaves it alone, which is the whole reason cancelling is distinct from an
        // empty answer: `vared PATH` escaped must not empty `$PATH`.
        oslo_ui::ask::Answer::Cancelled => Ok(1),
        oslo_ui::ask::Answer::NoTerminal => {
            eprintln!("vared: there is no terminal to edit on");
            Ok(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(f: fn(&mut Environment, &[String]) -> Result<i32>, args: &[&str]) -> i32 {
        let mut env = Environment::new();
        let argv: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
        f(&mut env, &argv).expect("must not unwind")
    }

    #[test]
    fn each_needs_the_thing_it_edits() {
        assert_eq!(run(builtin_funced, &["funced"]), 2);
        assert_eq!(run(builtin_vared, &["vared"]), 2);
    }

    /// One variable, so `vared A B` is a mistake rather than a silent edit of `A`.
    #[test]
    fn vared_takes_one_variable() {
        assert_eq!(run(builtin_vared, &["vared", "PATH", "HOME"]), 2);
    }

    /// An unknown option is refused rather than read as the name — `funced -x` must not open a
    /// function called `-x`.
    #[test]
    fn funced_refuses_an_option_it_does_not_have() {
        assert_eq!(run(builtin_funced, &["funced", "--nope", "greet"]), 2);
    }
}
