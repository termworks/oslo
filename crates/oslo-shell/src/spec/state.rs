//! What only this shell knows: its jobs, its aliases, its functions.
//!
//! ```text
//!   fg %⇥        %1   cargo build      running   job
//!   unalias ⇥    ll   ls -l            alias
//! ```
//!
//! # Why these cannot be sources like the others
//!
//! [`oslo_ui::spec::sources`] answers from files, so it needs nothing but the filesystem and lives
//! below the shell. These three are the shell's own state, held in memory in this process, and
//! there is no file to read them out of.
//!
//! They cannot go through `$(…)` either. **Every macro runs as a child** — see [`super::run`] — and
//! a child gets a fresh `Environment` that has never seen this session's aliases, and a job table
//! belonging to nobody. `$(jobs)` would answer honestly and emptily.
//!
//! So they ride the macro hook instead, answered here in the shell's own process before the name is
//! treated as a command to run.

use oslo_ui::shell::Shell;
use oslo_ui::spec::action::Offer;

/// What `$name` offers from this shell, or `None` if it is not one of these.
///
/// `None` rather than an empty list, for the same reason as in [`oslo_ui::spec::sources`]: a shell
/// with no jobs has answered, and a name that is not shell state has not.
pub fn offers(name: &str, env: &std::sync::Mutex<crate::env::Environment>) -> Option<Vec<Offer>> {
    Some(match name {
        "jobs" => jobs(),
        "aliases" => aliases(env),
        "functions" => functions(env),
        _ => return None,
    })
}

/// The jobs this shell is managing, as the `%n` that names them.
///
/// **`%1` and not `1`**, because `%1` is what `fg`, `bg`, `wait` and `kill` take — a bare number is
/// a pid to all four, which is a different job or no job at all.
fn jobs() -> Vec<Offer> {
    use crate::exec::job::{JobState, with_jobs};
    with_jobs(|table| {
        table
            .jobs()
            .iter()
            .map(|job| {
                let state = match job.state {
                    JobState::Running => "running",
                    JobState::Stopped => "stopped",
                    JobState::Completed(_) => "done",
                };
                Offer {
                    value: format!("%{}", job.id),
                    description: Some(format!("{}  {state}", job.command)),
                    tag: Some("job".to_string()),
                }
            })
            .collect()
    })
}

/// Every alias, with what it expands to.
///
/// The expansion is the note because it is the one thing the name does not tell you, and the reason
/// `unalias <Tab>` is worth having at all: the name you cannot place is the one you want to remove.
fn aliases(env: &std::sync::Mutex<crate::env::Environment>) -> Vec<Offer> {
    let env = env.lock().unwrap_or_else(|held| held.into_inner());
    let mut found: Vec<Offer> = env
        .aliases()
        .iter()
        .map(|(name, target)| Offer {
            value: name.clone(),
            description: Some(target.clone()),
            tag: Some("alias".to_string()),
        })
        .collect();
    found.sort_unstable_by(|a, b| a.value.cmp(&b.value));
    found
}

/// Every shell function defined in this session.
fn functions(env: &std::sync::Mutex<crate::env::Environment>) -> Vec<Offer> {
    let env = env.lock().unwrap_or_else(|held| held.into_inner());
    let mut found: Vec<Offer> = env
        .functions()
        .keys()
        .map(|name| Offer {
            value: name.clone(),
            description: None,
            tag: Some("function".to_string()),
        })
        .collect();
    found.sort_unstable_by(|a, b| a.value.cmp(&b.value));
    found
}

#[cfg(test)]
#[path = "state/tests.rs"]
mod tests;
