use super::*;
use crate::env::Environment;
use std::sync::Mutex;

fn env() -> Mutex<Environment> {
    Mutex::new(Environment::new())
}

/// The three names answer, and nothing else does — a name that is not shell state has to fall
/// through to [`super::super::run`], or `$(git branch)` stops working.
#[test]
fn only_shell_state_is_answered_here() {
    let held = env();
    for name in ["jobs", "aliases", "functions"] {
        assert!(offers(name, &held).is_some(), "${name} is not answered");
    }
    for name in ["", "bash", "pids", "git branch"] {
        assert!(offers(name, &held).is_none(), "${name} was swallowed");
    }
}

/// **`%1`, not `1`.** A bare number is a pid to `fg`, `bg`, `wait` and `kill` alike, which is a
/// different job or no job at all.
#[test]
fn a_job_is_offered_as_the_percent_that_names_it() {
    for one in jobs() {
        assert!(one.value.starts_with('%'), "{}", one.value);
        assert_eq!(one.tag.as_deref(), Some("job"));
    }
}

/// An alias carries what it expands to, which is the one thing its name does not say — and the
/// reason `unalias <Tab>` is worth having.
#[test]
fn an_alias_carries_its_expansion() {
    let held = env();
    {
        let mut inner = held.lock().unwrap();
        inner.set_alias("ll", "ls -l");
    }
    let found = aliases(&held);
    let one = found
        .iter()
        .find(|one| one.value == "ll")
        .expect("the alias");
    assert_eq!(one.description.as_deref(), Some("ls -l"));
    assert_eq!(one.tag.as_deref(), Some("alias"));
}

/// A fresh shell has no functions, and that is an empty answer rather than no answer.
#[test]
fn an_empty_shell_still_answers() {
    let held = env();
    assert_eq!(functions(&held).len(), 0);
    assert!(offers("functions", &held).is_some());
}
