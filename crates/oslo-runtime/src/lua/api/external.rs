//! A prompt produced by another program — starship, hexe, anything that writes a prompt to stdout.
//!
//! ```lua
//! oslo.prompt.left = {
//!   command = "starship",
//!   args = { "prompt", "--status", "$status", "--cmd-duration", "$duration_ms" },
//!   timeout_ms = 200,
//!   async = true,
//! }
//! ```
//!
//! The `$name` arguments are filled from the same context a segment's `render(ctx)` receives, so
//! the tool is told the exit status and how long the last command took without the config having to
//! plumb them through the environment.
//!
//! **Two protections, because a prompt is on the critical path of every keystroke.**
//!
//! `timeout_ms` bounds how long the shell will wait. A tool that hangs — a network mount, a git
//! repository the size of a planet — cannot hang the shell; the deadline passes and the last
//! prompt it produced is used instead.
//!
//! `async` goes further: the previous output is used *immediately* and the tool runs behind the
//! prompt, so its cost is never on the path at all. The first prompt of a session has nothing to
//! reuse and waits for one run, which is the only time it can be seen.

use crate::lua::context::Context;
use oslo_base::value::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[path = "external/strip.rs"]
mod strip;
use strip::{absorb, playing};

/// What a config asked for.
pub struct Spec {
    pub command: String,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub asynchronous: bool,
    /// `every = <ms>` — re-run this prompt on a clock, so the tool drawing it can animate.
    ///
    /// ```lua
    /// oslo.prompt.left = { command = "pixy", args = { … }, async = true, every = 150 }
    /// ```
    ///
    /// **Off unless asked for, and asked for is a real cost.** Every other prompt is re-run when
    /// its *inputs* could have moved; this one is re-run because time passed, which for a command
    /// means a process spawn per frame for as long as the shell is open. It exists because a prompt
    /// somebody has already built — with its own colours, its own zones, its own layout — cannot
    /// grow a moving part any other way: the alternative is replacing it with a segment list, which
    /// means rebuilding all of it somewhere else to gain one turning glyph.
    ///
    /// So the tool draws the frame. oslo only decides when to ask again.
    ///
    /// **The clock does not run where nothing can be drawn.** On `TERM=dumb` — a program driving
    /// oslo over a pty, a serial console — a frame has no product, so asking for one would be a
    /// process spawned six times a second for a picture that never appears. See
    /// `oslo_ui::prompt::animation::animate_in`.
    pub every: Option<Duration>,

    /// `frames = <ms>` — ask the tool for every frame of the next `<ms>` at once, and animate from
    /// what comes back instead of spawning it again per frame.
    ///
    /// ```lua
    /// oslo.prompt.right = {
    ///   command = "pixy",
    ///   args = { "render", "prompt.right", "--frames-ms", "$frames_ms", … },
    ///   frames = 1200, every = 150,
    /// }
    /// ```
    ///
    /// `every` alone re-runs the tool on a clock, which is a process spawn per frame for as long
    /// as the shell is open. The animation, though, needs nothing from outside: the pictures are a
    /// function of time, so they can all be drawn in one run and played back here. `every` then
    /// only says how often to *redraw* — no spawn — and this says how far ahead to ask.
    ///
    /// Opt-in because it is a promise about the tool: that it understands the horizon it is sent
    /// and answers with a list rather than a picture. A tool that does not is left alone.
    pub frames: Option<Duration>,
}

/// Read the external-prompt form out of a Lua table, or `None` if it is not one.
pub fn spec_of(value: &Value) -> Option<Spec> {
    let Value::Table(t) = value else {
        return None;
    };
    let t = t.borrow();
    let Value::Str(command) = t.get_str("command") else {
        return None;
    };
    let mut args = Vec::new();
    if let Value::Table(list) = t.get_str("args") {
        let list = list.borrow();
        for i in 1..=list.length() {
            match list.get(&Value::int(i)) {
                Value::Str(s) => args.push(s.to_string()),
                Value::Number(n) => args.push(n.to_string()),
                _ => {}
            }
        }
    }
    let timeout = match t.get_str("timeout_ms") {
        Value::Number(n) => n.as_int().unwrap_or(200).max(1) as u64,
        // Long enough for a tool doing real work, short enough that a hung one is noticed as a
        // pause rather than as a dead shell.
        _ => 200,
    };
    // **A floor above a segment's.** A segment that animates calls a Lua function; this spawns a
    // process. Sixty milliseconds is reasonable for the first and sixteen spawns a second is not
    // reasonable for the second, so the two have different floors and this is the higher one.
    let every = match t.get_str("every") {
        Value::Number(n) => n
            .as_int()
            .filter(|ms| *ms > 0)
            .map(|ms| Duration::from_millis((ms as u64).max(MIN_EVERY_MS))),
        _ => None,
    };
    let frames = match t.get_str("frames") {
        Value::Number(n) => n
            .as_int()
            .filter(|ms| *ms > 0)
            .map(|ms| Duration::from_millis(ms as u64)),
        _ => None,
    };
    Some(Spec {
        command: command.to_string(),
        args,
        timeout: Duration::from_millis(timeout),
        asynchronous: t.get_str("async").truthy(),
        every,
        frames,
    })
}

