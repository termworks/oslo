//! What a config's `oslo` table turns into.

use super::*;
use oslo_luavm::{Engine, Host};

fn settings_from(source: &str) -> (Settings, Vec<String>) {
    let engine = Engine::new();
    engine
        .eval(source, "settings test")
        .expect("the test chunk must run");
    read_lua_settings(&engine.global("oslo"))
}

#[test]
fn naming_one_field_keeps_every_other_default() {
    let (settings, problems) = settings_from("oslo = { completion = { max_rows = 5 } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(settings.completion.max_rows, 5);
    assert!(settings.completion.descriptions);
    assert_eq!(settings.suggest, Suggest::default());
    assert_eq!(settings.history, History::default());
}

/// `false` and "not mentioned" have to be different, or nothing could ever be turned off.
#[test]
fn a_false_flag_is_not_the_same_as_an_absent_one() {
    let (off, _) = settings_from("oslo = { completion = { show_kind = false } }");
    assert!(!off.completion.show_kind);
    let (absent, _) = settings_from("oslo = { completion = {} }");
    assert!(absent.completion.show_kind);
}

/// An empty list is a real answer — "offer nothing" — and must not read as "offer everything".
#[test]
fn the_completion_sources_are_read_including_an_empty_list() {
    let (all, _) = settings_from("oslo = { completion = {} }");
    assert_eq!(all.completion.sh_sources, None, "unset means every kind");
    assert_eq!(all.completion.lua_sources, None);

    let (some, problems) =
        settings_from("oslo = { completion = { sh_sources = {'command', 'dir'} } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        some.completion.sh_sources.as_deref(),
        Some(["command".to_string(), "dir".to_string()].as_slice())
    );

    let (none, _) = settings_from("oslo = { completion = { sh_sources = {} } }");
    assert_eq!(none.completion.sh_sources.as_deref(), Some([].as_slice()));

    // **One kind filter per prompt.** The two prompts share no kind of candidate — the shell has
    // `command`, `file`, `dir`; Lua has `function`, `field`, `keyword` — so a single list could
    // only ever be right for one of them, and a shell filter silently emptied every Lua dropdown.
    let (lua, problems) =
        settings_from("oslo = { completion = { lua_sources = {'function', 'field'} } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        lua.completion.lua_sources.as_deref(),
        Some(["function".to_string(), "field".to_string()].as_slice())
    );
    assert_eq!(lua.completion.sh_sources, None, "the shell's is untouched");
}

#[test]
fn the_completion_order_is_read_and_a_typo_is_named() {
    let (alpha, problems) = settings_from("oslo = { completion = { sort = 'alpha' } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(alpha.completion.sort, Sort::Alpha);
    assert_eq!(Settings::default().completion.sort, Sort::Frecency);

    let (kept, problems) = settings_from("oslo = { completion = { sort = 'vibes' } }");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("vibes"));
    assert_eq!(
        kept.completion.sort,
        Sort::Frecency,
        "a typo keeps the default"
    );
}

/// `oslo.suggest.accept = "right"` is how fish spells it, and it used to bind nothing.
#[test]
fn the_suggestion_keys_are_read_under_their_own_names() {
    let (settings, problems) =
        settings_from("oslo = { suggest = { accept = 'right', accept_word = 'alt-f' } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(settings.suggest.accept.as_deref(), Some("right"));
    assert_eq!(settings.suggest.accept_word.as_deref(), Some("alt-f"));
    // Naming them does not disturb the sources.
    assert_eq!(settings.suggest.sh_sources, Suggest::default().sh_sources);
}

/// **These are checked like every other key a config names.** They are compared against the
/// editor's own name for the key that was pressed, so a spelling nothing can produce — `alt-right`,
/// where the only Alt key oslo decodes is `Alt(char)` — was accepted and then matched nothing for
/// the rest of the session. Now it is reported.
#[test]
fn a_suggestion_key_that_cannot_fire_is_reported() {
    let (settings, problems) = settings_from("oslo = { suggest = { accept_word = 'alt-right' } }");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("alt-right"), "{problems:?}");
    assert_eq!(
        settings.suggest.accept_word, None,
        "a name that cannot fire is not kept"
    );

    // And the spelling is normalised, so `Ctrl-F` finds the key that reports itself as `ctrl-f`.
    let (settings, problems) = settings_from("oslo = { suggest = { accept = 'Ctrl-F' } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(settings.suggest.accept.as_deref(), Some("ctrl-f"));
}

#[test]
fn suggestion_sources_keep_the_order_they_were_written_in() {
    let (settings, problems) =
        settings_from("oslo = { suggest = { sh_sources = {'path', 'history'} } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        settings.suggest.sh_sources,
        vec![Source::Path, Source::History]
    );

    // An empty list turns suggestions off, which is a thing someone may want.
    let (off, _) = settings_from("oslo = { suggest = { sh_sources = {} } }");
    assert!(off.suggest.sh_sources.is_empty());
}

/// A typo that silently turns a source off is the kind of thing that gets blamed on the shell.
#[test]
fn an_unknown_source_is_named() {
    let (settings, problems) =
        settings_from("oslo = { suggest = { sh_sources = {'history', 'psychic'} } }");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("psychic"), "{problems:?}");
    // The ones that were understood still take effect.
    assert_eq!(settings.suggest.sh_sources, vec![Source::History]);
}

#[test]
fn a_row_count_is_clamped_to_something_drawable() {
    let (tiny, _) = settings_from("oslo = { completion = { max_rows = 0 } }");
    assert_eq!(tiny.completion.max_rows, 1);
    let (huge, _) = settings_from("oslo = { completion = { max_rows = 9999 } }");
    assert_eq!(huge.completion.max_rows, crate::dropdown::CEILING_ROWS);
}

#[test]
fn key_bindings_are_read_in_a_stable_order() {
    let (settings, problems) = settings_from(
        "oslo = { keys = { ['ctrl-l'] = 'clear-screen', ['shift-tab'] = 'toggle-language' } }",
    );
    assert!(problems.is_empty(), "{problems:?}");
    // Sorted, because table iteration has no order and a binding that depends on it would
    // behave differently between runs.
    assert_eq!(
        settings.keys,
        vec![
            ("ctrl-l".to_string(), "clear-screen".to_string()),
            ("shift-tab".to_string(), "toggle-language".to_string()),
        ]
    );
}

#[test]
fn history_settings_are_read_through() {
    let (settings, _) =
        settings_from("oslo = { history = { size = 50000, file = '~/h', ignore_dups = true } }");
    assert_eq!(settings.history.size, Some(50000));
    assert_eq!(settings.history.file.as_deref(), Some("~/h"));
    assert!(settings.history.ignore_dups);
    // Untouched.
    assert!(settings.history.ignore_space);
    assert!(settings.history.immediate, "recorded on Enter by default");

    let (deferred, _) = settings_from("oslo = { history = { immediate = false } }");
    assert!(
        !deferred.history.immediate,
        "a line is then counted only once it has finished"
    );
}

/// fish's `fish_greeting`, and the two ways of being quiet.
#[test]
fn a_greeting_is_separate_from_the_banner() {
    let (default, _) = settings_from("oslo = {}");
    assert!(default.misc.welcome);
    assert_eq!(default.misc.greeting, None);

    let (custom, problems) = settings_from("oslo = { misc = { greeting = 'hello' } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(custom.misc.greeting.as_deref(), Some("hello"));
}

/// The escape delay is clamped rather than refused: zero would read every arrow key as Esc.
#[test]
fn the_escape_delay_is_clamped_to_something_usable() {
    let (default, _) = settings_from("oslo = {}");
    assert_eq!(default.misc.escape_delay, 25);

    let (zero, _) = settings_from("oslo = { misc = { escape_delay = 0 } }");
    assert_eq!(zero.misc.escape_delay, 1, "0 would break every arrow key");
    let (huge, _) = settings_from("oslo = { misc = { escape_delay = 99999 } }");
    assert_eq!(huge.misc.escape_delay, 2000);
    let (slow, _) = settings_from("oslo = { misc = { escape_delay = 300 } }");
    assert_eq!(slow.misc.escape_delay, 300, "an ssh session's value");
}

/// A depth nobody can spell is named, not silently ignored — the whole point of the override is
/// that the user knows better than detection, so a typo must not fall back to detection.
#[test]
fn an_unknown_colour_depth_is_named() {
    let (settings, problems) = settings_from("oslo = { misc = { color_depth = 'lots' } }");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("lots"), "{problems:?}");
    assert_eq!(settings.misc.color_depth, None);

    let (ok, problems) = settings_from("oslo = { misc = { color_depth = 'truecolor' } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(ok.misc.color_depth.as_deref(), Some("truecolor"));
}

/// `$HISTIGNORE`'s list, and the notification's words.
#[test]
fn history_ignore_and_notify_text_are_read() {
    let (settings, problems) = settings_from(
        "oslo = { history = { ignore = {'ls', 'cd *'} },
                  notify = { after = 30, title = '{cmd}', command = 'notify-send x' } }",
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(settings.history.ignore, vec!["ls", "cd *"]);
    assert_eq!(settings.notify.after, 30);
    assert_eq!(settings.notify_text.title.as_deref(), Some("{cmd}"));
    assert_eq!(
        settings.notify_text.command.as_deref(),
        Some("notify-send x")
    );
}

/// A pattern list with something in it that is not a pattern says so.
#[test]
fn a_history_ignore_entry_that_is_not_a_pattern_is_named() {
    let (settings, problems) = settings_from("oslo = { history = { ignore = {'ls', 42} } }");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(settings.history.ignore, vec!["ls"], "the rest still apply");
}

/// `oslo.abbr` in both its forms. Until this existed the only way to define one was the `abbr`
/// builtin — a shell command, in a shell configured in Lua.
#[test]
fn abbreviations_are_read_in_both_forms() {
    let (settings, problems) = settings_from(
        "oslo = { abbr = {
             gco = 'git checkout',
             gst = 'git status',
             brc = { '~/.config/oslo/init.lua', anywhere = true },
             tmp = { expansion = '/tmp' },
         } }",
    );
    assert!(problems.is_empty(), "{problems:?}");
    // Sorted, because Lua table iteration has no order and two runs must install the same thing.
    assert_eq!(
        settings.abbr,
        vec![
            (
                "brc".to_string(),
                "~/.config/oslo/init.lua".to_string(),
                true
            ),
            ("gco".to_string(), "git checkout".to_string(), false),
            ("gst".to_string(), "git status".to_string(), false),
            ("tmp".to_string(), "/tmp".to_string(), false),
        ]
    );
}

