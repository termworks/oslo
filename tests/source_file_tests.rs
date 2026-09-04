//! The machine's own facts, from `/proc` and `/etc` to the Tab key.
//!
//! [`oslo_ui::spec::sources`] has unit tests for each source in isolation; this is the other half —
//! that a **shipped spec naming `$pids` actually reaches it**. The two ends are written in different
//! languages and joined by a string, so nothing but running it end to end can say that `kill 1<Tab>`
//! offers a process rather than a filename, or than nothing at all.
//!
//! Requires the `compgen` feature, which is where the spec reader lives.
#![cfg(feature = "compgen")]

use oslo::env::Environment;
use oslo::ui::OsloHelper;
use std::sync::{Arc, Mutex};

thread_local! {
    /// One environment for the whole test, shared by the editor and by the macro runner — which is
    /// how the shell holds it, and the only arrangement in which `$aliases` can see an alias the
    /// session set.
    static SESSION: Arc<Mutex<Environment>> = Arc::new(Mutex::new(Environment::new()));
}

fn offered(line: &str) -> Vec<String> {
    let mut helper = OsloHelper::new(SESSION.with(Arc::clone));
    helper.set_menu(false);
    let (_, found) = helper.candidates(line, line.len());
    found.into_iter().map(|one| one.display).collect()
}

/// One test for the whole path, because `$OSLO_COMPLETION` is process-wide: a second test setting it
/// would be taking turns with this one's environment.
///
/// Each case names a source, a shipped spec that points at it, and something the machine running the
/// test is guaranteed to have — this process's own pid, this process's own `PATH`, the root mount.
#[test]
fn a_shipped_spec_reaches_the_source_it_names() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("share/completion");
    if !dir.join("kill.yaml").exists() {
        return; // A checkout that has not run `scripts/completion.sh` has nothing to point at.
    }
    // SAFETY: read by `oslo_shell::spec::directories` and nothing else, and this is the only test
    // in this binary.
    unsafe { std::env::set_var("OSLO_COMPLETION", &dir) };
    oslo::ui::spec::custom::set_loader(Some(std::rc::Rc::new(oslo::spec::find)));

    let mine = std::process::id().to_string();
    assert!(
        offered("ps -t ").iter().any(|one| one.starts_with("pts/")),
        "$terminals did not reach `ps -t`"
    );
    assert!(
        offered("mount -t ").iter().any(|one| one == "proc"),
        "$filesystems did not reach `mount -t`"
    );
    assert!(
        offered("sysctl kernel.")
            .iter()
            .any(|one| one.starts_with("kernel.")),
        "$sysctls did not reach `sysctl`"
    );
    assert!(
        offered("rmmod ").len() > 1,
        "$modules offered nothing at all"
    );
    assert!(
        offered("chsh -s ").iter().any(|one| one.starts_with("/")),
        "$shells did not reach `chsh -s`"
    );
    assert!(
        offered("df ").iter().any(|one| one == "/"),
        "$mounts did not reach `df`"
    );
    assert!(
        offered("lsblk ").iter().all(|one| one.starts_with("/dev/")),
        "$devices offered something that is not a device"
    );
    assert!(
        offered("kill ").contains(&mine),
        "$pids did not offer this process"
    );
    assert!(
        offered("kill -s ").iter().any(|one| one == "TERM"),
        "$signals did not reach `kill -s`"
    );
    assert!(
        offered("unset ").iter().any(|one| one == "PATH"),
        "$variables did not reach `unset`"
    );
    assert!(
        offered("umount ").iter().any(|one| one == "/"),
        "$mounts did not offer the root filesystem"
    );
    assert!(
        !offered("chown ").is_empty(),
        "$users offered nobody at all"
    );

    // Shell state takes the macro hook rather than `sources`, because there is no file to read a
    // job table out of — so it needs the runner installed, exactly as the REPL installs it.
    let held = SESSION.with(Arc::clone);
    held.lock().unwrap().set_alias("ll", "ls -l");
    oslo::ui::spec::action::set_runner(Some(std::rc::Rc::new(
        move |name: &str, arg: &str, query: &_| {
            oslo::spec::state::offers(name, &held)
                .unwrap_or_else(|| oslo::spec::run::offers(name, arg, query))
        },
    )));
    assert!(
        offered("unalias ").iter().any(|one| one == "ll"),
        "$aliases did not reach `unalias`"
    );
    // A test process has no jobs, so the claim worth making is that `fg <Tab>` answers *emptily*
    // rather than falling back to filenames. An empty answer is the source having been consulted;
    // a directory listing would be it never having been reached.
    let no_jobs = offered("fg ");
    assert!(
        no_jobs.iter().all(|one| one.starts_with('%')),
        "$jobs offered something that is not a job: {no_jobs:?}"
    );
    assert!(
        offered("wait ").contains(&mine),
        "`wait` fell past $jobs without reaching $pids"
    );
    oslo::ui::spec::action::set_runner(None);

    oslo::ui::spec::custom::set_loader(None);
    oslo::ui::spec::custom::forget();
    unsafe { std::env::remove_var("OSLO_COMPLETION") };
}
