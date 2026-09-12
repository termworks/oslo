//! Evaluating compound commands: `if`, `while`, `until`, `for`, `case`, groups, subshells.
//!
//! `break` and `continue` arrive here as errors unwinding from the loop body; each loop peels
//! one level off the requested depth and either stops or re-raises.
//!
//! Conditions are run through `eval_condition`, never through `eval_command_list` directly:
//! POSIX exempts the condition of an `if`/`elif`/`while`/`until` from `set -e`, and a condition
//! that aborted the shell when it answered "no" would make `set -e` unusable.

use crate::env::Environment;
use crate::env::builtins::run_exit_trap;
use crate::exec::pipeline::{eval_command_list, status_of, suspend_errexit, wait_for_status};
use crate::expand::word::expand_word_to_pattern;
use crate::expand::{expand_word, expand_word_to_string};
use nix::unistd::{ForkResult, fork};
use oslo_base::ast::*;
use oslo_base::error::{Result, ShellError};

/// Push whatever this shell has buffered out to fd 1.
///
/// Called on both sides of a subshell fork: before, so the parent's buffer is not copied into the
/// child and printed twice; in the child before `process::exit`, which skips every destructor.
pub(crate) fn flush_stdout() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

/// What one iteration of a loop body decided.
enum LoopStep {
    /// Carry on with the next iteration.
    Next,
    /// Leave this loop, normally.
    Stop,
    /// Something must escape this loop: an outer `break`/`continue`, a `return`, or an error.
    Unwind(ShellError),
}

/// Run a loop body once and classify the outcome.
///
/// `break n` / `continue n` for `n > 1` are re-raised with the depth decremented, so each
/// enclosing loop peels off one level.
///
/// A `break` or `continue` that this loop consumes also *sets* the running status to 0, because
/// it is itself the last command the body executed and it succeeded. Leaving the previous
/// iteration's status in place made `for ((i=0; ; i++)); do echo $i; ((i>=2)) && break; done`
/// report 1 — the status of the `&&` test that was false last time round.
fn run_loop_body(env: &mut Environment, body: &CommandList, status: &mut i32) -> LoopStep {
    match eval_command_list(env, body) {
        Ok(st) => {
            *status = st;
            LoopStep::Next
        }
        Err(e) => classify_unwind(e, status),
    }
}

/// Decide what an error escaping a loop's body *or* its condition means to this loop.
///
/// `break n` / `continue n` for `n > 1` are re-raised with the depth decremented, so each
/// enclosing loop peels off one level. Anything else escapes untouched.
fn classify_unwind(error: ShellError, status: &mut i32) -> LoopStep {
    match error {
        ShellError::Break(depth) if depth > 1 => LoopStep::Unwind(ShellError::Break(depth - 1)),
        ShellError::Break(_) => {
            *status = 0;
            LoopStep::Stop
        }
        ShellError::Continue(depth) if depth > 1 => {
            LoopStep::Unwind(ShellError::Continue(depth - 1))
        }
        ShellError::Continue(_) => {
            *status = 0;
            LoopStep::Next
        }
        e => LoopStep::Unwind(e),
    }
}

/// Evaluate a construct's condition, with `set -e` suspended for its whole extent.
///
/// R6.2: POSIX 2.9.1 exempts "the compound list following the `while`, `until`, `if` or `elif`
/// reserved word" from errexit, and the exemption is dynamic — it covers functions and subshells
/// the condition calls, so `set -e; f() { false; echo reached; }; if f; then` prints `reached`.
/// Asking the question is not the same as failing at it.
fn eval_condition(env: &mut Environment, condition: &CommandList) -> Result<i32> {
    let _exempt = suspend_errexit();
    eval_command_list(env, condition)
}

