use super::*;

/// **This process is running, so it must be in the list.** The one assertion that cannot be true by
/// accident, and the one that catches `/proc` being read wrongly.
#[test]
fn the_running_process_is_among_the_pids() {
    let me = std::process::id().to_string();
    let all = pids();
    let mine = all.iter().find(|one| one.value == me);
    let mine = mine.unwrap_or_else(|| panic!("this process ({me}) is not in {} pids", all.len()));
    assert_eq!(mine.kind, "pid");
    assert!(!mine.note.is_empty(), "a pid should say what it is running");
}

/// Every value is a number, because a signal goes to a pid and `/proc` holds other things too.
#[test]
fn nothing_but_numbers_is_offered_as_a_pid() {
    for one in pids() {
        assert!(
            one.value.parse::<u32>().is_ok(),
            "{} is not a pid",
            one.value
        );
    }
}

/// Newest first: the thing you are killing is nearly always the thing you just started.
#[test]
fn the_newest_process_comes_first() {
    let all = pids();
    let numbers: Vec<u32> = all
        .iter()
        .filter_map(|one| one.value.parse().ok())
        .collect();
    let mut sorted = numbers.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(numbers, sorted);
}

/// **Named without the `SIG`**, because that is what `kill -s` and `trap` take.
#[test]
fn signals_are_named_the_way_kill_takes_them() {
    let all = signals();
    let term = all.iter().find(|one| one.value == "TERM").expect("TERM");
    assert_eq!(term.note, "15");
    assert_eq!(term.kind, "signal");
    assert!(all.iter().any(|one| one.value == "KILL" && one.note == "9"));
    assert!(all.iter().any(|one| one.value == "HUP" && one.note == "1"));
    assert!(
        !all.iter().any(|one| one.value.starts_with("SIG")),
        "the SIG prefix is not what kill -s takes"
    );
}
