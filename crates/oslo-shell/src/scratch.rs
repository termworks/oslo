//! Named sessions that keep running when the terminal they were opened in goes away.
//!
//! One key reaches all of it — `^\` by default — and it means the same thing in a scratch and out of
//! one: show the finder. What the finder is asked for is the only difference.

pub mod backend;
pub mod client;
pub mod daemon;
pub mod detach;
pub mod dir;
pub mod enter;
pub mod keeper;
pub mod log;
pub mod name;
#[cfg(feature = "watch")]
pub mod program;
pub mod store;
pub mod wire;

/// Point the scratch directory at a scratch path for the duration of one test.
///
/// The directory is **not** created: `dir`'s tests are about creating it, and everything else
/// calls `dir::open_checked` first, which is what the keeper does anyway.
///
/// **One lock for every test in this subtree, not one per module.** `OSLO_SCRATCH_DIR` is
/// process-wide, so two modules each holding their own mutex do not exclude each other at all —
/// which showed up exactly as it always does, as one test sweeping another's directory and a
/// failure that made no sense where it was reported.
#[cfg(test)]
pub(crate) fn scratch() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = tempfile::tempdir().expect("tempdir");
    // SAFETY: the lock above is the only thing that makes this sound, and every test that reads or
    // writes the variable takes it.
    unsafe { std::env::set_var("OSLO_SCRATCH_DIR", dir.path().join("scratch")) };
    (dir, guard)
}
