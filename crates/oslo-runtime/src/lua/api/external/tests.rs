//! A prompt drawn by somebody else's program.
//!
//! Split from [`super`] when that file crossed the 600-line limit. The seam is the usual one: the
//! module is the mechanism, this is what the mechanism must do.

use super::*;

fn ctx() -> Context {
    Context {
        status: 3,
        duration_ms: Some(1500),
        cwd: "/tmp/x".to_string(),
        cols: 100,
        language: "lua".to_string(),
        ..Context::default()
    }
}

/// **The cache key does not move when the shell's state does.**
///
/// This is the whole of `async`. Keyed on the filled arguments, a prompt passing `$status` or
/// `$duration_ms` — which is every prompt worth writing — missed on every lookup, answered
/// `None`, and was replaced by the shell's own fallback. The only way to see such a tool at
/// all was `async = false`, paying its full cost between every keystroke and the screen.
#[test]
fn an_asynchronous_prompt_is_found_again_after_the_status_changes() {
    let spec = Spec {
        command: "hexe".to_string(),
        args: vec!["prompt".to_string(), "--status=$status".to_string()],
        timeout: Duration::from_millis(400),
        asynchronous: true,
        every: None,
        frames: None,
    };
    let first = key_of(&spec);

    let mut later = ctx();
    later.status = 127;
    later.duration_ms = Some(90_000);
    assert_eq!(
        first,
        key_of(&spec),
        "a failed command must not lose the prompt its own tool drew"
    );

    // And two prompts are still told apart, or a right prompt would answer with a left one.
    let right = Spec {
        command: "hexe".to_string(),
        args: vec![
            "prompt".to_string(),
            "--right".to_string(),
            "--status=$status".to_string(),
        ],
        timeout: Duration::from_millis(400),
        asynchronous: true,
        every: None,
        frames: None,
    };
    assert_ne!(first, key_of(&right));
}

/// The tool is told what the shell knows, without the config plumbing it through the
/// environment by hand.
#[test]
fn arguments_are_filled_from_the_context() {
    assert_eq!(fill("--status=$status", &ctx(), 0, None), "--status=3");
    assert_eq!(
        fill("--cmd-duration=$duration_ms", &ctx(), 0, None),
        "--cmd-duration=1500"
    );
    assert_eq!(
        fill("--terminal-width=$cols", &ctx(), 0, None),
        "--terminal-width=100"
    );
    // A name that is not a placeholder is left exactly as written.
    assert_eq!(fill("--keep-$this", &ctx(), 0, None), "--keep-$this");
}

/// **Every renderable field can be named.** A field that a Lua segment can read but an
/// external prompt cannot ask for is a field that works in one prompt and silently vanishes in
/// the other — which is how `$vimode` came to exist on `Context` and be unreachable from
/// starship or hexe. If a field is added to `Context`, it is added here too.
#[test]
fn every_context_field_a_prompt_can_render_is_substitutable() {
    let mut facts = ctx();
    facts.vimode = Some("normal".to_string());
    facts.user = "ada".to_string();
    facts.host = "lovelace".to_string();
    facts.language = "lua".to_string();
    facts.branch = Some("main".to_string());

    assert_eq!(fill("$vimode", &facts, 0, None), "normal");
    assert_eq!(fill("$user@$host", &facts, 0, None), "ada@lovelace");
    assert_eq!(fill("$language", &facts, 0, None), "lua");
    assert_eq!(fill("$branch", &facts, 0, None), "main");

    // An absent optional is the empty string, so `--vimode=` reaches the program as "no
    // answer" rather than as the literal word `none` it would then have to special-case.
    facts.vimode = None;
    facts.branch = None;
    assert_eq!(fill("--vimode=$vimode", &facts, 0, None), "--vimode=");
    assert_eq!(fill("--branch=$branch", &facts, 0, None), "--branch=");
}

/// A tool that never finishes must not become a shell that never prompts.
#[test]
fn a_command_that_overruns_is_killed_and_reported_as_nothing() {
    let started = std::time::Instant::now();
    let sleep = ["/bin/sleep", "/usr/bin/sleep"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .expect("a system sleep");
    let out = run(sleep, &["10".to_string()], Duration::from_millis(60));
    assert!(out.is_none(), "an overrun produces nothing, not a hang");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the deadline was not honoured: waited {:?}",
        started.elapsed()
    );
}

