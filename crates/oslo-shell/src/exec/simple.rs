//! Evaluating a simple command.
//!
//! Expansion, alias substitution, command-prefix assignments, then command search: the order in
//! which a word becomes an alias, a function, a builtin or a program on PATH.
//!
//! The submodules are the parts that answer a question of their own: `external` runs a program
//! once it has been found, `assign` decides what each shape of assignment means, `trace` renders
//! `set -x`, `autocd` owns the `shopt -s autocd` option, and `posix` owns everything POSIX mode
//! changes about a command's *outcome*.

mod assign;
mod autocd;
pub(crate) mod autoload;
mod call;
mod declare;
/// `\command` and `\\command`, which decide how much of the shell a name skips past.
mod escape;
mod external;
/// bash's unforked last command of a `-c` program.
pub(crate) mod oneshot;
mod posix;
pub(crate) mod trace;

pub(crate) use assign::build_array;
pub use autocd::set_autocd;

use crate::env::Environment;
use crate::exec::redirect::RedirectGuard;
use crate::exec::simple::declare::{is_declaration_builtin, looks_like_an_assignment};
use crate::exec::simple::external::{Lookup, look_up_command, run_external};
use crate::expand::{expand_word, expand_word_to_string};
use oslo_base::ast::*;
use oslo_base::error::{Result, ShellError};
pub(crate) fn eval_simple_command(env: &mut Environment, simple: &SimpleCommand) -> Result<i32> {
    // Any `<(cmd)` in this command's words opens a pipe during expansion; it has to stay open
    // until the command has run and be closed afterwards, whichever way the command ends. The
    // wrapper is what guarantees the second half on the error paths too.
    let opened_before = env.procsubs.len();
    oneshot::begin();
    let result = eval_simple_command_inner(env, simple);
    oneshot::end();
    crate::exec::procsub::finish_from(env, opened_before);
    result
}

