//! The kind names, and a real watch over a real directory.

use super::super::util::probe;
use super::*;

#[test]
fn no_list_means_every_kind_this_module_names() {
    let all = wanted(None).expect("a default");
    for (_, kinds) in KINDS {
        for kind in *kinds {
            assert!(all.contains(kind), "the default left out {kind:?}");
        }
    }
}

#[test]
fn a_kind_that_is_not_one_says_what_they_are() {
    let mut asked = Table::new();
    asked.set(Value::int(1), Value::str("modfy"));
    let refused = wanted(Some(&Value::table(asked))).expect_err("should refuse");
    let said = refused.to_string();
    assert!(said.contains("modfy"), "{said}");
    // The message lists them, so a typo is one read away from being fixed.
    assert!(said.contains("write") && said.contains("create"), "{said}");
}

#[test]
fn the_named_kinds_are_the_flags_asked_for() {
    let mut asked = Table::new();
    asked.set(Value::int(1), Value::str("write"));
    asked.set(Value::int(2), Value::str("delete"));
    let kinds = wanted(Some(&Value::table(asked))).expect("valid");
    assert!(kinds.contains(&EventKind::Write));
    assert!(kinds.contains(&EventKind::Delete));
    assert!(!kinds.contains(&EventKind::Read));
}

/// **`write` is `IN_CLOSE_WRITE`, not `IN_MODIFY`**, and the distinction is the whole reason the
/// names are oslo's rather than inotify's: `IN_MODIFY` fires per `write(2)`, so one save of a large
/// file arrives as a burst and a caller counting saves counts wrong.
#[test]
fn write_means_saved_rather_than_written_to() {
    let mut asked = Table::new();
    asked.set(Value::int(1), Value::str("write"));
    let kinds = wanted(Some(&Value::table(asked))).expect("valid");
    assert!(kinds.contains(&EventKind::Write));
    assert!(!kinds.contains(&EventKind::Modify));
}

#[test]
fn a_change_is_seen_and_says_what_and_where() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut fs = Table::new();
    install(&mut fs);
    let fs = Value::table(fs);

    let watch = probe::first(
        &probe::field(&fs, "watch"),
        vec![Value::str(dir.path().to_string_lossy())],
    );
    assert!(matches!(watch, Value::Table(_)), "no handle");

    // Nothing has happened, and asking must answer at once rather than blocking.
    assert!(matches!(drain(&watch), Value::Nil), "an empty watch waited");

    std::fs::write(dir.path().join("note.txt"), "hello").expect("write");

    // The kernel is not obliged to have the event ready the instant `write` returns.
    let change = (0..50)
        .find_map(|_| match drain(&watch) {
            Value::Nil => {
                std::thread::sleep(std::time::Duration::from_millis(10));
                None
            }
            found => Some(found),
        })
        .expect("no change arrived within half a second");

    let Value::Table(change) = change else {
        panic!("a change is not a table")
    };
    let change = change.borrow();
    match change.get_str("name") {
        Value::Str(name) => assert_eq!(name.as_ref(), "note.txt"),
        other => panic!("name is {}", other.type_name()),
    }
    match change.get_str("path") {
        Value::Str(path) => assert_eq!(path.as_ref(), dir.path().to_string_lossy()),
        other => panic!("path is {}", other.type_name()),
    }
    assert!(matches!(change.get_str("kind"), Value::Str(_)));
    assert!(matches!(change.get_str("directory"), Value::Bool(false)));
}

/// **A burst is queued, not collapsed.** One `read_events` returns everything buffered, and the
/// iterator hands over one at a time — without the queue, nineteen of twenty saves would be lost.
#[test]
fn several_changes_at_once_are_all_delivered() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut fs = Table::new();
    install(&mut fs);
    let fs = Value::table(fs);

    let mut asked = Table::new();
    asked.set(Value::int(1), Value::str("create"));
    let watch = probe::first(
        &probe::field(&fs, "watch"),
        vec![
            Value::str(dir.path().to_string_lossy()),
            Value::table(asked),
        ],
    );

    for n in 0..5 {
        std::fs::write(dir.path().join(format!("f{n}")), "x").expect("write");
    }

    let mut seen = Vec::new();
    for _ in 0..100 {
        match drain(&watch) {
            Value::Nil if seen.len() >= 5 => break,
            Value::Nil => std::thread::sleep(std::time::Duration::from_millis(10)),
            Value::Table(change) => {
                if let Value::Str(name) = change.borrow().get_str("name") {
                    seen.push(name.to_string());
                }
            }
            other => panic!("a change is {}", other.type_name()),
        }
    }
    seen.sort();
    assert_eq!(
        seen,
        ["f0", "f1", "f2", "f3", "f4"],
        "a burst was collapsed"
    );
}

/// Closing releases the kernel's watch, and the handle refuses afterwards.
#[test]
fn a_closed_watch_refuses() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut fs = Table::new();
    install(&mut fs);
    let fs = Value::table(fs);
    let watch = probe::first(
        &probe::field(&fs, "watch"),
        vec![Value::str(dir.path().to_string_lossy())],
    );

    probe::method(&watch, "close", Vec::new()).expect("close");
    let refused = probe::method(&watch, "path", Vec::new()).expect_err("still open");
    assert!(refused.to_string().contains("closed"), "{refused}");
}

#[test]
fn explicit_close_releases_the_descriptor_immediately() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut source = EventSource::open().expect("source");
    source.add(dir.path()).expect("watch");
    let watching = Rc::new(Watching {
        source: RefCell::new(Some(source)),
        path: dir.path().display().to_string(),
        wanted: wanted(None).expect("kinds"),
        pending: RefCell::new(VecDeque::new()),
    });
    let watch = handle(Rc::clone(&watching));
    probe::method(&watch, "close", Vec::new()).expect("close");
    assert!(watching.source.borrow().is_none());
    assert!(
        matches!(watch, Value::Table(_)),
        "the handle remains reachable"
    );
}

#[test]
fn a_directory_that_is_not_there_is_a_message() {
    let mut fs = Table::new();
    install(&mut fs);
    let fs = Value::table(fs);
    let answered = probe::call(
        &probe::field(&fs, "watch"),
        vec![Value::str("/nowhere/at/all")],
    )
    .expect("should answer rather than raise");
    assert!(matches!(answered.first(), Some(Value::Nil)));
    assert!(answered.len() > 1, "no message alongside the nil");
}

/// Call the handle, which is what a generic `for` does.
fn drain(watch: &Value) -> Value {
    let Value::Table(table) = watch else {
        panic!("not a handle")
    };
    let called = table
        .borrow()
        .metatable
        .clone()
        .expect("no metatable")
        .borrow()
        .get_str("__call");
    probe::first(&called, vec![watch.clone()])
}
