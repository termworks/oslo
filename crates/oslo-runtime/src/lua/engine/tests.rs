//! What a prompt key is called with.
//!
//! These exist because [`super::LuaEngine::render_with`] had **no coverage at all**, and the gap it
//! left was not subtle: a plain function was called with no arguments, so the documented shape
//! `oslo.prompt.left = function(p) return p.cwd end` saw `p` as nil and raised on the first index.
//! Every prompt in the README was written that way. A single assertion here would have caught it.

use super::*;

fn engine_with(source: &str) -> LuaEngine {
    let lua = LuaEngine::new().expect("an interpreter");
    lua.setup_bindings(Arc::new(Mutex::new(Environment::new())))
        .expect("the oslo table");
    lua.eval_script(source).expect("the chunk must run");
    lua
}

fn facts() -> Context {
    Context {
        status: 3,
        cwd: "/srv/app".to_string(),
        branch: Some("main".to_string()),
        duration_ms: Some(250),
        jobs: 2,
        ..Context::default()
    }
}

/// The whole point: a function receives the facts as its argument.
#[test]
fn a_prompt_function_is_handed_the_context() {
    let lua = engine_with("oslo.prompt.left = function(p) return p.cwd .. '@' .. p.branch end");
    assert_eq!(
        lua.render_with("prompt.left", &facts()).as_deref(),
        Some("/srv/app@main")
    );
}

/// Every key resolves the same way. `transient` and `title` are the two newest and the two most
/// likely to be wired up differently by accident.
#[test]
fn every_prompt_key_gets_the_same_facts() {
    for key in ["left", "right", "continuation", "transient", "title"] {
        let lua = engine_with(&format!(
            "oslo.prompt.{key} = function(p) return tostring(p.status) .. ':' .. tostring(p.jobs) end"
        ));
        assert_eq!(
            lua.render_with(&format!("prompt.{key}"), &facts())
                .as_deref(),
            Some("3:2"),
            "prompt.{key} was not handed the facts"
        );
    }
}

/// A function that names no parameter still works — Lua discards arguments it does not take. This
/// is what keeps the fix from breaking every config written before it.
#[test]
fn a_function_taking_no_argument_is_unaffected() {
    let lua = engine_with("oslo.prompt.left = function() return 'static' end");
    assert_eq!(
        lua.render_with("prompt.left", &facts()).as_deref(),
        Some("static")
    );
}

/// A plain string is a prompt too, and is not called at all.
#[test]
fn a_string_is_used_verbatim() {
    let lua = engine_with("oslo.prompt.left = '$ '");
    assert_eq!(
        lua.render_with("prompt.left", &facts()).as_deref(),
        Some("$ ")
    );
}

/// `command` is what tells a title whether something is running. It is absent at a prompt, which
/// is the difference `oslo.prompt.title` branches on.
#[test]
fn the_command_fact_is_present_only_while_one_runs() {
    let lua = engine_with(
        "oslo.prompt.title = function(p) return p.command and ('run:' .. p.command) or 'idle' end",
    );
    assert_eq!(
        lua.render_with("prompt.title", &facts()).as_deref(),
        Some("idle")
    );
    let running = Context {
        command: Some("cargo test".to_string()),
        ..facts()
    };
    assert_eq!(
        lua.render_with("prompt.title", &running).as_deref(),
        Some("run:cargo test")
    );
}

/// A key nobody set renders nothing, which is what makes every one of them optional.
#[test]
fn an_unset_key_renders_nothing() {
    let lua = engine_with("");
    assert_eq!(lua.render_with("prompt.title", &facts()), None);
}

/// **A tick redraws the prompt; it does not rebuild it.** This is the whole economics of an
/// animated prompt: without the cache, a spinner asking for ten frames a second would drag every
/// other segment through `git` ten times a second with it.
///
/// Two segments, one animated and one not, rendered three times over with no real change in
/// between. The animated one runs each time; the other runs once.
#[test]
fn only_an_animated_segment_re_runs_between_real_changes() {
    let _serial = crate::serial::generation();
    crate::lua::api::segment::cache::forget();
    let lua = engine_with(
        r#"
        spun, still = 0, 0
        oslo.prompt.left = {
          oslo.segment({ name = "spin", every = 60, render = function()
            spun = spun + 1
            return { { text = "s" .. spun } }
          end }),
          oslo.segment({ name = "still", render = function()
            still = still + 1
            return { { text = "|" .. still } }
          end }),
        }
        "#,
    );

    let mut drawn = Vec::new();
    for _ in 0..3 {
        drawn.push(lua.render_with("prompt.left", &facts()).unwrap_or_default());
        // Past the animated segment's interval, and nothing else has changed.
        std::thread::sleep(std::time::Duration::from_millis(70));
    }

    assert_eq!(
        drawn,
        vec!["s1|1".to_string(), "s2|1".to_string(), "s3|1".to_string()],
        "the spinner moved on every pass and the other segment ran once"
    );

    // **And a real change drops the lot.** `invalidate` means the directory or the branch moved, so
    // a cached segment is not a cheap answer — it is a wrong one.
    oslo_ui::prompt::invalidate();
    assert_eq!(
        lua.render_with("prompt.left", &facts()).as_deref(),
        Some("s4|2"),
        "both ran again"
    );
    crate::lua::api::segment::cache::forget();
}
