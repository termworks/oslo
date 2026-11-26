//! The stored macros, brought into a shell — at startup, and again whenever they change.
//!
//! # After the config, deliberately
//!
//! Three sources define an alias: `alias` in a script, `oslo.alias` in `init.lua`, and
//! `oslo macros add`. The ordinary shell rule is that the last definition wins, and this applies it
//! to sources: **the database is applied after the configuration, so the database wins.**
//!
//! That is the deliberate half. The database is the one you can change without editing a file, so
//! `oslo macros add gco …` taking effect is what you asked for; a config that has to be edited and
//! re-sourced would be the more surprising winner. The cost is that a stored entry can shadow one
//! you wrote in `init.lua` — which is why what the config defined is written out here, so that the
//! manager can show you both, and so that *removing* the stored one puts the configured one back
//! rather than leaving a hole.
//!
//! # From the snapshot, not the database
//!
//! Measured: opening the database is 2.6 ms against an 842 µs shell start, and reading the flat file
//! is 3.6 µs. See `oslo_base::macros`. A shell that opened a second key-value store before its
//! first prompt would be paying three times its whole startup for something a `read(2)` answers.
//!
//! # And only where the config is read
//!
//! This runs from the interactive loop, beside `load_config`, and from nowhere else — so a script or
//! an `oslo -c` sees none of it. That is not a new restriction: `init.lua` is read by this loop and
//! by nothing else either, so a non-interactive shell has never had aliases to expand.

use oslo_base::macros::live::{Applied, Stamps};
use oslo_base::macros::{Entry, Kind};
use oslo_shell::env::Environment;
use oslo_shell::env::announce::{Change, Scope, Source, announce};
use oslo_shell::env::lists;
use std::sync::{Arc, Mutex};

/// What this shell was last given, so the next rebuild knows what is its to take away.
pub(super) struct Held {
    applied: Applied,
    stamps: Stamps,
}

/// Apply what `oslo macros` has stored, having first written down what the config defined.
pub(super) fn install(env: &Arc<Mutex<Environment>>) -> Held {
    // **Written before anything is applied over it**, which is the only moment the configuration's
    // own aliases can be told apart from everybody else's.
    publish_what_the_config_defined(env);
    // A session file outlives the shell that wrote it; a starting shell is where they get tidied.
    oslo_base::macros::live::session::sweep();
    scripts_on_path(env);

    let wanted = oslo_base::macros::live::want();
    let applied = Applied::of(&wanted);
    // Nothing is announced at startup: a shell learning what the store holds is not a change.
    apply(env, &wanted, &Applied::default(), &applied, false);
    Held {
        applied,
        stamps: Stamps::now(),
    }
}

/// Put the scripts directory at the front of `$PATH` when it exists and `$PATH` does not hold it.
fn scripts_on_path(env: &Arc<Mutex<Environment>>) {
    let Some(dir) = oslo_base::macros::scripts::directory().filter(|dir| dir.is_dir()) else {
        return;
    };
    let dir = dir.to_string_lossy().into_owned();
    let Ok(mut guard) = env.lock() else {
        return;
    };
    let base = std::path::Path::new("/");
    if !lists::contains(&guard, "PATH", &dir, base) {
        lists::prepend(&mut guard, "PATH", &[dir], base);
    }
}

/// Bring the shell back in step if either file has moved since the last prompt.
///
/// Two `stat`s and, almost always, nothing else: a change made in one terminal has to reach the one
/// beside it, and reading the files every prompt to find out would cost more than the feature is
/// worth.
///
/// Answers **whether anything moved**, which is what an idle prompt needs: a prompt is expanded
/// when it is built, not on every repaint, so a shell that woke, re-read and changed nothing must
/// not redraw — and one that did change something must.
pub(super) fn refresh(env: &Arc<Mutex<Environment>>, held: &mut Held) -> bool {
    let stamps = Stamps::now();
    if stamps == held.stamps {
        return false;
    }
    let wanted = oslo_base::macros::live::want();
    let applied = Applied::of(&wanted);
    // A refresh means the store moved since this shell last looked, which is a change worth a hook.
    apply(env, &wanted, &held.applied, &applied, true);
    *held = Held { applied, stamps };
    true
}