/// An entry that expands to nothing usable is named rather than silently dropped: an abbreviation
/// that does not fire looks exactly like the shell ignoring the config.
#[test]
fn an_abbreviation_without_an_expansion_is_named() {
    let (settings, problems) =
        settings_from("oslo = { abbr = { gco = 'git checkout', bad = { anywhere = true } } }");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("bad"), "{problems:?}");
    assert_eq!(settings.abbr.len(), 1, "the rest still apply");
}

#[test]
fn nav_settings_are_read_with_the_ui_vocabulary() {
    let (settings, problems) = settings_from(
        "oslo = { builtin = { nav = {
            fullscreen = false,
            position = 'center',
            width = 52,
            border = 'rounded',
            border_fg = 'cyan',
            border_fit = 'content',
            legend = false,
            legend_gap = 2,
            padding_x = 3,
            padding_y = 1,
            height = 12,
            hidden = true,
            filter_at = 'top',
            reverse = false,
            scanner = false,
        } } }",
    );
    assert!(problems.is_empty(), "{problems:?}");
    let nav = settings.builtin.nav;
    assert!(!nav.fullscreen);
    assert_eq!(nav.position, crate::ask::chrome::Place::Center);
    assert_eq!(nav.width, 52);
    assert_eq!(nav.border, crate::ask::Border::Rounded);
    assert_eq!(nav.border_fg, crate::theme::Color::parse("cyan"));
    assert_eq!(nav.border_fit, crate::ask::chrome::Fit::Content);
    assert!(!nav.legend);
    assert_eq!(nav.legend_gap, 2);
    assert_eq!(nav.padding_x, 3);
    assert_eq!(nav.padding_y, 1);
    assert_eq!(nav.height, 12);
    assert!(nav.hidden);
    assert_eq!(nav.filter_at, crate::ask::Where::Top);
    assert!(!nav.reverse);
    assert!(!nav.scanner);
}