/// What it printed, without the newline every tool ends with.
#[test]
fn output_is_taken_verbatim_less_the_trailing_newline() {
    // An absolute path, not `echo`: other tests in this binary mutate the process-wide
    // `$PATH`, and a bare name would make this test depend on whichever of them ran first.
    let echo = ["/bin/echo", "/usr/bin/echo"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .expect("a system echo");
    let out = run(echo, &["hi".to_string()], Duration::from_secs(30));
    assert_eq!(out.as_deref(), Some("hi"));
}

/// **A tick must not spawn anything.** An animated segment redraws the prompt on a clock, and a
/// redraw re-renders every prompt key — including this one, which is a process. Measured on a
/// prompt of one external command and one segment at `every = 120` over three seconds: without
/// this guard the command ran 24 times for 23 frames; with it, twice.
///
/// Twice and not once because an `async` answer landing calls `invalidate` itself, which is the
/// door a late arrival is supposed to come through and must stay open.
#[test]
fn nothing_is_re_run_until_the_content_could_have_changed() {
    // The content generation is one number for the whole process, and this test's subject is
    // whether a reading of it still holds. See `crate::serial`.
    let _serial = crate::serial::generation();
    let key = "prompt.test.unchanged";
    remember(key, "drawn".to_string());

    // Never run at this generation: there is no answer to reuse, whatever is remembered.
    assert_eq!(unchanged(key, None), None, "it has not run yet");

    running_at(key);
    assert_eq!(
        unchanged(key, None).as_deref(),
        Some("drawn"),
        "nothing has moved, so draw what it said"
    );

    // A real change — a directory, a variable, an async answer landing — and it runs again.
    oslo_ui::prompt::invalidate();
    assert_eq!(unchanged(key, None), None, "the content moved");
}

/// `every` is read, floored, and off unless asked for.
#[test]
fn an_interval_is_read_and_floored() {
    let spec = |lua: &str| {
        let mut t = oslo_base::value::Table::new();
        t.set_str("command", Value::str("p"));
        if !lua.is_empty() {
            t.set_str("every", Value::int(lua.parse::<i64>().unwrap()));
        }
        spec_of(&Value::Table(std::rc::Rc::new(std::cell::RefCell::new(t)))).expect("a spec")
    };
    assert_eq!(spec("").every, None, "off unless asked for");
    assert_eq!(
        spec("0").every,
        None,
        "zero is off, not as-fast-as-possible"
    );
    assert_eq!(spec("150").every, Some(Duration::from_millis(150)));
    assert_eq!(
        spec("5").every,
        Some(Duration::from_millis(100)),
        "floored: this spawns a process per frame"
    );
}

/// **`$frame` is what makes `every` mean anything.** A prompt that is a command is a fresh
/// process each time; it cannot count its own frames, so oslo counts them for it. Without this
/// field `every` can only ask the same question faster.
#[test]
fn the_frame_number_is_substitutable_and_advances_per_run() {
    let facts = ctx();
    assert_eq!(fill("$frame", &facts, 0, None), "0");
    assert_eq!(fill("f=$frame", &facts, 7, None), "f=7");
    // One frame per *render*, not per argument: a prompt with three of them must not advance a
    // spinner three glyphs.
    assert_eq!(fill("$frame-$frame-$frame", &facts, 2, None), "2-2-2");

    let key = "prompt.test.frames";
    let first = next_frame(key);
    assert_eq!(next_frame(key), first + 1, "it counts up");
    assert_ne!(
        next_frame("prompt.test.other"),
        first + 2,
        "and counts per prompt, so a left and a right do not share a spinner"
    );
}

/// **A frame arriving early must not end the animation.** The deadline and the last run are
/// measured from moments that are not quite the same, so a tick can find nothing to do. Arming
/// only when the command actually ran meant that one early frame stopped the prompt moving for
/// the rest of the session — measured as exactly one frame in three seconds, where twenty were
/// asked for.
///
/// This is the property that made that impossible: the interval belongs to the spec, so it is
/// still an animated prompt on a render that reused its last answer.
#[test]
fn an_interval_belongs_to_the_spec_not_to_a_run() {
    let mut t = oslo_base::value::Table::new();
    t.set_str("command", Value::str("p"));
    t.set_str("every", Value::int(150));
    let spec =
        spec_of(&Value::Table(std::rc::Rc::new(std::cell::RefCell::new(t)))).expect("a spec");

    let key = "prompt.test.spec_interval";
    remember(key, "held".to_string());
    running_at(key);
    // The guard reuses — nothing has moved and the interval has not elapsed — and that is
    // exactly the render that used to forget to ask for the next frame.
    assert_eq!(
        unchanged(key, spec.every).as_deref(),
        Some("held"),
        "reused, as it should be"
    );
    assert!(spec.every.is_some(), "and it is still an animated prompt");
}

/// **`every` is a rate limit as well as a clock.** An animated `async` prompt otherwise spawns
/// itself in a loop — the answer lands, the landing invalidates, the invalidation reads as
/// "run again" — measured at 110 spawns in three seconds where twenty were asked for.
///
/// So with an interval, a content change does *not* force an early run: it is drawn on the next
/// frame, which at the 100 ms floor is not a wait anybody can see.
#[test]
fn an_interval_holds_even_when_the_content_changes() {
    let _serial = crate::serial::generation();
    let key = "prompt.test.rate_limit";
    let every = Some(Duration::from_secs(30));
    remember(key, "held".to_string());
    running_at(key);

    oslo_ui::prompt::invalidate();
    assert_eq!(
        unchanged(key, every).as_deref(),
        Some("held"),
        "a change does not beat the clock"
    );
    // Without an interval the same change is exactly what does force a run.
    assert_eq!(unchanged(key, None), None, "and without one, it does");
}
/// `$frames_ms` tells the tool the horizon, and says nothing when none was asked for.
#[test]
fn the_horizon_reaches_the_tool_only_when_asked_for() {
    let ctx = ctx();
    assert_eq!(
        fill(
            "--frames-ms=$frames_ms",
            &ctx,
            0,
            Some(Duration::from_millis(1200))
        ),
        "--frames-ms=1200"
    );
    assert_eq!(
        fill("--frames-ms=$frames_ms", &ctx, 0, None),
        "--frames-ms="
    );
}

/// **A tool that leaves something behind must not hold the shell.**
///
/// The output used to be read with no deadline, after the wait loop had already reaped the child.
/// A grandchild — a `&`, an ssh ControlMaster, a daemonising helper — inherits the write end of the
/// pipe, so EOF never came and the thread drawing the prompt never returned: an interactive shell
/// that never prompts, never reads a line and cannot be exited. Measured before the fix, this exact
/// shape gave `exit=124 prompt=0 HELLO=0`; after it, `exit=0 prompt=2 HELLO=3`.
#[test]
fn a_tool_that_leaves_a_process_behind_still_answers() {
    let started = std::time::Instant::now();
    let answer = super::run(
        "sh",
        &["-c".to_string(), "sleep 30 & printf 'ready'".to_string()],
        Duration::from_millis(2000),
    );
    let took = started.elapsed();
    assert_eq!(answer.as_deref(), Some("ready"), "the output is used");
    assert!(
        took < Duration::from_secs(3),
        "it returned rather than waiting for the grandchild: {took:?}"
    );
}

/// **More than a pipe buffer must not deadlock.** A pipe holds 64 KB; a tool with more to say
/// blocks on the write, so a loop that only polled for exit waited out the whole deadline, killed a
/// tool that was working, and threw the output away — on every prompt.
#[test]
fn a_tool_that_says_more_than_a_pipe_holds_is_not_killed() {
    let answer = super::run(
        "sh",
        &[
            "-c".to_string(),
            "awk 'BEGIN{for(i=0;i<40000;i++)printf \"x\"}'".to_string(),
        ],
        Duration::from_millis(4000),
    );
    let text = answer.expect("a tool that prints a lot still answers");
    assert_eq!(text.len(), 40000, "every byte arrived, not one pipe buffer");
}
