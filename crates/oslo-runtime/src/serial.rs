//! One lock, for the tests that share process-wide prompt state.
//!
//! A crate's tests are one binary and run in parallel by default. Most of these are independent;
//! the ones that touch the prompt's *content generation* are not. That generation is a single
//! `AtomicU64` in `oslo_ui::prompt`, `invalidate()` bumps it, and a cache test's whole subject is
//! whether a reading it took a moment ago still holds. Another test calling `invalidate()` in
//! between answers that question for it — wrongly, and only sometimes.
//!
//! It is a real failure and it looked like a mystery: `nothing_is_re_run_until_the_content_could
//! _have_changed` passed alone, passed five times in a row over its own crate, and failed once in a
//! whole-workspace run, on an assertion about code that had not changed.
//!
//! The same hazard, and the same fix, as `oslo_ui::marks`: one at a time.

/// Held by every test that reads or moves the prompt's content generation.
static GENERATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take the lock, ignoring a poisoning left by an unrelated test that panicked while holding it.
///
/// A panic elsewhere is already a failure being reported; turning it into a second failure in every
/// test that runs afterwards buries the first one.
pub(crate) fn generation() -> std::sync::MutexGuard<'static, ()> {
    GENERATION.lock().unwrap_or_else(|held| held.into_inner())
}
