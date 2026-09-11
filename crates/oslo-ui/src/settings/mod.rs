//! The knobs a config sets, other than colours.
//!
//! Read from `oslo.completion`, `oslo.suggest`, `oslo.history` and `oslo.keys` once the config has
//! run, and merged over the defaults the same way [`super::theme`] is — naming one field must not
//! blank the rest.
//!
//! Kept apart from the theme because they answer different questions: a theme says what things
//! *look* like, and these say what the shell *does*. A user who wants a different colour scheme
//! and a user who wants fewer dropdown rows are not the same user.

use super::matching::Fuzzy;

mod editing;
mod escape;
pub use editing::{Autopair, Vi};
mod misc;
pub use misc::{Misc, Transcript};
mod nav;
pub use escape::{EscapeAction, InterruptEscape};
pub use nav::{Icons, Nav, TypeNav};

/// Everything configurable that is not a colour.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub completion: Completion,
    pub suggest: Suggest,
    /// `oslo.lua`: what the Lua prompt does that the shell prompt does not.
    pub lua: Lua,
    pub history: History,
    /// `oslo.vi`: whether vi mode is on, and the cursor for each mode.
    pub vi: Vi,
    /// `oslo.autopair`: whether a bracket or a quote closes itself.
    pub autopair: Autopair,
    /// `oslo.notify`: when a finished command is worth a desktop notification.
    pub notify: Notify,
    /// The notification's words, split out because `Notify` is `Copy`.
    pub notify_text: NotifyText,
    /// `oslo.abbr` — abbreviations declared by the config, as `(name, expansion, anywhere)`.
    ///
    /// A `Vec` rather than a map because it is installed once, in order, and a map would only add
    /// a question about which of two definitions of the same name won.
    ///
    /// Abbreviations already existed, and until now the **only** way to define one was the `abbr`
    /// builtin — so a Lua config had to shell out to declare one, in a shell whose configuration
    /// is Lua. That is the gap this closes.
    pub abbr: Vec<(String, String, bool)>,
    /// `oslo.finder`: the full-screen history search.
    pub finder: Finder,
    /// `oslo.misc`: the settings that belong to no other group.
    pub misc: Misc,
    /// `oslo.transcript` — what a finished line leaves behind in place of its prompt.
    pub transcript: Transcript,
    /// `oslo.builtin`: knobs belonging to a builtin rather than to the editor.
    pub builtin: Builtins,
    /// `oslo.dirs`: the directories `@name` reaches.
    ///
    /// Sorted, because table iteration has no order and a diagnostic that named them in a
    /// different order each run would be maddening to compare.
    pub dirs: Vec<(String, String)>,
    /// `oslo.keys`: key name to action name, both as written.
    pub keys: Vec<(String, String)>,
    /// `oslo.keys[…] = { desc = … }` — what a binding says it does, for anything that lists them.
    ///
    /// Beside `keys` rather than inside it: a description is optional and most bindings have none,
    /// and widening the pair every existing reader destructures would be a change to all of them
    /// for the sake of a field they do not use.
    pub key_descriptions: Vec<(String, String)>,
    /// `oslo.scratch`: named sessions that outlive the terminal they were opened in.
    pub scratch: Scratch,
    /// `oslo.macros`: the small named things you keep, and the key that opens them.
    pub macros: Macros,
    /// `oslo.table`: how a structured pipeline's rows are **drawn**.
    pub table: Table,
}

/// `oslo.table` — the face of a row table that a person reads.
///
/// **Drawn, never transported.** Nothing here may reach `render_transport`: that is the form another
/// program reads, and a setting that changed it would put somebody's preference on `grep`'s standard
/// input. The two renderers are two functions for exactly this reason, and this is the configuration
/// surface of one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// A leading column of row numbers, counted from zero.
    ///
    /// Off by default. It is genuinely useful for `first`/`skip` arithmetic and genuinely noise the
    /// rest of the time, and `enumerate` already adds a real column for anyone who wants one that
    /// survives into a pipe.
    pub index: bool,
    /// The widest a single cell may be drawn, in terminal cells. `0` is no limit.
    ///
    /// A `cmdline` column is a hundred characters and squeezes every other column off the row. The
    /// whole line is already clamped to the terminal; this stops one column doing it alone.
    pub max_column: usize,
    /// What an absent or null cell shows. Empty by default, which is what a blank column means.
    pub null: String,
    /// The rule drawn around and between the cells.
    ///
    /// **The same names `ui style` uses** — `none`, `rounded`, `square`, `double`, `thick` — because
    /// a config that says "rounded" should mean one thing everywhere, and a second vocabulary for
    /// the same five shapes is a second thing to remember.
    ///
    /// Borders cost three columns per column boundary, and the drawn line is already clamped to the
    /// terminal, so a wide table loses more of its last column with them on. That is the trade, and
    /// it is why this is a setting rather than a decision.
    pub border: crate::ask::Border,
}