fn eval_simple_command_inner(env: &mut Environment, simple: &SimpleCommand) -> Result<i32> {
    // R6.5: a signal that arrived while the previous command ran is handled *between* commands,
    // where the trap body can run ordinary shell code. See `run_pending_traps`.
    crate::env::builtins::run_pending_traps(env)?;

    // `$BASH_COMMAND` names the command before the DEBUG trap fires for it, and stays put while
    // a trap runs.
    if !crate::env::builtins::in_trap() {
        env.set_bash_command(crate::env::builtins::control::format_simple(simple));
    }
    crate::env::builtins::run_debug_trap(env);

    if simple.words.is_empty() {
        return apply_assignments_only(env, simple);
    }
    // A command whose words all expand away reports its last substitution's status: `$(exit 3)`.
    env.take_substitution_status();

    // The command name decides how the rest of the words expand, so it is expanded first: a
    // declaration builtin's `name=value` operands are assignments and must not be field-split or
    // globbed. See `declare` for what that cost when they were.
    let mut words = Vec::new();
    let mut keyword_assignments = Vec::new();
    // `set +B` runs the words as they were written, braces and all.
    let written = match &simple.unbraced {
        Some(unbraced) if !env.braceexpand() => unbraced,
        _ => &simple.words,
    };
    let mut rest = written.iter();
    // Read from the word's *shape*, before expansion turns it into a string and the backslash
    // stops being visible. See `escape`, which is also where the interactive gate lives.
    let mut escape = escape::Escape::None;
    let mut backslashes = 0;
    // A lowered `[[ … ]]`, whose operands are never split: each is one string.
    let mut testing = false;
    if let Some(first) = rest.next() {
        (escape, backslashes) = escape::intent(first, env.interactive());
        testing = matches!(first.parts.as_slice(), [WordPart::Literal(w)] if w == "[[");
        words.extend(crate::expand::expand_command_word(env, first)?);
    }

    if words.is_empty() {
        // Nothing to name a declaration builtin, so the rest are ordinary words.
        for w in rest {
            words.extend(expand_word(env, w)?);
        }
        if words.is_empty() {
            let expanded = env.take_substitution_status();
            let status = apply_assignments_only(env, simple)?;
            let assigned = env.take_substitution_status();
            return Ok(match status {
                0 => assigned.or(expanded).unwrap_or(0),
                failed => failed,
            });
        }
    } else {
        let declaring = is_declaration_builtin(words[0].trim());
        // `set -k`: an assignment anywhere among the arguments belongs to the command's environment.
        let keyword = env.option(crate::env::options::ShellOption::Keyword);
        for w in rest {
            if testing {
                words.push(crate::expand::word::expand_word_to_test_string(env, w)?);
            } else if declaring && looks_like_an_assignment(w) {
                words.push(crate::expand::expand_word_to_string(env, w)?);
            } else if keyword && looks_like_an_assignment(w) {
                let text = crate::expand::expand_word_to_string(env, w)?;
                if let Some((name, value)) = text.split_once('=') {
                    keyword_assignments.push((name.to_string(), value.to_string()));
                }
            } else {
                words.extend(expand_word(env, w)?);
            }
        }
    }

    // Aliases are *not* expanded here. Substitution happens on the source text before it is
    // parsed — see [`crate::syntax::alias`] — because an alias body is source, not a list of
    // arguments, and only a pre-parse pass can let `alias forever='while :; do'` work. Doing it
    // in both places would also expand twice: `alias ls='ls -F'` would have become `ls -F -F`.

    // `\\rm` is the word `\rm` by the time it is a string — the lexer turned the pair into one
    // literal backslash. It comes off here, so that argv[0] and everything downstream see the name
    // the user meant rather than one no `$PATH` entry will ever match.
    let cmd_name = words[0].trim()[backslashes..].to_string();
    words[0] = cmd_name.clone();

    let is_declaration = is_declaration_builtin(&cmd_name);

    // A prefix assignment on a *declaration* builtin is really that builtin's argument:
    // `export FOO=bar` must reach `export`, not be applied behind its back.
    let mut prefix_assignments = Vec::new();
    for assign in &simple.assignments {
        // A prefix assignment lasts exactly as long as the command, and oslo undoes it by
        // restoring a *scalar* from the scope frame. `a=(1 2) cmd` and `a[1]=x cmd` have no such
        // undo, so they are refused rather than left behind after the command finishes.
        let (AssignmentTarget::Name(name), AssignmentValue::Scalar(value)) =
            (&assign.target, &assign.value)
        else {
            return Err(ShellError::SyntaxError(format!(
                "{}: an array assignment cannot be a command prefix",
                assign.name()
            )));
        };
        let val_str = expand_word_to_string(env, value)?;
        if is_declaration {
            words.push(format!("{}={}", name, val_str));
        } else {
            prefix_assignments.push((name.clone(), val_str));
        }
    }
    prefix_assignments.extend(keyword_assignments);

    let traced: Vec<String> = prefix_assignments
        .iter()
        .map(|(name, value)| trace::assignment_text(name, false, value))
        .collect();
    trace::trace_command(env, &traced, &words);

    // `FOO=bar cmd` exports FOO for the duration of `cmd` only.
    //
    // The scope is pushed only when there is something to put in it. Pushing unconditionally
    // would give `local` a throwaway frame to write into, so `local V=x` would be undone the
    // moment the command finished.
    if prefix_assignments.is_empty() {
        let result = run_command_word(env, &cmd_name, &words, &simple.redirections, escape);
        // A `[[ … ]]` leaves `$_` as it was.
        if !testing {
            remember_last_argument(env, &words);
        }
        return result;
    }

    // **A here-document is expanded without the prefix assignments in scope**, which is what bash
    // and dash both do:
    //
    // ```text
    // x=1 cat <<EOF     bash, dash: []
    // [$x]              oslo, before this: [1]
    // EOF
    // ```
    //
    // The body used to be expanded down in `redirect::apply`, which runs after the scope below is
    // pushed — so the document saw a variable that belongs to the command's environment and not to
    // the shell's. Expanded here instead, and carried down as a literal: the parser already uses
    // that shape for a quoted delimiter, so nothing expands it a second time.
    let redirections = heredocs_expanded(env, &simple.redirections)?;

    env.push_scope();
    for (name, value) in &prefix_assignments {
        env.set_local_exported_var(name, value);
    }
    let result = run_command_word(env, &cmd_name, &words, &redirections, escape);
    env.pop_scope();
    remember_last_argument(env, &words);
    result
}

/// `$_` — the last argument of the command that just ran, expanded.
///
/// **`mkdir -p a/b && cd $_` is the idiom this exists for**, and without it `$_` expanded to
/// nothing, `cd` was called with no argument, and the shell went to `$HOME` instead. A wrong
/// directory reported as success is the worst shape a bug can take: the commands after it run, on
/// the wrong files.
///
/// Set after the command rather than before it, which is the same thing seen from the next command
/// and is where the expanded words are. Written only for a command that had words — bash leaves
/// `$_` alone for an assignment-only line, and a pipeline's components run in their own shells, so
/// neither reaches here.
///
/// Not exported. bash does put a `_` in a child's environment, but that one holds the path of the
/// command being run rather than this value, and inventing an export that disagrees with bash's
/// would be worse than leaving the child alone.
fn remember_last_argument(env: &mut Environment, words: &[String]) {
    if let Some(last) = words.last() {
        env.set_var("_", last, false);
    }
}