/// Shared driver for `while` and `until`, which differ only in how they read the condition.
///
/// The loop counter is entered and exited around the whole construct rather than around the body
/// so that it stays balanced on every exit path, including an error escaping the condition.
fn eval_conditional_loop(
    env: &mut Environment,
    condition: &CommandList,
    body: &CommandList,
    run_while_zero: bool,
) -> Result<i32> {
    let mut status = 0;
    env.enter_loop();

    let result = loop {
        // A `break` in the *condition* belongs to this loop just as much as one in the body.
        // POSIX puts the condition inside the loop, and the idiom it enables is real: modernish
        // writes its option parsers as `while case ${1-} in (-x) ...;; (*) break;; esac; do shift;
        // done`. Letting the error escape here unwound past the loop and out of the enclosing
        // function, so every statement after such a loop was silently skipped.
        let cond = match eval_condition(env, condition) {
            Ok(c) => c,
            Err(e) => match classify_unwind(e, &mut status) {
                // `while continue` re-asks the question, which is what bash does — and loops
                // forever if the answer never changes.
                LoopStep::Next => continue,
                LoopStep::Stop => break Ok(status),
                LoopStep::Unwind(e) => break Err(e),
            },
        };
        if (cond == 0) != run_while_zero {
            break Ok(status);
        }

        match run_loop_body(env, body, &mut status) {
            LoopStep::Next => {}
            LoopStep::Stop => break Ok(status),
            LoopStep::Unwind(e) => break Err(e),
        }
    };

    env.exit_loop();
    result
}