#[test]
fn invalid_nav_presentation_names_keep_the_defaults() {
    let (settings, problems) = settings_from(
        "oslo = { builtin = { nav = {
            position = 'sideways', border = 'fancy', border_fit = 'wide-ish',
            border_fg = 'puce', filter_at = 'middle',
        } } }",
    );
    assert_eq!(problems.len(), 5, "{problems:?}");
    assert_eq!(settings.builtin.nav, Nav::default());
}

#[test]
fn nav_defaults_to_a_centered_quiet_half_screen() {
    let nav = Nav::default();
    assert!(nav.fullscreen);
    assert_eq!(nav.position, crate::ask::chrome::Place::Center);
    assert_eq!(nav.width, 0);
    assert_eq!(nav.height, 0);
    assert_eq!(nav.border_fit, crate::ask::chrome::Fit::Content);
    assert!(!nav.legend);
}

/// The two nav knobs a config is most likely to reach for, and the defaults they replace.
#[test]
fn nav_marks_and_type_navigation_are_configurable() {
    let (settings, problems) = settings_from(
        "oslo = { builtin = { nav = {
            icons = { dir = 'D', file = 'F', ext = { RS = 'r', md = 'm' } },
            type_nav = { enabled = false, settle_ms = 1200 },
        } } }",
    );
    assert!(problems.is_empty(), "{problems:?}");
    let nav = &settings.builtin.nav;

    assert_eq!(nav.icons.directory, "D");
    assert_eq!(nav.icons.file, "F");
    // Extensions are folded to lower case as they are read, so the lookup never has to.
    assert_eq!(nav.icons.of("main.rs", false), "r");
    assert_eq!(nav.icons.of("README.MD", false), "m");
    assert_eq!(nav.icons.of("Makefile", false), "F");
    assert_eq!(nav.icons.of("src", true), "D");

    assert!(!nav.type_nav.enabled);
    assert_eq!(nav.type_nav.settle, std::time::Duration::from_millis(1200));
}

