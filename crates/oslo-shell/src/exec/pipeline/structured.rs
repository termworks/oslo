//! Choosing between the byte pipeline and the structured one.
//!
//! Every pipeline oslo has ever run goes through the byte path: one process per stage, bytes on
//! every descriptor. This module answers one question before that happens — *does any edge of this
//! pipeline carry structured rows instead* — and it answers "no" for every pipeline written before
//! oslo invented a vocabulary for saying otherwise.
//!
//! Kept apart from the pipeline itself so the byte path stays exactly as long and exactly as
//! readable as it was, and so the seam between the two is one function rather than a condition
//! threaded through the old code. See `docs/features/structured-pipelines.md`.

/// Reading the byte upstream, and the bound on how much of it is held.
mod capture;
mod handover;
/// Refusing a named column the stream cannot have, at plan time.
mod refusal;
mod stream;
use capture::{Upstream, capture, read_standard_input, too_large};
use handover::{Printed, byte_suffix_at, hand_over, runnable_here};
pub(super) use refusal::refuse_redirected_middle;
use refusal::refuse_unknown_column;

use super::Pipeline;
use crate::data::{Sink, Stage};
use crate::env::Environment;
use oslo_base::ast::types::{Command, SimpleCommand, WordPart};
use oslo_base::error::Result;

/// The command name of a simple command, when it is the same whatever the environment holds.
///
/// A name that comes out of an expansion — `$cmd foo` — is not known until the command runs, and a
/// planner that guessed could send structure to something that turned out to be an external.
/// Unknown means bytes, which is always safe.
///
/// **But quoting is not expansion**, and this used to accept a bare literal only. That mattered more
/// than it sounds: quoting a word is the POSIX way to stop an alias expanding, so somebody whose own
/// `alias lines=tokei` shadows the verb reaches for `\lines` — and got `lines: command not found`,
/// because the escape that recovered the name from the alias then hid it from the planner. The one
/// escape hatch for a vocabulary collision did not work.
///
/// A shell function of that name answers `None`: the function runs, as bash runs it.
fn simple_command_name(env: &Environment, simple: &SimpleCommand) -> Option<String> {
    let word = simple.words.first()?;
    let name: String = word
        .parts
        .iter()
        .map(refusal::constant)
        .collect::<Option<_>>()?;
    (!name.is_empty() && env.get_function(&name).is_none()).then_some(name)
}

/// Whether a redirection points **this stage's stdout** somewhere else.
///
/// The fd it governs when none is written down is the one its direction implies: `>` and `>>` are
/// stdout, `<` and a here-document are stdin. `2>&1` names fd 2 and leaves fd 1 where it was, so it
/// does not count either — what matters is only whether the rows this stage produces would still
/// reach the next one.
fn redirects_stdout(redirection: &oslo_base::ast::types::Redirection) -> bool {
    use oslo_base::ast::types::RedirectKind::*;
    let implied = match redirection.kind {
        Output | Append | Clobber | DupOutput => 1,
        Input | ReadWrite | DupInput | Heredoc | HeredocStrip => 0,
    };
    redirection.fd.unwrap_or(implied) == 1
}

/// Every stage that is not a simple command naming a registered tool is reported as plain bytes,
/// which is what keeps compound commands, functions and externals on the path they always took.
fn plan_pipeline(env: &Environment, pipeline: &Pipeline) -> Vec<Sink> {
    let stages: Vec<Stage> = pipeline
        .commands
        .iter()
        .map(|command| {
            let Command::Simple(simple) = command else {
                // A compound command or a function: not in-process in the sense that matters,
                // because its own stages are pipelines of their own.
                return Stage::bytes();
            };
            let Some(name) = simple_command_name(env, simple) else {
                return Stage::bytes();
            };
            match crate::data::tool::lookup(&name) {
                Some(tool) => Stage {
                    accepts: tool.accepts,
                    produces: tool.produces,
                    in_process: true,
                    // A redirection of **stdout** means the user asked for this stage's output at a
                    // named place, and that outranks anything the planner would prefer.
                    //
                    // Only stdout. Any redirection at all used to count, so `2>/dev/null` — which
                    // cannot touch a single row — forced text on the stage, left no structured edge
                    // and dropped the whole line onto the byte path, where the verbs are not
                    // commands: `… | lines | first 2 2>/dev/null | cat` answered
                    // `lines: command not found`. Adding a stderr redirection to a working pipeline
                    // must not change what the pipeline *is*.
                    redirected: simple.redirections.iter().any(redirects_stdout),
                },
                None => Stage::bytes(),
            }
        })
        .collect();
    // **Nothing structured here, so do not ask the terminal.** `is_terminal` is an `ioctl`, and it
    // was being issued for every pipeline the shell ran — one failing syscall per simple command,
    // including a bare `x=1` — to decide the last sink of a plan in which nothing carries rows.
    // A pipeline of plain commands has the same answer either way: bytes all the way down.
    if stages.iter().all(|stage| !stage.in_process) {
        return vec![Sink::Text; stages.len()];
    }
    crate::data::plan(
        &stages,
        std::io::IsTerminal::is_terminal(&std::io::stdout()),
    )
}