/// The fastest an external prompt may be re-run.
///
/// Ten frames a second, which is a spinner that reads as smooth and a hundred process spawns every
/// ten seconds. Below this the shell spends more time starting the tool than the terminal spends
/// drawing what it said.
const MIN_EVERY_MS: u64 = 100;

/// Substitute `$name` in an argument from the context.
///
/// **Every field a prompt segment can render is substitutable here**, and that is the rule rather
/// than a coincidence: an external prompt is oslo describing itself to a program that cannot look
/// inside, so anything a native segment may use it must be able to say out loud. `$vimode`, `$user`
/// and `$host` were in [`Context`] and missing from this list, which made them reachable from a
/// Lua segment and unreachable from starship, hexe or anything else run as a command.
///
/// An absent optional becomes the **empty string**, not the word `none`: the receiving program is
/// being told "no answer", and every argument parser already knows what an empty value means.
fn fill(arg: &str, ctx: &Context, frame: u64, horizon: Option<Duration>) -> String {
    let mut out = arg.to_string();
    for (name, value) in [
        ("$status", ctx.status.to_string()),
        ("$duration_ms", ctx.duration_ms.unwrap_or(0).to_string()),
        ("$cwd", ctx.cwd.clone()),
        ("$cols", ctx.cols.to_string()),
        ("$jobs", ctx.jobs.to_string()),
        ("$language", ctx.language.clone()),
        ("$branch", ctx.branch.clone().unwrap_or_default()),
        ("$vimode", ctx.vimode.clone().unwrap_or_default()),
        ("$user", ctx.user.clone()),
        ("$host", ctx.host.clone()),
        // **Which frame this is.** Every other field is a fact about the shell; this one is a fact
        // about the drawing, and it is here because a tool run afresh for each frame has no memory
        // of the last one. Without it `every` can only ask the same question faster.
        // **How far ahead to draw.** Empty unless `frames` asked for a strip, so a tool sent it
        // either way is told "no horizon" rather than a number it did not earn.
        //
        // Before `$frame`, which is a prefix of it: substitution is a series of replacements over
        // one string, so the shorter name would eat the longer one and leave `0s_ms` behind.
        (
            "$frames_ms",
            horizon
                .map(|h| h.as_millis().to_string())
                .unwrap_or_default(),
        ),
        ("$frame", frame.to_string()),
    ] {
        out = out.replace(name, &value);
    }
    out
}

/// How many times each prompt has been run, so `$frame` can say which one this is.
///
/// Wraps rather than saturating: a spinner indexes into a list of frames with it, and a counter
/// that stopped at the top would leave one stuck on the last glyph after a few weeks of uptime.
fn next_frame(key: &str) -> u64 {
    static FRAMES: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    let frames = FRAMES.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut frames) = frames.lock() else {
        return 0;
    };
    let at = frames.entry(key.to_string()).or_insert(0);
    *at = at.wrapping_add(1);
    *at
}

/// Which prompt an answer belongs to.
///
/// The arguments **as written**, before `$status` and the rest are filled in, so the identity does
/// not move when the shell's state does. See the note in [`render`].
fn key_of(spec: &Spec) -> String {
    format!("{} {}", spec.command, spec.args.join(" "))
}

