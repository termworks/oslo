use super::*;

fn item(kind: &str, name: &str, tags: &[&str]) -> Item {
    Item {
        kind: kind.to_string(),
        name: name.to_string(),
        first: format!("echo {name}"),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        created: 1_000,
        active: true,
        session_off: false,
        stored: true,
        clash: false,
    }
}

/// A recorder standing in for the database, so the key loop can be driven without one.
#[derive(Default)]
struct Recorded(Vec<(String, Act)>);

impl Backing for Recorded {
    fn act(&mut self, item: &Item, act: Act) {
        self.0.push((item.key(), act));
    }
}

fn state(items: Vec<Item>) -> State {
    State::new(items, "")
}

#[test]
fn the_query_matches_every_column_on_the_row() {
    let mut state = state(vec![
        item("alias", "gs", &["git"]),
        item("script", "deploy", &["work"]),
    ]);
    state.query = "script".to_string();
    state.refilter();
    assert_eq!(state.shown.len(), 1, "the kind is part of the row");
    assert_eq!(state.shown[0].name, "deploy");

    state.query = "git".to_string();
    state.refilter();
    assert_eq!(state.shown[0].name, "gs", "and so is a tag");
}

/// **Left and Right are the kind**, which is the division a list of five kinds is navigated by —
/// and the one that matters once the inherited side carries every variable this shell has.
#[test]
fn the_arrows_move_through_the_kinds_in_use() {
    let mut state = state(vec![
        item("alias", "a", &[]),
        item("script", "b", &[]),
        item("var", "c", &[]),
    ]);
    assert_eq!(state.kind(), None, "all of them, to start");
    assert_eq!(state.shown.len(), 3);

    state.next_kind();
    assert_eq!(state.kind().as_deref(), Some("alias"));
    assert_eq!(state.shown.len(), 1);
    assert_eq!(state.shown[0].name, "a");

    state.next_kind();
    assert_eq!(state.kind().as_deref(), Some("script"));

    state.next_kind();
    assert_eq!(state.kind().as_deref(), Some("var"));

    state.next_kind();
    assert_eq!(state.kind(), None, "round the loop");

    state.previous_kind();
    assert_eq!(
        state.kind().as_deref(),
        Some("var"),
        "and back the other way"
    );
}

/// **A tag is asked for in the query**, in any order and with or without anything else — so there is
/// no second key, and the arrows are free for the kind.
#[test]
fn a_hash_in_the_query_asks_for_a_tag() {
    let mut state = state(vec![
        item("alias", "gs", &["git"]),
        item("alias", "gp", &["git"]),
        item("alias", "ll", &["system"]),
    ]);

    state.query = "#git".to_string();
    state.refilter();
    assert_eq!(state.shown.len(), 2, "everything with the tag");

    for query in ["#git gs", "gs #git"] {
        state.query = query.to_string();
        state.refilter();
        assert_eq!(state.shown.len(), 1, "{query}");
        assert_eq!(state.shown[0].name, "gs");
    }

    // The tag is not matched as text: `#git` finds the tag, not a body that says "git".
    state.query = "#nothing".to_string();
    state.refilter();
    assert!(state.shown.is_empty());
}

/// And it is scoped by the kind, because the kind filter runs first.
#[test]
fn a_tag_is_asked_for_within_the_kind_on_screen() {
    let mut state = state(vec![
        item("alias", "gs", &["git"]),
        item("script", "deploy", &["git"]),
    ]);

    state.query = "#git".to_string();
    state.refilter();
    assert_eq!(state.shown.len(), 2, "both kinds, in the everything view");

    state.next_kind();
    assert_eq!(state.kind().as_deref(), Some("alias"));
    assert_eq!(state.shown.len(), 1, "only this kind's");
    assert_eq!(state.shown[0].name, "gs");
}

/// Tab is the source, and an untagged list has nothing but "all" to move through.
#[test]
fn tab_moves_between_the_database_and_the_config() {
    let mut config = item("alias", "ll", &[]);
    config.stored = false;
    let mut state = state(vec![item("alias", "gs", &["git"]), config]);

    assert_eq!(state.source, Source::Stored);
    assert_eq!(state.shown.len(), 1);
    assert_eq!(state.shown[0].name, "gs");

    state.next_source();
    assert_eq!(state.source, Source::Elsewhere);
    assert_eq!(state.shown[0].name, "ll");
    assert_eq!(state.kind(), None, "the old source’s kind does not follow");
}