/// The sinks for this pipeline, when any edge of it carries rows.
///
/// One call, not two: [`plan`](crate::data::plan) counts the structured edges it hands out, and
/// planning the same pipeline twice would report twice as many as were actually taken — which
/// matters because that count is what the POSIX assertion reads.
pub(super) fn structured_sinks(env: &Environment, pipeline: &Pipeline) -> Option<Vec<Sink>> {
    // **Asked before anything is built.** Planning allocates a `Vec` of stages, clones every
    // command word, locks the registry once per stage, and issues an `ioctl` to ask whether stdout
    // is a terminal. With nothing registered the answer is `None` for every pipeline, so all of
    // that was spent to learn nothing — measured at one failing `ioctl` per simple command, on the
    // hottest path the shell has.
    if !crate::data::tool::any_registered() {
        return None;
    }
    let sinks = plan_pipeline(env, pipeline);
    if sinks.contains(&Sink::Rows) {
        return Some(sinks);
    }
    // **A tool on its own has no edge, and `Sink::Rows` only ever describes an edge.** So a
    // pipeline of one plans to no rows at all and falls to the byte path, where the name is looked
    // up on `$PATH`.
    //
    // For `ls`, `ps` and `df` that is the right answer and must stay: a bare `ls` is coreutils, and
    // oslo's row-shaped one is what you get when you ask for structure by piping it somewhere. A
    // name a config invented has no such counterpart — `oslo.register_tool{name = "stale", …}` then
    // answered `stale: command not found`, and only `stale | length` ran it, which is not a
    // discoverable interface for a feature whose whole point is adding a command.
    (lone_custom_tool(env, pipeline) || bridge_at_the_tail(env, pipeline)).then_some(sinks)
}

/// The last stage is a **bridge**: a tool that takes bytes and gives rows.
///
/// `cat data.json | from json` answered `from: command not found`, and so did `seq 1 3 | lines` and
/// every `parse` written without a verb after it. The edge rule above is why — a bridge at the tail
/// has no downstream stage, so no edge of the pipeline carries rows — but the fallback that rule
/// protects is not one a bridge has. `ls`, `ps`, `df` and `history` are `Nothing -> Rows` and each
/// shadows a real command a bare name must keep reaching; `from`, `lines`, `parse` and
/// `detect-columns` are `Bytes -> Rows` and shadow nothing at all, so falling through to `$PATH`
/// finds nobody and reports a name that is not missing.
///
/// **The shapes are the test, not a list of four names.** A bridge is anything that can read bytes
/// and answers rows, so one registered later is covered by having been declared — the same argument
/// the planner makes everywhere else. `text` is the case that widened it from `Bytes` to *takes*
/// bytes: it reads rows when a stage before it has them and its own operands when nothing does,
/// which is `Any`, and `text upper hello` on its own must still be a command.
///
/// A redirection is **not** excluded here, unlike in [`lone_custom_tool`]. The last stage is the
/// one whose redirection `run` applies around its own output, which is also why the planner lets
/// rows reach a redirected consumer when it is last, so `cat data.json | from json > rows.txt`
/// writes the transport to the file and nothing to the terminal.
fn bridge_at_the_tail(env: &Environment, pipeline: &Pipeline) -> bool {
    let Some(Command::Simple(simple)) = pipeline.commands.last() else {
        return false;
    };
    simple_command_name(env, simple)
        .and_then(|name| crate::data::tool::lookup(&name))
        .is_some_and(|tool| {
            tool.accepts.takes_bytes() && tool.produces == crate::data::plan::Shape::Rows
        })
}