/// Does `subject` match any of `patterns`?
///
/// Neither the subject nor the patterns are field-split or pathname-expanded; `expand_word` would
/// glob `f*` against the working directory instead of leaving it as a pattern to match against.
///
/// The pattern keeps its quoting all the way to the matcher. Flattening it to a string first made
/// `case $x in "$expected")` — an ordinary way to compare two strings — a pattern match, so a
/// `$expected` of `*` answered yes for every subject.
fn any_pattern_matches(env: &mut Environment, patterns: &[Word], subject: &str) -> Result<bool> {
    for pat_word in patterns {
        let runs = expand_word_to_pattern(env, pat_word)?;
        crate::expand::glob::refuse_extglob(&runs)?;
        let nocase = oslo_base::glob::walk::nocasematch();
        if crate::expand::glob::pattern_from_runs(&runs).matches_case(subject, nocase) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `case … esac`, including bash's two fallthrough terminators.
///
/// The branches are walked by index rather than with a `for`, because `;&` has to reach the
/// *next* branch and run its body without consulting its patterns — `forced` is that carry. `;;&`
/// leaves `forced` clear and simply keeps testing, which is the "re-test" half of the feature.
///
/// The status is that of the last body actually run, so a `;;&` chain whose later patterns all
/// fail still reports what the branch that did match returned.
fn eval_case(env: &mut Environment, word: &Word, items: &[CaseItem]) -> Result<i32> {
    let subject = expand_word_to_string(env, word)?;

    let mut status = 0;
    let mut forced = false;

    for item in items {
        if !forced && !any_pattern_matches(env, &item.patterns, &subject)? {
            continue;
        }

        status = eval_command_list(env, &item.body)?;
        match item.post_action {
            CaseAction::ExitCase => return Ok(status),
            CaseAction::FallThrough => forced = true,
            CaseAction::ContinueMatching => forced = false,
        }
    }

    Ok(status)
}

/// Evaluate one arithmetic expression for a command-level construct.
///
/// A bad expression is the *command's* failure, not the shell's: bash prints the diagnostic,
/// leaves `$?` at 1 and runs the next command, unlike `$(( … ))` in a word, which is fatal.
/// `None` means "already reported"; callers turn it into status 1 and stop.
fn eval_arith(env: &mut Environment, expr: &str) -> Option<i64> {
    match crate::expand::arithmetic::eval_arithmetic(env, expr) {
        Ok(value) => Some(value),
        Err(e) => {
            eprintln!("oslo: ((: {}", e);
            None
        }
    }
}

/// `(( expr ))` — evaluated in the *current* shell, so its assignments survive.
///
/// The status is inverted with respect to the value: a non-zero result is success. That is what
/// makes `while (( i < n ))` and `if (( x ))` read the way arithmetic conditions are written, and
/// it is the same convention the `let` builtin uses.
fn eval_arithmetic_command(env: &mut Environment, expr: &str) -> i32 {
    match eval_arith(env, expr) {
        Some(value) => i32::from(value == 0),
        None => 1,
    }
}

/// `for ((init; cond; step)) do … done`.
///
/// Every section is optional, and an absent condition means *true* — `for ((;;))` is the
/// idiomatic infinite loop, not a loop that never runs. The step runs after `continue` as well as
/// after a normal iteration, which is what keeps `continue` from wedging a counting loop.
fn eval_arithmetic_for(
    env: &mut Environment,
    init: Option<&str>,
    cond: Option<&str>,
    step: Option<&str>,
    body: &CommandList,
) -> Result<i32> {
    if let Some(expr) = init
        && eval_arith(env, expr).is_none()
    {
        return Ok(1);
    }

    let mut status = 0;
    env.enter_loop();

    let result = loop {
        // An absent condition is true, which is why this is not `unwrap_or(1)` on a value: there
        // is nothing to evaluate, so nothing can fail either.
        if let Some(expr) = cond {
            match eval_arith(env, expr) {
                Some(0) => break Ok(status),
                Some(_) => {}
                None => break Ok(1),
            }
        }

        match run_loop_body(env, body, &mut status) {
            LoopStep::Next => {}
            LoopStep::Stop => break Ok(status),
            LoopStep::Unwind(e) => break Err(e),
        }

        if let Some(expr) = step
            && eval_arith(env, expr).is_none()
        {
            break Ok(1);
        }
    };

    env.exit_loop();
    result
}

pub(crate) fn eval_compound_command(
    env: &mut Environment,
    compound: &CompoundCommand,
) -> Result<i32> {
    match compound {
        CompoundCommand::If {
            condition,
            then_branch,
            elif_branches,
            else_branch,
        } => {
            let cond_status = eval_condition(env, condition)?;
            if cond_status == 0 {
                return eval_command_list(env, then_branch);
            }

            for (elif_cond, elif_body) in elif_branches {
                let elif_status = eval_condition(env, elif_cond)?;
                if elif_status == 0 {
                    return eval_command_list(env, elif_body);
                }
            }

            if let Some(else_b) = else_branch {
                eval_command_list(env, else_b)
            } else {
                Ok(0)
            }
        }
        CompoundCommand::While { condition, body } => {
            eval_conditional_loop(env, condition, body, true)
        }
        CompoundCommand::Until { condition, body } => {
            eval_conditional_loop(env, condition, body, false)
        }
        CompoundCommand::For {
            var_name,
            items,
            body,
        } => {
            let item_words = if let Some(it) = items {
                let mut vec = Vec::new();
                for w in it {
                    vec.extend(expand_word(env, w)?);
                }
                vec
            } else {
                env.get_positional().to_vec()
            };

            let mut status = 0;
            env.enter_loop();
            let result = 'items: {
                for item in item_words {
                    env.set_var(var_name, &item, false);
                    match run_loop_body(env, body, &mut status) {
                        LoopStep::Next => {}
                        LoopStep::Stop => break 'items Ok(status),
                        LoopStep::Unwind(e) => break 'items Err(e),
                    }
                }
                Ok(status)
            };
            env.exit_loop();
            result
        }
        CompoundCommand::Case { word, items } => eval_case(env, word, items),
        CompoundCommand::Arithmetic(expr) => Ok(eval_arithmetic_command(env, expr)),
        CompoundCommand::ArithmeticFor {
            init,
            cond,
            step,
            body,
        } => eval_arithmetic_for(env, init.as_deref(), cond.as_deref(), step.as_deref(), body),
        CompoundCommand::Subshell(body) => {
            // Anything this shell has buffered would be duplicated by the fork and printed twice.
            flush_stdout();
            unsafe {
                match fork() {
                    Ok(ForkResult::Child) => {
                        // R4.7: a subshell is a process that runs commands, so it starts from the
                        // signal state a program is entitled to — in particular SIGPIPE at
                        // `SIG_DFL`, so `( while :; do echo x; done ) | head -1` dies on the
                        // closed pipe instead of spinning on EPIPE.
                        crate::exec::job::reset_signals_for_child();
                        // The child keeps the environment `fork` just copied: functions,
                        // aliases, positionals, `$?` and export flags all survive, and nothing
                        // private is force-exported. Only subshell-local state is refreshed.
                        env.enter_subshell();
                        // `status_of`, not `unwrap_or(1)`: `( exit 3 )` unwinds as an error
                        // carrying its code, and the exit status is the only channel left here.
                        let res = status_of(eval_command_list(env, body));
                        // `process::exit` runs no destructors, so a partial line written by
                        // `echo -n` would die in the buffer instead of reaching the parent.
                        flush_stdout();
                        // A subshell is a shell, so a trap it set for its own exit has to run before it goes.
                        // `enter_subshell` cleared whatever the parent had, so this only ever fires
                        // a handler installed *inside* here — which is why `trap T EXIT; (:)`
                        // still prints once and not twice.
                        std::process::exit(run_exit_trap(env, res));
                    }
                    Ok(ForkResult::Parent { child }) => Ok(wait_for_status(child)),
                    Err(e) => Err(ShellError::ExecutionError(format!(
                        "Subshell fork failed: {}",
                        e
                    ))),
                }
            }
        }
        CompoundCommand::Group(body) => eval_command_list(env, body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse_bash_script;

    /// Run `src` in one environment and hand back the status, so a test can then ask what the
    /// script left behind. In-process on purpose: the point of `(( ))` is that its side effects
    /// land in *this* shell, and a forked child would hide a regression.
    fn run(env: &mut Environment, src: &str) -> i32 {
        let list = parse_bash_script(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        eval_command_list(env, &list).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    fn status(src: &str) -> i32 {
        run(&mut Environment::new(), src)
    }

    /// Non-zero is success. Getting this backwards makes every `if (( ))` take the wrong branch.
    #[test]
    fn the_arithmetic_command_status_is_inverted() {
        assert_eq!(status("((1))"), 0);
        assert_eq!(status("((0))"), 1);
        assert_eq!(status("((5 > 3))"), 0);
        assert_eq!(status("((3 > 5))"), 1);
        assert_eq!(status("((-1))"), 0);
    }

    /// The whole reason the construct exists: no fork, so the assignment is still there after.
    #[test]
    fn arithmetic_assignments_stay_in_the_current_environment() {
        let mut env = Environment::new();
        env.set_var("x", "5", false);
        assert_eq!(run(&mut env, "((x++))"), 0);
        assert_eq!(env.get_var("x"), Some("6"));
        assert_eq!(run(&mut env, "((x *= 3))"), 0);
        assert_eq!(env.get_var("x"), Some("18"));
    }

    /// A bad expression is the command's failure, not the shell's, so the next command runs.
    #[test]
    fn a_bad_expression_fails_the_command_only() {
        let mut env = Environment::new();
        assert_eq!(run(&mut env, "((1 +)); ((marker = 9))"), 0);
        assert_eq!(env.get_var("marker"), Some("9"));
    }

    /// An absent condition is *true*; reading it as the empty expression would make the loop a
    /// no-op instead of the idiomatic infinite loop.
    #[test]
    fn an_arithmetic_for_loop_with_no_condition_runs_until_it_breaks() {
        let mut env = Environment::new();
        run(
            &mut env,
            "for (( ; ; )); do ((n++)); ((n >= 4)) && break; done",
        );
        assert_eq!(env.get_var("n"), Some("4"));
    }

    #[test]
    fn an_arithmetic_for_loop_steps_after_continue() {
        let mut env = Environment::new();
        run(
            &mut env,
            "for ((i = 0; i < 4; i++)); do ((i == 1)) && continue; ((seen++)); done",
        );
        assert_eq!(
            env.get_var("i"),
            Some("4"),
            "the step must run after continue"
        );
        assert_eq!(env.get_var("seen"), Some("3"));
    }

    /// `;&` reaches the next branch without testing its pattern; `;;&` keeps testing.
    #[test]
    fn case_terminators_select_different_branches() {
        let mut env = Environment::new();
        run(
            &mut env,
            "case a in a) hit=1;& zzz) fell=1;; esac; case abc in a*) first=1;;& *c) second=1;; esac",
        );
        assert_eq!(env.get_var("hit"), Some("1"));
        assert_eq!(env.get_var("fell"), Some("1"), ";& must ignore the pattern");
        assert_eq!(env.get_var("first"), Some("1"));
        assert_eq!(env.get_var("second"), Some("1"), ";;& must keep matching");
    }

    /// A `;;&` chain reports the last body that actually ran, not the failed match after it.
    #[test]
    fn a_case_reports_the_status_of_the_last_body_it_ran() {
        assert_eq!(status("case a in a) false;;& zzz) true;; esac"), 1);
        assert_eq!(status("case a in a) true;;& zzz) false;; esac"), 0);
        assert_eq!(status("case a in zzz) false;; esac"), 0);
    }
}