/// Naming one nav setting must not blank the rest — the merge rule the whole module follows.
#[test]
fn nav_marks_left_unset_keep_their_defaults() {
    let (settings, problems) = settings_from("oslo = { builtin = { nav = { hidden = true } } }");
    assert!(problems.is_empty(), "{problems:?}");
    let nav = &settings.builtin.nav;
    assert_eq!(nav.icons.directory, "■");
    assert_eq!(nav.icons.file, "≡");
    assert!(nav.icons.by_extension.is_empty(), "no marks are built in");
    assert!(nav.type_nav.enabled, "on by default");
}

/// `rm` is skipped by default, and the list is a list rather than a hard-coded name.
#[test]
fn the_commands_whose_history_is_not_offered_are_configurable() {
    let stock = Settings::default();
    assert_eq!(stock.suggest.skip_history, vec!["rm".to_string()]);

    let (settings, problems) =
        settings_from("oslo = { suggest = { skip_history = { 'shred', 'trash' } } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        settings.suggest.skip_history,
        vec!["shred".to_string(), "trash".to_string()]
    );

    // An empty list is a real answer — "offer history for everything" — and not a missing setting.
    let (none, problems) = settings_from("oslo = { suggest = { skip_history = {} } }");
    assert!(problems.is_empty(), "{problems:?}");
    assert!(none.suggest.skip_history.is_empty());
}