/// The redirections with every here-document body already expanded.
///
/// Only the bodies: a redirection *target* is expanded by `redirect::apply` at the moment it is
/// opened, and bash expands that one with the assignments in scope — `x=/tmp/f x=1 cat > $x` is
/// not a case anybody writes, but the two halves genuinely differ and following bash on each is
/// cheaper than explaining a rule of our own.
fn heredocs_expanded(
    env: &mut Environment,
    redirections: &[Redirection],
) -> Result<Vec<Redirection>> {
    let mut out = Vec::with_capacity(redirections.len());
    for redir in redirections {
        let mut redir = redir.clone();
        if matches!(
            redir.kind,
            RedirectKind::Heredoc | RedirectKind::HeredocStrip
        ) && let Some(word) = &redir.heredoc_content
        {
            let text = match redir.here_string {
                true => expand_word_to_string(env, word)?,
                false => crate::expand::join::expand_heredoc_to_string(env, word)?,
            };
            redir.heredoc_content = Some(Word::from_literal(&text));
        }
        out.push(redir);
    }
    Ok(out)
}

/// Run a command that has no command word: `x=1`, or `x=1 $empty`.
///
/// Shared by the two paths that reach it — assignments written with no word at all, and
/// assignments whose word expanded to nothing — because they have to agree about the two things
/// that are easy to get subtly different: the assignment is still performed, and `set -x` still
/// traces it.
///
/// An assignment that the environment *refused* — a read-only name, or one `environ` cannot hold
/// — is a failed command. This used to answer `Ok(0)` regardless: `set_var` printed
/// `r: is read only`, returned `false`, and the `false` was dropped on the floor, so `r=2; echo
/// $?` said the assignment had worked (PLAN C3).
fn apply_assignments_only(env: &mut Environment, simple: &SimpleCommand) -> Result<i32> {
    let mut refused: Option<String> = None;
    for assign in &simple.assignments {
        // Applied before the next one is expanded, not batched at the end: POSIX 2.9.1 evaluates
        // assignments left to right and makes each visible to the next, so `a=1 b=${a}2` sets
        // `b` to `12`. What each shape means is `assign`'s business.
        // Each is traced as it is made, as bash does; see `assign`.
        let outcome = assign::apply_assignment(env, assign)?;
        if !outcome.assigned && refused.is_none() {
            refused = Some(assign.name().to_string());
        }
    }
    // The first name to fail is the one reported, as bash reports the first.
    if let Some(name) = refused {
        return posix::assignment_failure(env, &name);
    }
    // **A line with no command clears `$_` rather than leaving it.** Checked against bash 5.3:
    // `true kept; x=1; echo "[$_]"` prints `[]`, and `${_-UNSET}` prints empty rather than `UNSET`
    // — so it is set to nothing, not unset. `export z=1` is a command with a word and keeps the
    // ordinary rule, which is why it answers `z=1`.
    env.set_var("_", "", false);
    Ok(apply_wordless_redirections(env, &simple.redirections))
}

/// Run an argv that is already expanded, as Lua's `oslo.run` hands it over.
///
/// This is the whole point of the argv call model: no word, no quoting, no glob and no field
/// splitting stand between the caller's list and the command. `oslo.run{"rm", name}` cannot
/// misbehave for a `name` with a space or a `*` in it, where `oslo.proc.exec("rm " .. name)` can.
///
/// Everything past this point is shared with the shell — the same command search, the same
/// builtins, the same functions — so `sh.cd("/tmp")` moves the shell exactly as `cd /tmp` does.
pub fn run_argv(env: &mut Environment, argv: &[String]) -> Result<i32> {
    let Some(name) = argv.first() else {
        return Ok(0);
    };
    crate::env::builtins::run_pending_traps(env)?;
    // No escape: the argv model has no source text and therefore no backslash to have written.
    // `oslo.run{"\\rm"}` asks for a command literally named `\rm`, which is the whole promise of
    // that call model — the list you write is the list that runs.
    run_command_word(env, name, argv, &[], escape::Escape::None)
}