impl Default for Table {
    fn default() -> Self {
        Table {
            index: false,
            // Wide enough for a path or a status, narrow enough that one column cannot own the row.
            max_column: 60,
            null: String::new(),
            // **Off, like `index`, and for the same reason.** A rule is a real improvement when a
            // table is the thing you are reading and pure noise when it scrolls past between two
            // commands — and the borderless form is what every existing session already looks like.
            // `oslo.table.border = "rounded"` is one line for anyone who wants the drawn box.
            border: crate::ask::Border::None,
        }
    }
}

/// `oslo.macros` — the manager `oslo macros show` opens, on a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macros {
    /// The key that opens the manager at the prompt.
    pub key: String,
}

impl Default for Macros {
    fn default() -> Self {
        Macros {
            // `alt-\`, beside the scratch finder's `ctrl-\`: the same key with the other
            // modifier, for the other list of things you keep. Alt sends `ESC` and the character
            // in every terminal, so unlike a Ctrl+Enter this one arrives without the kitty
            // keyboard protocol having to be negotiated first.
            key: "alt-\\".to_string(),
        }
    }
}

/// `oslo.tab` — named sessions that keep running when the terminal goes away.
///
/// ```lua
/// oslo.scratch.key = "ctrl-\\"   -- opens the finder, in a tab or out of one
/// oslo.scratch.daemon = false    -- a keeper per tab, rather than one registry process
/// ```
///
/// **Read even in a build without the `tab` feature**, where nothing acts on it. A config is
/// shared between machines and builds; making it ask `if oslo.tab then` before setting a key would
/// be a question with only one useful answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scratch {
    /// The key that opens the finder. Only a control chord can be one byte, which is what the
    /// client inside a tab has to match.
    pub key: String,
    /// Whether tabs are held by one registry process rather than a keeper each.
    pub daemon: bool,
    /// Where the sockets and their bookkeeping live. Empty means the default under `/tmp`.
    pub dir: String,
    /// How much of a tab's output to keep, so an attach lands on the screen it left.
    pub log_bytes: u64,
}

impl Default for Scratch {
    fn default() -> Self {
        Scratch {
            // `^\`, because nothing in common use binds it — and whatever this is gets swallowed
            // from every program inside a tab, which rules out `^X`: nano's Exit, emacs' prefix.
            key: "ctrl-\\".to_string(),
            daemon: false,
            dir: String::new(),
            log_bytes: 1 << 20,
        }
    }
}

/// `oslo.notify` — a desktop notification when a slow command finishes.
///
/// ```lua
/// oslo.notify = { after = 10 }   -- seconds; 0 turns it off
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Notify {
    /// Seconds a command must run before finishing is worth telling you about. `0` never notifies.
    pub after: u64,
}

/// `oslo.notify`'s text, which is a `String` and so cannot live in the `Copy` struct above.
///
/// The title is configurable because the default — the command that finished — is the right
/// answer on a desktop and the wrong one on a machine you have four sessions open on. Naming the
/// host, or the project, is what makes a notification worth reading.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotifyText {
    /// Shown as the notification's title instead of the command. `{cmd}`, `{status}` and
    /// `{duration}` are substituted.
    pub title: Option<String>,
    /// A command run instead of writing the terminal's own notification escape.
    ///
    /// The escape is right for a terminal that forwards it and does nothing at all for one that
    /// does not — and there is no way to ask. This is the escape hatch: `notify-send`, `terminal-notifier`,
    /// a shell function that pushes to your phone. The same three placeholders are substituted.
    pub command: Option<String>,
}

impl Default for Notify {
    fn default() -> Self {
        // Ten seconds: long enough that you have looked away, short enough to catch a test run.
        // A notification for something you sat and watched is noise, and noise gets muted.
        Notify { after: 10 }
    }
}