/// One simple command, naming a tool the *config* registered, with nothing redirected.
///
/// Redirections are excluded because [`run`] writes with `println!` — the structured stages do not
/// fork, so nothing has applied `> file` to this process's stdout, and routing `stale > out` here
/// would print to the terminal and leave the file empty. The byte path keeps those.
fn lone_custom_tool(env: &Environment, pipeline: &Pipeline) -> bool {
    let [Command::Simple(simple)] = pipeline.commands.as_slice() else {
        return false;
    };
    simple.redirections.is_empty()
        && simple_command_name(env, simple)
            .is_some_and(|name| crate::data::custom::registered(&name))
}

/// Run a pipeline whose edges carry rows.
///
/// **No forking.** Every stage here runs in this process, one after another, and the rows move by
/// being handed over — there is no descriptor between them, so there is nothing to serialise and
/// no format to get wrong. That is the whole argument for keeping structure in-process.
///
/// `fallback` is the byte path, used for anything this cannot run.
pub(super) fn run(
    env: &mut Environment,
    pipeline: &Pipeline,
    sinks: &[Sink],
    fallback: fn(&mut Environment, &Pipeline) -> Result<i32>,
) -> Result<i32> {
    // **Before the byte prefix, let alone the tools.** A column nothing can be carrying is refused
    // here, where nothing has run and nothing has had a side effect. Status 2 is what
    // `tools::unknown_column` answers for the same mistake, so the two do not disagree about what
    // it costs — only about how early it is noticed.
    if let Some(problem) = refuse_unknown_column(env, pipeline) {
        crate::env::complain_from(
            &env.origin(),
            &problem.words,
            &problem.word,
            &problem.message,
            problem.label,
            problem.help.as_deref(),
        );
        return Ok(2);
    }

    // **An upstream with no end is read in slices rather than to its end.** Everything below this
    // materialises — `capture` reads the whole prefix before the first verb runs — so
    // `tail -f app.log | lines | where …` printed nothing at all, for ever. When every part of a
    // pipeline can be streamed, it is; when any part cannot, this answers `None` and the general
    // path runs exactly as it always did. See `stream::plan` for what "can be" means.
    if let Some(streamed) = stream::plan(env, pipeline, sinks) {
        return stream::run(env, pipeline, sinks, &streamed, fallback);
    }

    let mut rows: Option<Vec<crate::data::Record>> = None;
    let mut statuses = Vec::with_capacity(pipeline.commands.len());

    // **The last stage's redirection, applied around everything this writes.**
    //
    // The structured stages do not fork, so nothing has pointed this process's stdout at the file
    // the user named — and `run` writes with `println!`. The planner will only route rows *into* a
    // redirected stage when it is the last one, precisely so this is the only redirection that has
    // to be honoured here. The guard restores the descriptor when it drops, however this returns.
    let mut redirected = crate::exec::redirect::RedirectGuard::new();
    if let Some(Command::Simple(last)) = pipeline.commands.last()
        && !last.redirections.is_empty()
    {
        let redirections = last.redirections.clone();
        redirected.apply(env, &redirections)?;
    }

    // **The byte prefix.** `kubectl get pods -o json | from json | where ...` is an external
    // followed by structured stages: the external cannot hand over rows, so its *output* is what
    // the first structured stage is given. Everything up to the first tool runs exactly as it
    // always did, forked and on descriptors, and only its bytes cross into this half.
    let first_tool = pipeline.commands.iter().position(|c| runnable_here(env, c));
    let mut bytes: Option<String> = None;
    let start = match first_tool {
        Some(0) | None => {
            // **A bytes-taking tool at the head of the pipeline reads the shell's own input.**
            //
            // There is no stage before it, so `bytes` stayed `None` and `lines` was handed the
            // empty string:
            //
            // ```text
            // printf 'a\nb\nc\n' | oslo -c 'lines | length'      answered 0
            // printf 'a\nb\nc\n' | oslo -c 'cat | lines | length'  answers 3
            // ```
            //
            // Zero rows for three lines of input, silently — a wrong answer where a refusal would
            // have been survivable, which is the one failure `docs/known-gaps.md` opens by saying
            // oslo does not have. It does now what `cat` would do in its place: read standard
            // input to the end. At a terminal that waits for Ctrl-D, exactly as `wc -l` does.
            if let Some(Command::Simple(first)) = pipeline.commands.first()
                && let Some(name) = simple_command_name(env, first)
                && crate::data::tool::lookup(&name).is_some_and(|tool| {
                    matches!(
                        tool.accepts,
                        crate::data::Shape::Bytes | crate::data::Shape::Any
                    )
                })
            {
                match read_standard_input() {
                    Upstream::Read(text) => bytes = Some(text),
                    // Interrupted: 130 is what a shell reports for a line Ctrl-C ended, and
                    // returning it here leaves the queued keystrokes to be read as commands.
                    Upstream::Interrupted => return Ok(130),
                    Upstream::TooLarge => {
                        eprintln!("{}{}", crate::env::origin_now(), too_large(&name));
                        return Ok(1);
                    }
                }
            }
            0
        }
        Some(at) => {
            let prefix = Pipeline {
                commands: pipeline.commands[..at].to_vec(),
                negated: false,
                timed: false,
            };
            match capture(env, &prefix, fallback) {
                Ok((status, output)) => {
                    // One status per prefix stage; the byte path filled in its own vector, and
                    // this half appends to it rather than replacing it.
                    statuses.extend(env.pipeline_status().iter().copied());
                    if statuses.is_empty() {
                        statuses.push(status);
                    }
                    bytes = Some(output);
                    at
                }
                Err(e) => return Err(e),
            }
        }
    };

    // **Decided before any stage runs**, because a tool that *prints* — `to json` writes with
    // `println!` and hands back no rows — would otherwise have written past the seam already. With
    // a byte suffix coming, the tool half's stdout goes to a scratch file for its duration.
    let mut printed = match byte_suffix_at(env, pipeline, start) {
        Some(_) => Some(Printed::start()?),
        None => None,
    };

    for (i, command) in pipeline.commands.iter().enumerate().skip(start) {
        // **Where this half stops.** A stage that is not a tool cannot run here, and what happens
        // next depends entirely on whether anything has run yet.
        if !runnable_here(env, command) {
            return match i > start {
                // Nothing structured has happened, so the byte path can have the whole pipeline
                // exactly as it always did.
                false => fallback(env, pipeline),
                // Some of it *has* run, and falling back would run it again — `ls | first 2 | cat`
                // executed the structured `ls`, then re-ran the whole line on the byte path where
                // `first` is not a command: empty output, `first: command not found`, and status 0.
                // A wrong answer that reported success, which is the failure this project opens by
                // saying it does not have. So the rows made so far are rendered and handed to the
                // rest of the pipeline as its standard input.
                true => hand_over(
                    env,
                    pipeline,
                    i,
                    rows.take(),
                    printed.take(),
                    &mut statuses,
                    fallback,
                ),
            };
        }
        let Command::Simple(simple) = command else {
            return fallback(env, pipeline);
        };
        let Some(name) = simple_command_name(env, simple) else {
            return fallback(env, pipeline);
        };
        let words = expand_words(env, simple)?;

        // Published for the same reason `exec_custom_builtin` publishes it: a structured verb is a
        // builtin in every sense that matters to whoever is reading the diagnostic, and this is the
        // one place they are all dispatched from. See `env::scope::origin`.
        let _origin = crate::env::scope::origin::Published::new(env.origin());
        // **A verb that *prints* has to cross the seam too.** `to json` and `to text` write with
        // `println!` and hand back no rows, so when the next stage reads bytes there is nothing to
        // give it: `df | to json | from json` dumped the JSON to the terminal and then told
        // `from json` about an empty input. Stdout goes to a scratch file for this stage's
        // duration, exactly as it already does when a *byte* suffix follows — which is why this is
        // armed only when that one is not, rather than nesting two redirections.
        // **Only an edge into another *tool*.** A next stage this half cannot run — `cat`, a
        // function, a compound — is the byte suffix, and `hand_over` renders the rows for it and
        // runs the rest of the pipeline on that. Turning them into bytes here would leave that
        // path with nothing to hand over: `ls | first 2 | cat` printed nothing at all.
        let crosses_as_text = pipeline
            .commands
            .get(i + 1)
            .is_some_and(|next| runnable_here(env, next))
            && !matches!(sinks.get(i), Some(crate::data::plan::Sink::Rows));
        let mut caught = match crosses_as_text && printed.is_none() {
            true => Printed::start().ok(),
            false => None,
        };
        // **This stage's own redirections, around this stage only.**
        //
        // The last stage's are applied once, around everything, further up — that is the one whose
        // output this half writes. A stage before it keeps its rows in memory, so the only
        // redirections that can reach here are the ones the planner let through: those that leave
        // stdout alone. Applying them is what makes `… | first 2 2>/dev/null | …` actually quiet,
        // rather than merely no longer fatal.
        let mut per_stage = crate::exec::redirect::RedirectGuard::new();
        if i + 1 < pipeline.commands.len()
            && let Command::Simple(simple) = command
            && !simple.redirections.is_empty()
        {
            let redirections = simple.redirections.clone();
            per_stage.apply(env, &redirections)?;
        }
        let (status, produced) =
            match crate::data::tools::run_tool(&name, &words, rows.take(), bytes.as_deref()) {
                Some(outcome) => outcome,
                // Registered, but it declined to run — a shape it cannot take, an argument it will
                // not have. Same rule as above: whole pipeline back to bytes if nothing has run,
                // hand over what there is if something has.
                None => {
                    return match i > start {
                        false => fallback(env, pipeline),
                        true => hand_over(
                            env,
                            pipeline,
                            i,
                            rows.take(),
                            // Whichever capture is armed — see above; never both.
                            printed.take().or_else(|| caught.take()),
                            &mut statuses,
                            fallback,
                        ),
                    };
                }
            };
        statuses.push(status);
        // **The edge's own sink decides what crosses it.** The planner already works this out —
        // `Sink::Rows` where the next stage takes rows, `Sink::Text` where it takes bytes — and
        // this loop used to hand rows on regardless. A bytes-accepting tool after a rows-producing
        // one was therefore given *nothing*: `ls | lines | length` answered 0 where `ls | length`
        // answers 2, and `df | to json | from json | length` printed the JSON to the terminal, then
        // `EOF while parsing`, then 0, with status 0 — the round trip those verbs exist for.
        //
        // The last stage is not an edge; it writes itself out below, so its rows are kept.
        let caught = caught.take().map(Printed::finish);
        match crosses_as_text {
            // Rendered for a reader of bytes, in the plain transport form — never the drawn table,
            // for the reason the final write-out gives below. A verb that produced no rows printed
            // instead, and what it printed is what crosses.
            true => {
                bytes = match produced {
                    Some(table) => Some(crate::data::render_transport(&crate::data::Val::table(
                        table,
                    ))),
                    None => caught.filter(|text| !text.is_empty()),
                };
                rows = None;
            }
            false => {
                rows = produced;
                // The bytes belong to the first structured stage only; a second `lines` further
                // down is reading rows, not the original output all over again.
                bytes = None;
            }
        }

        // Everything but the last stage hands its rows on. The last one writes them out, in
        // whichever of the two renderings its sink asked for.
        if i + 1 == pipeline.commands.len()
            && let Some(final_rows) = rows.take()
        {
            {
                let value = crate::data::Val::table(final_rows);
                let text = match sinks.last() {
                    Some(Sink::Print) => crate::data::render_display(&value),
                    // Into a pipe or a file: the plain, complete form. **Never the drawn table** —
                    // a box-drawing character on another program's stdin is the failure this whole
                    // design exists to prevent.
                    _ => crate::data::render_transport(&value),
                };
                if !text.is_empty() {
                    println!("{text}");
                }
            }
        }
    }

    // **`pipeline_status`, not `last()`.** `set -o pipefail` is a property of the pipeline, not of
    // the path it happens to run on: taking the last status meant appending one structured verb to
    // an ordinary byte pipeline silently disarmed pipefail for the whole thing, and `set -e` with
    // it. The byte path's own helper answers this question, so both ask it the same way.
    let status = super::pipeline_status(env, &statuses);
    // The stages did not fork, so there are no child statuses to collect — but `PIPESTATUS` must
    // still describe the pipeline the user wrote, or `${PIPESTATUS[0]}` starts lying the moment a
    // pipeline happens to be structured.
    env.set_pipeline_status(statuses);
    Ok(status)
}

/// The words of a simple command, expanded as the byte path would expand them.
fn expand_words(env: &mut Environment, simple: &SimpleCommand) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for word in &simple.words {
        out.extend(crate::expand::expand_word(env, word)?);
    }
    Ok(out)
}
