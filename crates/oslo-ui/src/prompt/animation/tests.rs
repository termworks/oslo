//! The clock and the two counters.
//!
//! One test again: the deadline is thread-local but the content counter is not, and a second test
//! function bumping it on another thread would make the first one's cache look stale.

use super::*;

#[test]
fn a_tick_is_due_once_and_never_says_the_content_changed() {
    settle();
    assert!(!tick_due(), "nothing has been asked for");

    // **A tick moves one counter and not the other.** The whole point: a spinner redraws the prompt
    // without telling a cache that the branch name it is holding has gone stale.
    let content = content_generation();
    schedule(Duration::from_millis(0));
    assert!(tick_due(), "the moment has come");
    assert_eq!(content_generation(), content, "a tick changes no content");

    // **And it is due once.** Re-arming is rendering's job, so a segment that stops asking stops
    // being asked and the editor goes back to blocking on a key.
    assert!(!tick_due(), "the deadline was cleared by asking");

    // The nearest deadline wins, so several segments at different speeds share one timer.
    schedule(Duration::from_secs(30));
    schedule(Duration::from_millis(0));
    assert!(tick_due(), "the sooner of the two");

    // A real change is the other counter, and that one a cache does watch.
    let before = content_generation();
    content_changed();
    assert_ne!(content_generation(), before);

    settle();
    assert!(!tick_due());
}