/// `oslo.completion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// Rows the dropdown shows before it scrolls.
    pub max_rows: usize,
    /// Whether the description column is drawn at all.
    pub descriptions: bool,
    /// Whether the kind badge is drawn.
    pub show_kind: bool,
    /// Whether a candidate must match the typed case.
    pub case_sensitive: bool,
    /// How far the dropdown will stretch to reach a candidate you did not prefix.
    ///
    /// Tried only after the three prefix matchers have all come back empty, so turning it on can
    /// never push a candidate you *did* prefix down the list. Free on the typing path either way:
    /// the dropdown is built once per Tab, not per keystroke.
    pub fuzzy: Fuzzy,
    /// How candidates are ordered: by how often they are used, or by name.
    pub sort: Sort,
    /// The kinds of candidate offered at a **shell** prompt. `None` means every kind, the default.
    ///
    /// Kept as the kind strings the candidates already carry, so adding a kind does not mean
    /// touching an enum here as well.
    ///
    /// `oslo.completion.sh_sources`, named to match [`Self::lua_sources`] and
    /// `oslo.suggest.sh_sources`.
    pub sh_sources: Option<Vec<String>>,
    /// The kinds of candidate offered at a **Lua** prompt: `function`, `field`, `keyword`.
    ///
    /// A separate list for the same reason the suggestion sources are separate — the two prompts
    /// do not share a single kind of candidate between them, so one filter could only ever be
    /// wrong for one of them. A shell filter naming `file` and `option` would have silently
    /// emptied every Lua dropdown.
    pub lua_sources: Option<Vec<String>>,
}

/// `oslo.completion.sort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Most-used first, name as the tie-break. Without this, `exit` suggests `exitsnoop-bpfcc`.
    #[default]
    Frecency,
    /// Name only, ignoring how often anything has been used.
    Alpha,
}

impl Default for Completion {
    fn default() -> Self {
        Completion {
            // What the menu has always actually shown. It used to be documented as 15 and capped
            // at 8, so the number here was unreachable; now the cap is gone the honest default is
            // the behaviour people already have. Raise it to anything up to `CEILING_ROWS`.
            max_rows: crate::dropdown::DEFAULT_ROWS,
            descriptions: true,
            show_kind: true,
            case_sensitive: false,
            // On by default in the dropdown, which is the place it costs nothing and where you
            // have already said "help me" by pressing Tab.
            fuzzy: Fuzzy::Smart,
            sort: Sort::default(),
            sh_sources: None,
            lua_sources: None,
        }
    }
}

/// Where a ghost suggestion may come from, and in what order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A line the user has actually run.
    History,
    /// **Whatever this prompt's completer offers** — the same machinery Tab uses, reduced to the
    /// single answer a ghost can promise.
    ///
    /// What that *is* follows the language, because "complete what is being typed" is one idea
    /// with two answers: at a shell prompt a command name — a builtin, an alias, a function,
    /// something on `$PATH` — and at a Lua prompt a Lua name, meaning a global or a field of the
    /// table being indexed. `names` and `globals` are accepted spellings for the Lua reading.
    ///
    /// There was briefly a separate `Names` source for the Lua half. That was a mistake: two names
    /// for one job, and a config had to know which prompt it was writing for to pick the right
    /// word.
    Completion,
    /// A file or directory.
    Path,
    /// What the model thinks comes next.
    ///
    /// **Not in the default order.** It is the only source that can offer a line you have not
    /// typed *here* before, which is its value and its risk, and it is worth having only once it
    /// has been measured against `History` on a real history rather than assumed to be better.
    /// Ask for it by name until then.
    Prediction,
    /// Whatever a config or a plugin registered with `oslo.suggest.provider`.
    ///
    /// **A source among the sources.** Where a plugin's answer sits relative to your own history is
    /// one line of `oslo.suggest.sh_sources`, and it is your line — the plugin does not get to decide
    /// that it outranks what you have actually run.
    Provider,
}

impl Source {
    fn parse(name: &str) -> Option<Source> {
        match name {
            "history" => Some(Source::History),
            // `names` and `globals` are the Lua reading of the same source, taken as spellings
            // rather than as a source of their own — see `Source::Completion`.
            "completion" | "completions" | "names" | "name" | "globals" => Some(Source::Completion),
            "path" | "paths" | "file" => Some(Source::Path),
            "predict" | "prediction" => Some(Source::Prediction),
            "provider" | "providers" | "plugin" => Some(Source::Provider),
            _ => None,
        }
    }
}