/// The content generation each key was last actually run at.
///
/// **So a tick runs nothing.** An animated segment redraws the prompt on a clock, and a redraw
/// re-renders every key — including this one, which is a *process*. At eight frames a second that
/// is eight spawns a second of somebody's prompt tool, for ever, on a shell nobody is typing at;
/// with `async` it is eight overlapping runs whose output interleaves, which shows up as a prompt
/// whose colours come apart.
///
/// An external prompt never asks to be re-run — only a segment can, with `every` — so it is run
/// again when its *inputs* could have moved, which is what the content generation says. An async
/// answer landing calls `invalidate` itself, so a late arrival still gets through.
fn ran_at() -> &'static Mutex<HashMap<String, (u64, Instant)>> {
    static AT: OnceLock<Mutex<HashMap<String, (u64, Instant)>>> = OnceLock::new();
    AT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What this prompt said last time, when nothing has happened since that would change it.
///
/// The two answers to "would change it" are different, and that is the point:
///
/// * **Without `every`**, a run is owed when the content generation has moved — the directory, a
///   variable, the branch, or an `async` answer landing, which invalidates on its own.
/// * **With `every`**, a run is owed when that long has passed *and not before* — a rate limit as
///   much as a clock. An animated `async` prompt otherwise spawns itself in a loop: the answer
///   lands, the landing invalidates, the invalidation reads as "run again", and it does, as fast as
///   the tool can finish. Measured at 110 spawns in three seconds where twenty were asked for.
///
/// The cost of the rate limit is that a real change waits for the next frame. At the floor of
/// 100 ms that is not a wait anybody can see.
fn unchanged(key: &str, every: Option<Duration>) -> Option<String> {
    let (ran, at) = *ran_at().lock().ok()?.get(key)?;
    let fresh = match every {
        Some(every) => at.elapsed() < every,
        None => ran == oslo_ui::prompt::content_generation(),
    };
    fresh.then(|| remembered(key))?
}

/// Note that this key is being run, against the content and the clock as they now stand.
fn running_at(key: &str) {
    if let Ok(mut at) = ran_at().lock() {
        at.insert(
            key.to_string(),
            (oslo_ui::prompt::content_generation(), Instant::now()),
        );
    }
}

/// The last output each command produced, so a slow or async run has something to show.
fn cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remembered(key: &str) -> Option<String> {
    cache().lock().ok()?.get(key).cloned()
}

fn remember(key: &str, value: String) {
    if let Ok(mut c) = cache().lock() {
        c.insert(key.to_string(), value);
    }
}

/// How long the *first* answer is waited for, whatever the configured deadline says.
///
/// Long enough for a prompt tool doing real work on a slow machine, short enough that a hung one
/// is still noticed at startup rather than never.
const FIRST_ANSWER: Duration = Duration::from_secs(2);

/// Prompts whose tool has already missed its deadline once.
fn overran() -> &'static Mutex<HashSet<String>> {
    static SLOW: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SLOW.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether waiting for this prompt is known to be a waste of the deadline.
fn known_slow(key: &str) -> bool {
    overran().lock().is_ok_and(|slow| slow.contains(key))
}

/// Record whether the tool answered inside its deadline this time.
fn note_deadline(key: &str, in_time: bool) {
    if let Ok(mut slow) = overran().lock() {
        if in_time {
            slow.remove(key);
        } else {
            slow.insert(key.to_string());
        }
    }
}