/// **An inherited row is not editable and not deletable**, because there is no macro behind it —
/// only an alias your config defined or a variable your profile exported. Enter used to open an
/// editor and store a *new* macro shadowing it, which is not what the key looks like it does.
#[test]
fn an_inherited_row_cannot_be_edited_or_forgotten() {
    let mut backing = Recorded::default();
    let mut config = item("var", "EDITOR", &[]);
    config.stored = false;
    let mut state = state(vec![config]);
    state.next_source();
    assert_eq!(state.source, Source::Elsewhere);
    assert_eq!(state.shown.len(), 1);

    state.forget_selected(&mut backing);
    assert!(
        backing.0.is_empty(),
        "something was removed from a database it was never in: {:?}",
        backing.0
    );
    assert_eq!(state.shown.len(), 1, "and the row is still there");

    // A stored one, for contrast, goes.
    let mut stored = State::new(vec![item("alias", "gs", &[])], "");
    stored.forget_selected(&mut backing);
    assert_eq!(backing.0, [("alias/gs".to_string(), Act::Forget)]);
}

/// One space is the session, and it toggles.
#[test]
fn a_space_turns_it_off_for_the_session_and_a_second_puts_it_back() {
    let mut backing = Recorded::default();
    let mut state = state(vec![item("alias", "gs", &[])]);

    state.press_space(&mut backing);
    assert!(state.shown[0].session_off);
    state.press_space(&mut backing);
    assert!(!state.shown[0].session_off, "the second press undoes it");

    assert_eq!(
        backing.0,
        [
            ("alias/gs".to_string(), Act::Session(true)),
            ("alias/gs".to_string(), Act::Session(false)),
        ]
    );
}

/// **Three is one change, not three.** The third press puts the session state back where the burst
/// found it and turns the macro off everywhere instead.
#[test]
fn three_spaces_turn_it_off_everywhere_and_leave_the_session_alone() {
    let mut backing = Recorded::default();
    let mut state = state(vec![item("alias", "gs", &[])]);

    state.press_space(&mut backing);
    state.press_space(&mut backing);
    state.press_space(&mut backing);

    assert!(!state.shown[0].active, "off everywhere");
    assert!(
        !state.shown[0].session_off,
        "and the session is where it started"
    );
    assert_eq!(
        backing.0.last(),
        Some(&("alias/gs".to_string(), Act::Everywhere(false)))
    );

    // And three more turn it back on, which is how you undo it.
    state.press_space(&mut backing);
    state.press_space(&mut backing);
    state.press_space(&mut backing);
    assert!(state.shown[0].active);
}

/// A burst is per row: moving to another one starts the count again.
#[test]
fn spaces_on_two_different_rows_are_two_bursts() {
    let mut backing = Recorded::default();
    let mut state = state(vec![item("alias", "a", &[]), item("alias", "b", &[])]);

    state.press_space(&mut backing);
    state.selected = 1;
    state.press_space(&mut backing);
    state.press_space(&mut backing);

    assert!(
        state.shown.iter().all(|row| row.active),
        "no row reached three of its own"
    );
}

#[test]
fn forgetting_takes_the_row_out_and_keeps_the_cursor_where_the_eye_was() {
    let mut backing = Recorded::default();
    let mut state = state(vec![
        item("alias", "a", &[]),
        item("alias", "b", &[]),
        item("alias", "c", &[]),
    ]);
    state.selected = 1;
    state.forget_selected(&mut backing);

    assert_eq!(state.shown.len(), 2);
    assert_eq!(state.selected, 1, "not thrown back to the top");
    assert_eq!(backing.0, [("alias/b".to_string(), Act::Forget)]);
}

/// The badge says what you are looking at, in the finder's own words.
#[test]
fn the_source_and_the_tag_both_have_a_label() {
    assert_eq!(Source::Stored.label(), "[stored]");
    assert_eq!(Source::Stored.other(), Source::Elsewhere);
    assert!(Source::Elsewhere.holds(&Item {
        stored: false,
        ..item("alias", "x", &[])
    }));
}

/// Off is off however it was turned off, which is what the row's colour is asking.
#[test]
fn a_row_is_on_only_when_both_switches_say_so() {
    let mut row = item("alias", "x", &[]);
    assert!(row.on());
    row.session_off = true;
    assert!(!row.on());
    row.session_off = false;
    row.active = false;
    assert!(!row.on());
}