/// `oslo.finder` — the full-screen history search.
///
/// Separate from `oslo.completion` because it answers a different question. Completion suggests
/// what a half-typed word *could* be; this searches what you have actually run. They share the
/// fuzzy setting, since "how loosely should matching work" is one preference and having two would
/// only ever be a way to set them inconsistently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finder {
    /// Off means Up keeps walking history a line at a time, as it always did.
    pub enabled: bool,
    /// The key that opens it. Any name `oslo.keys` understands.
    pub key: String,
    /// How many distinct commands to load. The finder reads once when it opens and filters in
    /// memory, so this bounds the work done on the keystroke that opens it — not per keystroke
    /// while you type.
    pub limit: usize,
    /// Whether Delete asks before forgetting a command.
    ///
    /// **On by default.** Deleting a history entry is not undoable — the rows are gone from the
    /// store, and the only way back is to run the command again — so the default is the one that
    /// cannot lose something by a mistyped keystroke. Turn it off if you are clearing a lot at
    /// once and the question is in the way.
    pub confirm_delete: bool,
    /// The scope it opens in. `None` is `"auto"`: the workspace inside a git worktree that has
    /// history of its own, global everywhere else.
    pub scope: Option<crate::finder::Scope>,
}

impl Default for Finder {
    fn default() -> Self {
        Finder {
            enabled: true,
            // Up rather than Ctrl-R: Ctrl-R is muscle memory pointing at a *search*, and this is
            // first of all a history *list* — the thing you reach for by pressing Up. Both are
            // configurable, and Up still walks a line at a time when the finder is off.
            key: "up".to_string(),
            confirm_delete: true,
            // Far more than anyone has, so the list is "everything" in practice, and still a bound
            // rather than an unbounded read on a store that has been collecting for years.
            limit: 10_000,
            scope: None,
        }
    }
}

/// `oslo.suggest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggest {
    /// The sources to try at a **shell** prompt, in order. Empty turns suggestions off entirely.
    ///
    /// `oslo.suggest.sh_sources`. It was `sources`, which read as "the sources" — the one list —
    /// right up until there were two of them and the other one had to be called something else.
    pub sh_sources: Vec<Source>,
    /// The sources to try at a **Lua** prompt.
    ///
    /// **A separate list, because a Lua prompt is not a shell prompt wearing a hat.** Three of the
    /// shell sources answer with shell — `path` completes a filename in command position,
    /// `predict` is a model trained on commands, `completion` offers names from `$PATH` — and none
    /// of them can be typed at a Lua prompt. Sharing one list meant a config tuned for the shell
    /// silently decided what Lua did, and what it usually decided was nothing at all.
    ///
    /// **`history` is deliberately not the default here.** A Lua prompt should behave like an
    /// editor: what it offers is what *exists* — the names in the session — not what you happened
    /// to type last week. History is still available by asking for it.
    pub lua_sources: Vec<Source>,
    /// The key that takes the whole ghost suggestion, e.g. `"right"`.
    ///
    /// The action was always bindable through `oslo.keys`, but not under the name the suggestion
    /// settings use — so a config that wrote `oslo.suggest.accept = "right"`, which is how fish
    /// spells it, silently bound nothing.
    pub accept: Option<String>,
    /// The key that takes one word of it.
    pub accept_word: Option<String>,
    /// Commands whose past lines are not worth offering back.
    ///
    /// **Because their arguments consume themselves.** `rm z` suggested `rm zzz-old-notes` from
    /// history — a path that does not exist any more, *because the suggested command deleted it*.
    /// Accepting a ghost is one keystroke, and for `rm` that is one keystroke aiming a destructive
    /// command at whatever the name happens to match now.
    ///
    /// Only the history source is skipped. The filesystem still completes the argument, which is
    /// the answer that was wanted in the first place.
    pub skip_history: Vec<String>,
}

impl Default for Suggest {
    fn default() -> Self {
        // fish's order. History first because a line the user has run is a better guess than
        // anything that can be ranked.
        Suggest {
            sh_sources: vec![Source::History, Source::Completion, Source::Path],
            // The completer alone: an editor offers what exists, and at a Lua prompt that is the
            // names in the session. See the field.
            lua_sources: vec![Source::Completion],
            accept: None,
            accept_word: None,
            skip_history: vec!["rm".to_string()],
        }
    }
}

#[cfg(test)]
#[path = "suggest_tests.rs"]
mod suggest_tests;

#[path = "lua.rs"]
mod lua_settings;
pub use lua_settings::{Enter, Lua};

/// `oslo.history`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    pub size: Option<usize>,
    pub file: Option<String>,
    /// Whether a line beginning with a space is kept out of the history.
    pub ignore_space: bool,
    /// Whether a line identical to the one before it is dropped.
    ///
    /// Off by default, and that is not an aesthetic choice: dropping a repeat renumbers every
    /// later event, so `!-2` would point one line further back than it says.
    pub ignore_dups: bool,
    /// Patterns whose matching lines are never remembered. bash's `$HISTIGNORE`.
    ///
    /// Matched against the **whole line** with the shell's own glob rules, so `ls` is only `ls`
    /// and `ls *` is every `ls`. Whole-line rather than per-word because the thing people want to
    /// keep out is a command shape — `git commit -m *`, or anything with a token in it — and a
    /// per-word rule would drop half a line and remember the rest.
    pub ignore: Vec<String>,
}