/// Run the tool and return what it printed.
///
/// `None` when it could not be run at all, so the caller falls back to a prompt of its own rather
/// than drawing an empty line.
pub fn render(spec: &Spec, ctx: &Context) -> Option<String> {
    // **Keyed on the spec, not on the arguments it was filled with.**
    //
    // This is what makes `async` usable. The promise is "whatever it said last time, now" — but
    // the key used to be the *substituted* argv, so any argument that moves between prompts made
    // every lookup a miss, and a miss means the shell falls back to a prompt of its own. Every
    // real prompt moves at least one: `$status` after a failure, `$jobs` on a background job,
    // `$duration_ms` after almost every command. So `async` answered `None` forever and the tool
    // it was told to run asynchronously had to be made synchronous to be seen at all — paying its
    // full cost on the path of every keystroke, which is the one thing the option exists to avoid.
    //
    // The raw args identify which prompt this is (a left and a right differ by `--right`) and stay
    // put across prompts, which is exactly the identity wanted: last output for *this* prompt.
    let key = key_of(spec);

    // **Armed before the guard, not after it.** A prompt that animates says so in its spec, and it
    // says so whether or not *this* render is the one that runs the command. Arming only on a run
    // meant a frame arriving a hair early — the deadline and the last run measured from moments
    // that are not quite the same — found nothing to do, re-armed nothing, and the animation
    // stopped for good after exactly one frame.
    if let Some(every) = spec.every {
        oslo_ui::prompt::animate_in(every);
    }

    // A strip already in hand covers this frame, so play it and spawn nothing. Checked before
    // `unchanged` because it is the stronger claim: that one is "nothing has moved", this one is
    // "the next picture is already here".
    if spec.frames.is_some()
        && let Some(text) = playing(&key)
    {
        return Some(text);
    }

    // Nothing about this prompt has moved since it last ran — a tick asked for a redraw, not a
    // rebuild — so draw what it said then and spawn nothing. See `unchanged`.
    if let Some(text) = unchanged(&key, spec.every) {
        return Some(text);
    }
    running_at(&key);
    // **One frame per run, and only for a run that happens.** Counted here rather than beside the
    // substitution so a prompt with three arguments does not advance a spinner three glyphs, and
    // after the guard so a frame nobody drew does not consume one.
    let frame = next_frame(&key);
    let args: Vec<String> = spec
        .args
        .iter()
        .map(|a| fill(a, ctx, frame, spec.frames))
        .collect();

    if spec.asynchronous {
        // **Wait a little, then give up — rather than never waiting at all.**
        //
        // Returning the previous output unconditionally is the obvious reading of "asynchronous"
        // and it is wrong for a prompt: the arguments carry `$status`, `$duration_ms` and `$jobs`,
        // so a prompt drawn from the last run reports the status of the command *before* the one
        // that just finished. A `✗ 127` arriving one command late is worse than a prompt that took
        // a moment, because it is not wrong in a way you can see.
        //
        // So the run starts in the background and this waits `timeout_ms` for it. On a quiet
        // machine the tool answers well inside that and the prompt is both fresh and correct; when
        // it does not — a loaded machine, a cold git cache — the last good answer is drawn
        // immediately and the run goes on to fill the cache for next time. The stale prompt becomes
        // the exception rather than the rule, and the stall never exceeds the deadline the config
        // already declares.
        let previous = remembered(&key);

        // **A deadline that has already been missed is not waited for again.**
        //
        // The wait above is only worth making while there is a chance of a fresh answer. A tool
        // that reliably takes longer than its deadline — `hexe shp prompt` at ~33 ms against a
        // 10 ms one — loses that bet every time, and the shell pays the full deadline before
        // drawing exactly the answer it already had. Two prompts, left and right, made that
        // `2 × timeout_ms` on every command for nothing.
        //
        // So the first miss is remembered, and after it this returns the last answer immediately
        // and lets the background run refresh it. The outcome is identical — the same cached text
        // is drawn either way — and the deadline is no longer spent to reach it. A tool that
        // becomes fast again clears the mark on its next answer, so this recovers on its own.
        if known_slow(&key)
            && let Some(text) = previous
        {
            let _ = spawn(spec.command.clone(), args, key, spec.frames);
            return Some(text);
        }

        let ready = spawn(spec.command.clone(), args, key.clone(), spec.frames);
        // **With nothing to fall back to, the deadline is not worth keeping.**
        //
        // Answering `None` makes the caller draw oslo's *own* prompt instead — a different prompt
        // of a different width. The editor lays the row out against the width it was given, so the
        // next redraw writes in the wrong place and the screen doubles up: the symptom is a
        // session that flips between two prompts and repeats the output of the last command.
        //
        // A session has exactly one cold prompt, and the module doc already says that one waits.
        // Every prompt after it has an answer to show, and keeps the short deadline.
        let deadline = match previous {
            Some(_) => spec.timeout,
            None => spec.timeout.max(FIRST_ANSWER),
        };
        let started = std::time::Instant::now();
        return match ready.recv_timeout(deadline) {
            // Judged against the *configured* deadline, not the one actually waited for, so a
            // first answer that only arrived because of the grace above is still recorded as slow.
            Ok(Some(fresh)) => {
                note_deadline(&key, started.elapsed() <= spec.timeout);
                Some(fresh)
            }
            // It failed, or it is still running. Either way the last good answer beats a blank
            // prompt, and the thread will cache whatever it eventually produces.
            Ok(None) | Err(_) => {
                note_deadline(&key, false);
                remembered(&key)
            }
        };
    }

    match run(&spec.command, &args, spec.timeout) {
        Some(out) => Some(absorb(&key, out, spec.frames)),
        // It overran or failed. The last good answer beats a blank prompt, and beats waiting.
        None => remembered(&key),
    }
}