/// Take away what was ours and is not wanted any more, then put the wanted set in.
///
/// In that order, because a name in both lists must end up **set**: the other way round would add
/// `gs` and then remove it again for having been in yesterday's set too.
fn apply(
    env: &Arc<Mutex<Environment>>,
    wanted: &[Entry],
    had: &Applied,
    now: &Applied,
    announce_changes: bool,
) {
    let gone = had.gone(now);
    for name in &gone.abbrevs {
        oslo_ui::abbr::remove(name);
    }
    let Ok(mut guard) = env.lock() else {
        return;
    };
    for name in &gone.aliases {
        guard.remove_alias(name);
    }
    // The recipe, not the value: see `Applied::gone`. A variable already read is a variable like
    // any other by now, and unsetting it under a running command would be a different feature.
    for name in &gone.vars {
        guard.remove_lazy_var(name);
    }
    for entry in wanted {
        match entry.kind {
            Kind::Alias => guard.set_alias(&entry.name, &entry.body),
            Kind::Abbrev => oslo_ui::abbr::add(
                &entry.name,
                &entry.body,
                // **Whatever it already has, if it has one.**
                //
                // `want()` merges the macro database with `from_elsewhere` — and `from_elsewhere`
                // is the snapshot *this shell* wrote a few lines ago, describing what its own
                // config defined. So a config's `{ "| less", anywhere = true }` came straight back
                // here and was re-added as `Command`, and the flag was gone before the first
                // prompt. The config reader was right all along; this was the thief.
                //
                // A row that is genuinely only in the database has nothing in the table yet and
                // gets the default: `oslo macros` has no `--anywhere`, and command position is
                // what an abbreviation is for.
                oslo_ui::abbr::placement_of(&entry.name)
                    .unwrap_or(oslo_ui::abbr::Placement::Command),
            ),
            // **A value now; a recipe when somebody asks.**
            //
            // A plain value costs nothing, so it is exported here like any other environment
            // variable — which is also the only way a program that reads the environment *itself*
            // can find it, since nothing about `gh` or `aws` expands `$NAME` first.
            //
            // A body that runs a command is different in kind: doing that for every stored variable
            // at every prompt would decrypt everything you own to answer a question nobody asked.
            // The shell is handed the line and runs it the first time the name is read — see
            // `oslo_shell::expand::param::materialise`.
            //
            // **A stored variable wins, like every other kind here.** It used to skip a name that
            // already had a value, so `EDITOR` inherited from a terminal emulator could not be
            // changed by storing one — the command reported success and nothing happened. Alias and
            // Abbrev are its structural peers in this very loop and neither ever deferred, and the
            // `macros.sh` rendering of this same entry is an unguarded `export`, so the old rule
            // was not even consistent with itself. What you store is what you get.
            Kind::Var if oslo_base::macros::is_a_value(&entry.body) => {
                // **Announced only when the value actually moved.** This runs at every prompt, so
                // saying what it read rather than what changed would fire `on-variable-change`
                // continuously at an idle prompt — the store says the same thing each time.
                let arrived = guard.get_var(&entry.name) != Some(entry.body.trim());
                guard.set_var(&entry.name, entry.body.trim(), true);
                if arrived && announce_changes {
                    announce(
                        &entry.name,
                        Change::Set { exported: true },
                        Scope::Stored,
                        Source::Remote,
                    );
                }
            }
            Kind::Var => {
                // **The value goes first, or the recipe never runs.** `materialise` returns early
                // for a name that already has one, so a lazy body registered beside an inherited
                // value would sit there unread — the deference removed above, in a second place.
                guard.unset_var(&entry.name);
                guard.set_lazy_var(&entry.name, &entry.body);
            }
            // A function or a script is found after `$PATH` fails, so neither is in the snapshot
            // and neither belongs here. See `oslo_shell::exec::stored`.
            Kind::Func | Kind::Script => {}
        }
    }
}

/// Write down the aliases and abbreviations the *configuration* defined.
///
/// The manager's second source, and what makes a removal able to put the configured alias back.
/// Everyone else reads it from the file rather than by running Lua, because this is the only
/// process that has already run it.
/// Whether a variable is one somebody would look for in a list of what this shell has.
///
/// **Nearly all of them are**, so this is a short list of the ones that are not: the four the shell
/// rewrites as you move and type, which would be a different value every time the list is drawn and
/// tell you nothing about your setup. Everything else stays — including `PATH` and the long ones,
/// because a list that quietly hid what it judged uninteresting would be worth less than `env`.
fn worth_listing(name: &str) -> bool {
    !matches!(name, "_" | "PWD" | "OLDPWD" | "SHLVL")
}

fn publish_what_the_config_defined(env: &Arc<Mutex<Environment>>) -> Option<()> {
    let mut entries: Vec<Entry> = {
        let guard = env.lock().ok()?;
        guard
            .get_aliases()
            .iter()
            .map(|(name, body)| Entry::new(Kind::Alias, name, body))
            .collect()
    };
    entries.extend(
        oslo_ui::abbr::all()
            .into_iter()
            .map(|(name, abbr)| Entry::new(Kind::Abbrev, &name, &abbr.expansion)),
    );
    // **And every variable this shell has**, which is the other half of "what is defined that you
    // did not store". A stored `EDITOR` that never applies because a profile already exported one
    // is invisible until the two lists are on the same screen; this is that screen.
    if let Ok(guard) = env.lock() {
        entries.extend(
            guard
                .exported_vars()
                .into_iter()
                .filter(|(name, _)| worth_listing(name))
                .map(|(name, value)| Entry::new(Kind::Var, &name, &value)),
        );
    }
    entries.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    // Best effort: this is a list the manager draws. A shell that failed to start because it could
    // not write one would be absurd.
    oslo_base::macros::live::publish_elsewhere(&entries).ok()
}