/// Dispatch an already-expanded command word.
///
/// POSIX 2.9.1.1 command search, and the order matters at every step:
///
/// 1. **alias** — done by the caller, before the word is even split.
/// 2. **special builtin**, in POSIX mode only. POSIX puts `export`, `eval`, `set`, `.` and the
///    rest ahead of functions; bash follows that only under `--posix`, where it goes further and
///    refuses to *define* such a function at all.
/// 3. **function**. This is the step oslo skipped: `is_builtin` was consulted first, so
///    `echo() { … }`, `cd() { … }` and `test() { … }` could be defined but never called, and
///    `type echo` insisted it was a builtin. Wrapping a builtin is how a shell script overrides
///    behaviour it does not control, and silently ignoring the wrapper runs the original.
/// 4. **regular builtin**.
/// 5. **PATH**, or a path operand — see the `external` submodule.
///
/// `escape` is how much of that a leading backslash asks to skip past — nothing at all for a
/// script, which is where the steps above are POSIX's and stay POSIX's. See the `escape` module.
fn run_command_word(
    env: &mut Environment,
    cmd_name: &str,
    words: &[String],
    redirections: &[Redirection],
    escape: escape::Escape,
) -> Result<i32> {
    let name = cmd_name.trim();

    // Ahead of the POSIX order rather than woven into it, because that is what the two forms mean:
    // they name a step to start *from*, so the steps before it are not consulted at all. Folding
    // the condition into each `if` below would read as three unrelated exceptions.
    if escape.skips_function() {
        return run_program(env, name, words, redirections, escape);
    }

    if !escape.skips_builtin()
        && posix::special_builtins_outrank_functions(env)
        && crate::env::scope::is_special_builtin(name)
        && env.is_builtin(name)
    {
        return run_builtin(env, name, words, redirections);
    }

    if let Some(func_body) = env.shared_function(name) {
        return call::call_function_command(env, &func_body, words, redirections);
    }

    if !escape.skips_builtin() && env.is_builtin(name) {
        return run_builtin(env, name, words, redirections);
    }

    run_program(env, name, words, redirections, escape)
}

/// Apply the command's redirections and run it as a builtin.
///
/// A builtin never sees its own redirections, so the one thing decided here is how long they
/// last. `exec > "$log" 2>&1` — `exec` with no command word — is the form POSIX says applies to
/// the shell itself from then on, so its guard must not restore anything; every other builtin
/// gets the ordinary guard that puts the descriptors back when the command ends.
///
/// Both outcomes are then passed through `posix`, which is where a *special* builtin's utility
/// error or redirection error becomes the end of a POSIX-mode shell.
fn run_builtin(
    env: &mut Environment,
    name: &str,
    words: &[String],
    redirections: &[Redirection],
) -> Result<i32> {
    let mut guard = if crate::env::builtins::exec_makes_redirections_permanent(name, words) {
        RedirectGuard::for_exec()
    } else {
        RedirectGuard::new()
    };
    if let Err(e) = guard.apply(env, redirections) {
        return posix::redirect_failure(env, name, report_redirect_failure(&env.origin(), &e));
    }
    let result = execute_builtin(env, name, words);
    posix::resolve_builtin_result(env, name, result)
}

/// Run an external program, or explain why the word does not name one.
///
/// The statuses are the ones a caller reads: 127 for "no such command", 126 for "found it,
/// cannot run it". oslo used to report 127 for a non-executable file and, for a directory,
/// nothing at all — it changed directory and returned 0 (PLAN R5.13).
fn run_program(
    env: &mut Environment,
    cmd_name: &str,
    words: &[String],
    redirections: &[Redirection],
    escape: escape::Escape,
) -> Result<i32> {
    match look_up_command(cmd_name) {
        Lookup::Program(path) => run_external(env, &path, cmd_name, words, redirections),
        Lookup::Directory => match autocd::try_autocd(env, cmd_name, words) {
            Some(result) => result,
            None => report_unrunnable(env, redirections, cmd_name, words, "Is a directory", 126),
        },
        Lookup::NotExecutable => {
            report_unrunnable(env, redirections, cmd_name, words, "Permission denied", 126)
        }
        // A bare word is a PATH search, so a directory of that name in the *current* directory
        // was never a candidate; it only becomes one if autocd is on. bash reports the same
        // "command not found" and 127 here even with `shopt -s autocd` set, as long as the shell
        // is not interactive.
        Lookup::NotFound => match autocd::try_autocd(env, cmd_name, words) {
            Some(result) => result,
            // Nothing on `$PATH` and not a directory — so a function kept in its own file may
            // still answer for this name. Read *after* the search rather than before it, which is
            // what stops a file on disk from quietly redefining a command that already works.
            // **Not for `\cmd`**, which asks for the program and nothing else. `autoload::load`
            // answers "already defined" for a function the shell has in hand — that is its
            // recursion guard — so leaving this in the path put the function back in the running
            // after the step above had taken it out, and `\cd` went on calling `cd() { … }`.
            // **Autoload is skipped for `\cmd`**, which asks for the program and nothing else.
            // `autoload::load` answers "already defined" for a function the shell has in hand —
            // that is its recursion guard — so leaving it in the path put the function back in the
            // running after the step above had taken it out, and `\cd` went on calling
            // `cd() { … }` with no sign that the backslash had done anything.
            None if escape.skips_function() => nothing_to_run(env, cmd_name, words, redirections),
            None => match autoload::try_call(env, cmd_name, words, redirections) {
                Some(result) => result,
                // And then the database, on the same terms and for the same reason: after `$PATH`
                // has already failed, so a stored row can add a name but never take one over. It
                // is last because a file you can see beats a row you cannot.
                None => match super::stored::try_call(env, cmd_name, words, redirections) {
                    Some(result) => result,
                    None => nothing_to_run(env, cmd_name, words, redirections),
                },
            },
        },
    }
}