/// Run it in the background, keeping the result for the next prompt and announcing it on the way.
///
/// The channel is how [`render`] can wait a little for a *fresh* answer without giving up the
/// background run if it does not arrive: whoever is listening may stop listening, and the thread
/// goes on to cache the result regardless. A send into a dropped receiver is an error this
/// deliberately ignores.
///
/// The thread's own deadline stays generous and is not the caller's: a hung tool costs one thread
/// rather than the prompt, and the next run replaces whatever this one eventually says.
fn spawn(
    command: String,
    args: Vec<String>,
    key: String,
    horizon: Option<Duration>,
) -> std::sync::mpsc::Receiver<Option<String>> {
    let (ready, waiting) = std::sync::mpsc::channel();
    // Counted before the thread starts, so the editor cannot decide to block for a keystroke in
    // the window between asking for a refresh and the refresh being under way.
    oslo_ui::prompt::refresh_started();
    std::thread::spawn(move || {
        // Read before the run, because absorbing the answer is what writes the cache: comparing
        // afterwards would compare the fresh text with itself, find nothing changed, and never ask
        // for the repaint that puts it on screen.
        let previous = remembered(&key);
        // A filmstrip is turned into frames here rather than at the waiter, so the text that
        // reaches the cache, the channel and the screen is a prompt in every path.
        let out =
            run(&command, &args, Duration::from_secs(10)).map(|raw| absorb(&key, raw, horizon));
        if let Some(fresh) = out.clone() {
            let changed = previous.as_deref() != Some(fresh.as_str());
            // **An answer that arrives after the prompt was drawn still gets shown.**
            //
            // Without this, a tool slower than its deadline could never be seen for the command
            // it described: the prompt was drawn from the previous answer, this one replaced it
            // in the cache, and the *next* prompt drew it — one command late. For a prompt whose
            // arguments carry `$status` and `$duration_ms` that is the failure the deadline
            // exists to avoid, arriving by a different route.
            //
            // Bumping the generation is what the editor already watches to redraw a prompt whose
            // inputs moved, so the fresh text lands on screen as soon as it exists. Only when it
            // differs, or a stable prompt would repaint itself forever.
            if changed {
                oslo_ui::prompt::invalidate();
            }
        }
        // After the cache and the generation, so a waiter that wakes on this sees both.
        oslo_ui::prompt::refresh_finished();
        let _ = ready.send(out);
    });
    waiting
}

/// Run a command with a deadline, returning its stdout with the trailing newline trimmed.
pub(crate) fn run(command: &str, args: &[String], timeout: Duration) -> Option<String> {
    use std::process::{Command, Stdio};
    // Resolved through the shell's own table rather than left to `execvp`, which tries every
    // `$PATH` entry in turn and pays for the miss on each one. This runs from a prompt, so the
    // same name is looked up on every prompt for the life of the session; on a Nix dev shell's
    // 48-entry `$PATH` that is 48 `execve` calls each time against one.
    let resolved = oslo_shell::env::builtins::hash_lookup(command);
    let command: &std::ffi::OsStr = match &resolved {
        Some(path) => path.as_os_str(),
        None => std::ffi::OsStr::new(command),
    };
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Left alone on purpose: a tool's complaints belong on the terminal where the user can see
        // them, not folded into the prompt.
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(None) => {
                // Overran. Killed rather than left behind: one hung prompt tool per keystroke
                // would otherwise become hundreds of processes in a session.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            // `ECHILD`: something reaped the child before this loop could — a `SIGCHLD`
            // disposition of `SIG_IGN` does exactly that, and a shell is a program likely to have
            // one. It means the command *finished*, not that it failed, so fall through and read
            // what it wrote. Returning here made every run produce nothing whenever such a handler
            // was installed, which is how the test found it.
            Err(_) => break,
        }
    }

    // Read the pipe *after* the wait loop, and not with `wait_with_output`: the loop above has
    // already reaped the child, so `wait_with_output` fails and every run would return nothing.
    // The test caught exactly that.
    use std::io::Read;
    let mut text = String::new();
    child.stdout.take()?.read_to_string(&mut text).ok()?;
    Some(text.trim_end_matches('\n').to_string())
}

#[cfg(test)]
#[path = "external/tests.rs"]
mod tests;