/// `oslo.scratch` is read whether or not the build can act on it, so one config works on a machine
/// with the feature and a machine without.
#[test]
fn the_scratch_block_is_read() {
    let (settings, problems) = settings_from(
        r#"oslo = { scratch = { key = "ctrl-g", daemon = true, dir = "/run/x", log_bytes = 4096 } }"#,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(settings.scratch.key, "ctrl-g");
    assert!(settings.scratch.daemon);
    assert_eq!(settings.scratch.dir, "/run/x");
    assert_eq!(settings.scratch.log_bytes, 4096);
}

#[test]
fn the_scratch_default_is_ctrl_backslash_and_no_daemon() {
    let (settings, problems) = settings_from("oslo = {}");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(settings.scratch, Scratch::default());
    assert_eq!(settings.scratch.key, "ctrl-\\");
    assert!(!settings.scratch.daemon);
}

/// **`alt-\`, beside the scratch finder's `ctrl-\`.** The same key with the other modifier, for
/// the other list of things you keep — and Alt sends `ESC` and the character in every terminal, so
/// unlike Ctrl+Enter it arrives without the kitty keyboard protocol having to be negotiated.
#[test]
fn the_macro_manager_opens_on_alt_backslash() {
    let (settings, problems) = settings_from("oslo = {}");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(settings.macros.key, "alt-\\");
    assert_ne!(
        settings.macros.key, settings.scratch.key,
        "two screens on one key would be one screen"
    );

    let (settings, problems) = settings_from(r#"oslo = { macros = { key = "f4" } }"#);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(settings.macros.key, "f4");

    let (settings, problems) = settings_from(r#"oslo = { macros = { key = "nonsense" } }"#);
    assert_eq!(problems.len(), 1, "reported where it was written");
    assert_eq!(settings.macros.key, "alt-\\", "and the default is kept");
}

/// A key nothing can name is reported against the line that wrote it, and the default is kept —
/// the alternative is a scratch with no way out.
#[test]
fn a_scratch_key_that_is_not_a_key_is_reported() {
    let (settings, problems) = settings_from(r#"oslo = { scratch = { key = "wat" } }"#);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("oslo.scratch.key"), "{problems:?}");
    assert_eq!(settings.scratch.key, Scratch::default().key);
}

/// **oslo browses with its own unless told otherwise**, which is the whole point of the setting:
/// a browser that took over because it happened to be installed is a surprise, and one that cannot
/// be turned off without uninstalling something is a worse one.
#[test]
fn nav_uses_the_builtin_browser_until_a_command_names_another() {
    assert!(
        Nav::default().command.is_empty(),
        "an empty command is the built-in browser, and that is the default"
    );

    let (settings, problems) = settings_from(
        "oslo = { builtin = { nav = { command = \
         { 'trek', '--explore', '--cwd-file', '{answer}', '{dir}' } } } }",
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        settings.builtin.nav.command,
        vec!["trek", "--explore", "--cwd-file", "{answer}", "{dir}"],
        "the argv is the config's, verbatim — substitution happens when it runs"
    );
}

/// Anything in the list that is not a string is dropped rather than stringified: a number or a
/// table in an argv is a mistake, and guessing what it meant would run something nobody wrote.
#[test]
fn a_nav_command_keeps_only_words() {
    let (settings, _) = settings_from(
        "oslo = { builtin = { nav = { command = { 'trek', 7, {}, '--explore' } } } }",
    );
    assert_eq!(settings.builtin.nav.command, vec!["trek", "--explore"]);
}

/// `oslo.transcript.rule` is read, and empty is off.
///
/// **Off by default**, because it replaces what a terminal has looked like since terminals: a
/// setting that changes the shape of the scrollback has to be asked for.
#[test]
fn a_transcript_rule_is_off_until_a_config_sets_one() {
    let rule = |source: &str| settings_from(source).0.transcript.rule;
    assert_eq!(rule("oslo = {}"), "", "nothing configured");
    assert_eq!(rule("oslo = { transcript = {} }"), "");
    assert_eq!(rule(r#"oslo = { transcript = { rule = "- " } }"#), "- ");
    assert_eq!(
        rule(r#"oslo = { transcript = { rule = "─" } }"#),
        "─",
        "a unit is whatever the config says, not a dash"
    );
}

/// Nothing stands before the command unless a config asks for it — the brackets already do.
#[test]
fn a_transcript_header_says_this_was_run() {
    let prefix = |source: &str| settings_from(source).0.transcript.prefix;
    assert_eq!(
        prefix("oslo = {}"),
        "",
        "the brackets already say it was run"
    );
    assert_eq!(prefix(r#"oslo = { transcript = { prefix = "$ " } }"#), "$ ");
    assert_eq!(
        prefix(r#"oslo = { transcript = { prefix = "" } }"#),
        "",
        "and empty stays empty"
    );
}

/// The divider's colour, and why the default is an index rather than a slot.
///
/// **`"1"` is palette entry 1**, which a terminal can retint without the shell being told — hexe's
/// `OSC 1330` namespaces do exactly that. A theme slot would be resolved here and baked into the
/// bytes, and no palette could reach it afterwards.
#[test]
fn the_divider_is_an_indexed_colour_by_default() {
    let style = |source: &str| settings_from(source).0.transcript.style;
    assert_eq!(style("oslo = {}"), "1");
    assert_eq!(
        style("oslo = { transcript = { style = \"#7c7c7c\" } }"),
        "#7c7c7c"
    );

    // And it parses as one, which is what makes it an `ESC[38;5;1m` rather than a name nobody reads.
    assert_eq!(
        crate::theme::Color::parse("1"),
        Some(crate::theme::Color::Indexed(1))
    );
}

/// `oslo.builtin.nav.detached` — the browser draws elsewhere, so the prompt is kept alive.
///
/// **Off by default, and being wrong about it damages the screen**: a browser run inline owns the
/// terminal, and a prompt repainted over it lands in the middle of somebody's file list.
#[test]
fn a_browser_is_assumed_to_take_the_terminal() {
    let detached = |source: &str| settings_from(source).0.builtin.nav.detached;
    assert!(!detached("oslo = {}"), "nothing configured");
    assert!(!detached("oslo = { builtin = { nav = {} } }"));
    assert!(detached(
        "oslo = { builtin = { nav = { detached = true } } }"
    ));
    assert!(!detached(
        "oslo = { builtin = { nav = { detached = false } } }"
    ));
}