impl Default for History {
    fn default() -> Self {
        History {
            size: None,
            file: None,
            ignore_space: true,
            ignore_dups: false,
            ignore: Vec::new(),
        }
    }
}

/// `oslo.builtin` — the knobs that belong to a builtin rather than to the editor.
///
/// Separate from the rest because these are read by code in `crate::env::builtins`, which runs in
/// scripts as well as at a prompt. Everything else here describes an interactive session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Builtins {
    pub rm: Rm,
    pub nav: Nav,
}

/// `oslo.builtin.rm` — what the `rm` builtin does with what it removes.
///
/// ```lua
/// oslo.builtin.rm.to_tmp     = true    -- move instead of destroying
/// oslo.builtin.rm.max_to_tmp = 100     -- MB; anything larger is destroyed
/// oslo.builtin.rm.trash      = "/tmp"
/// ```
///
/// **None of this applies to a script.** `rm` reads these only when the shell is interactive, so a
/// `#!/bin/sh` file running under oslo removes what it says it removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rm {
    /// Whether a removal at the prompt moves the file to [`Rm::trash`] instead of unlinking it.
    ///
    /// Off by default. On is the more forgiving behaviour, but it is also the more surprising one:
    /// `rm` that does not free space is not what the name has meant since 1971, and a default that
    /// silently fills a filesystem is not a default.
    pub to_tmp: bool,
    /// Megabytes. Anything larger than this is destroyed rather than moved.
    ///
    /// The cap exists because the trash is often on another filesystem — `/tmp` usually is, and is
    /// usually tmpfs — where a move is a *copy*, so trashing a large file costs its size in time
    /// and possibly in RAM. Below the cap that cost is not worth noticing; above it, it is.
    pub max_to_tmp: u64,
    /// Where trashed files go. Created on first use.
    pub trash: String,
}

impl Default for Rm {
    fn default() -> Self {
        Rm {
            to_tmp: false,
            max_to_tmp: 100,
            trash: "/tmp".to_string(),
        }
    }
}

/// Read the settings out of the `oslo` table, and what was wrong with them.
pub mod from_lua;
pub use from_lua::read_lua_settings;

use std::sync::RwLock;

static SETTINGS: RwLock<Option<std::sync::Arc<Settings>>> = RwLock::new(None);

/// What [`current`] answers before a config has been installed.
static UNCONFIGURED: std::sync::OnceLock<std::sync::Arc<Settings>> = std::sync::OnceLock::new();

/// The settings in force.
///
/// There used to be a `VI_OVERRIDE` slot here, holding what `--vi`/`--no-vi` had asked for so it
/// could beat the config that is read after the command line. Both flags are gone and so is it:
/// `oslo.vi.enabled` is the only place the editing mode lives, which means there is no longer a
/// second source for the two to disagree from — and one less piece of process-wide mutable state.
/// Shared rather than copied: the editor asks two to four times per keystroke, and a `Settings`
/// carries every abbreviation and every key binding the config declared.
pub fn current() -> std::sync::Arc<Settings> {
    if let Ok(guard) = SETTINGS.read()
        && let Some(settings) = guard.as_ref()
    {
        return std::sync::Arc::clone(settings);
    }
    std::sync::Arc::clone(UNCONFIGURED.get_or_init(|| std::sync::Arc::new(Settings::default())))
}

pub fn install(settings: Settings) {
    // Publish vi mode before the prompt or editor reads the installed settings.
    super::vi::set_enabled(settings.vi.enabled);
    super::edit::pair::set_enabled(settings.autopair.enabled);
    // `oslo.misc.color_depth` is applied here rather than read where colour is painted: the depth
    // is cached on first use, so a config that sets it after something has already drawn would be
    // ignored. Installing the settings is the moment the config is known and nothing has drawn.
    if let Some(name) = &settings.misc.color_depth
        && let Some(depth) = super::theme::Depth::named(name)
    {
        super::theme::set_depth(depth);
    }
    if let Ok(mut slot) = SETTINGS.write() {
        *slot = Some(std::sync::Arc::new(settings));
    }
}

#[cfg(test)]
mod tests;
