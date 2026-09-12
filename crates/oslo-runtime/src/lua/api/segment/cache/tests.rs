//! What may be reused, and what may not.
//!
//! One test: the content generation is process-wide and the cache is thread-local, so two test
//! functions would each see the other's bumps and neither would see the other's entries.

use super::*;

fn made(text: &str) -> Rendered {
    Rendered {
        name: "seg".to_string(),
        priority: 50,
        text: text.to_string(),
        width: text.chars().count(),
    }
}

#[test]
fn a_segment_is_reused_until_it_has_something_new_to_say() {
    let _serial = crate::serial::generation();
    forget();

    // Nothing kept yet.
    assert!(reuse("prompt.left", "seg", None).is_none());

    keep("prompt.left", "seg", &made("main"));
    assert_eq!(
        reuse("prompt.left", "seg", None).map(|r| r.text),
        Some("main".to_string()),
        "a segment with nothing to wait for is reused for ever"
    );

    // **The two prompts are two segments.** One key would have `cwd` on the left and `cwd` on the
    // right overwrite each other every frame, which is slower than not caching at all.
    assert!(
        reuse("prompt.right", "seg", None).is_none(),
        "the other prompt's cwd is not this one's"
    );

    // **A real change drops everything.** The branch moved, the directory moved, a variable moved —
    // nothing kept is trustworthy, whatever interval it asked for.
    oslo_ui::prompt::invalidate();
    assert!(
        reuse("prompt.left", "seg", None).is_none(),
        "invalidate means the content is stale"
    );

    // An interval that has not elapsed keeps the entry; one that has, drops it.
    keep("prompt.left", "seg", &made("|"));
    assert!(
        reuse("prompt.left", "seg", Some(60_000)).is_some(),
        "a minute has not passed"
    );
    assert!(
        reuse("prompt.left", "seg", Some(0)).is_none(),
        "no time at all has certainly passed"
    );

    // **An unnamed segment is never cached.** Two of them would share one entry and take turns
    // overwriting it, and the second would be drawn where the first belongs.
    keep("prompt.left", "", &made("x"));
    assert!(reuse("prompt.left", "", None).is_none());

    forget();
    assert!(reuse("prompt.left", "seg", None).is_none());
}