/// The end of the command search, and how it is reported.
mod notfound;
use notfound::{nothing_to_run, report_unrunnable};

/// Report a redirection failure and hand back the status the failed command takes on.
///
/// A redirection that cannot be set up fails *the command*, not the shell. oslo used to propagate
/// the error to `main`, which exited — so `echo hi < /nonexistent; echo CONTINUE` never printed
/// CONTINUE, while the same redirection on an external command continued happily. The two paths
/// disagreed with each other; this is the one place that decides.
///
/// Status 1, measured against `bash --posix` for a builtin (`read x < /nonexistent`), a bad
/// descriptor (`echo hi >&7`), a function, a compound and an external command: all print a
/// diagnostic, set `$?` to 1 and carry on.
///
/// The one case bash treats differently is a redirection error on a *special* builtin (`:`,
/// `export`, …) in POSIX mode, which does abort the shell. That is now implemented, but not
/// here: this function only reports and scores the failure, and [`posix::redirect_failure`]
/// decides whether the shell survives it. Callers on a path that cannot be a special builtin —
/// a function, a compound, an external command, a command word with no builtin behind it — use
/// this answer directly, because for them the question never arises.
pub(crate) fn report_redirect_failure(origin: &str, err: &ShellError) -> i32 {
    eprintln!("{origin}{err}");
    1
}

/// Apply the redirections of a command that has no command word.
///
/// `> out` on its own still creates `out`, and `x=1 < /nonexistent` still fails with status 1
/// after performing the assignment. The guard is dropped immediately, which restores the saved
/// descriptors — the redirection's only lasting effect is on the filesystem.
fn apply_wordless_redirections(env: &mut Environment, redirections: &[Redirection]) -> i32 {
    if redirections.is_empty() {
        return 0;
    }
    let mut guard = RedirectGuard::new();
    match guard.apply(env, redirections) {
        Ok(()) => 0,
        Err(e) => report_redirect_failure(&env.origin(), &e),
    }
}

/// Run `cmd_name` as a builtin, assuming redirections are already in place.
///
/// The only dispatcher, and it owns no list of its own: it asks the registry. The `match` that
/// used to be here named 30 builtins and their functions a second time, so the registry could
/// hold a *different* implementation for a name and never be consulted — which is exactly what
/// made `oslo.register_builtin('echo', …)` do nothing (PLAN R5.6, R9.8) — while the `_` arm
/// answered `Ok(0)` for anything unlisted, turning a name that was a builtin only according to
/// `is_builtin` into a command that silently succeeded without doing anything.
///
/// Public to the crate so `command` and `builtin` (PLAN R5.7) can reach a builtin without
/// re-entering command search.
pub(crate) fn execute_builtin(
    env: &mut Environment,
    cmd_name: &str,
    words: &[String],
) -> Result<i32> {
    match env.exec_custom_builtin(cmd_name, words) {
        Some(result) => result,
        // Only reachable from a caller that dispatched without asking `is_builtin` first, which
        // is a bug here rather than in the user's script — but it is still the user who gets the
        // message, so it says what a shell says about a name it cannot run.
        None => {
            eprintln!("oslo: {}: not a shell builtin", cmd_name);
            Ok(127)
        }
    }
}

#[cfg(test)]
#[path = "simple/tests.rs"]
mod tests;
