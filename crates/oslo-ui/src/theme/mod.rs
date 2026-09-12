//! The colours the prompt draws with, and where they come from.
//!
//! One [`Theme`], read from `oslo.theme` in the config, used by three places that previously each
//! carried their own hardcoded escapes: the dropdown, the syntax highlighter and the prompt.
//!
//! **Merged, not replaced.** A config that writes `oslo.theme = { syntax = { command = "cyan" } }`
//! means "make commands cyan", not "discard every other colour". A whole-table assignment is the
//! natural way to write one field and it must not silently blank the other forty — so every field
//! is an `Option` layered over [`Theme::default`], and only what the config names is overridden.

pub mod color;
mod from_lua;
pub mod styles;

pub use color::{Color, Depth, Style};
pub use from_lua::read_lua_theme;

use std::sync::RwLock;

/// The theme in force.
static THEME: RwLock<Option<Theme>> = RwLock::new(None);

/// The colour depth in force, decided once.
static DEPTH: RwLock<Option<Depth>> = RwLock::new(None);

/// Read the theme. Cheap enough for a keystroke: a read lock and a clone of a plain struct.
pub fn current() -> Theme {
    THEME
        .read()
        .ok()
        .and_then(|t| t.clone())
        .unwrap_or_default()
}

/// Install a theme, replacing whatever was there.
pub fn install(theme: Theme) {
    if let Ok(mut slot) = THEME.write() {
        *slot = Some(theme);
    }
}

/// The colour depth, detected on first use.
///
/// Cached because `$TERM` and `$COLORTERM` do not change during a session, and because this is
/// read once per styled span — several hundred times for one redraw of a full dropdown.
pub fn depth() -> Depth {
    if let Ok(slot) = DEPTH.read()
        && let Some(depth) = *slot
    {
        return depth;
    }
    let detected = Depth::detect();
    if let Ok(mut slot) = DEPTH.write() {
        *slot = Some(detected);
    }
    detected
}

/// Force a depth, for tests and for a config that knows better than the environment.
pub fn set_depth(depth: Depth) {
    if let Ok(mut slot) = DEPTH.write() {
        *slot = Some(depth);
    }
}

/// Hold the depth at `depth` for as long as the returned guard lives.
///
/// **`DEPTH` is one slot for the whole process**, and several modules' tests assert on the
/// escapes it decides: `highlight` renders at `Ansi16`, the finder and the dropdown at `Ansi256`.
/// Without a lock they race — whichever sets it first decides what the others see, and the loser
/// fails on an escape that is perfectly correct for the depth it actually got. It is intermittent,
/// which is worse than broken: it went green six runs in a row while being wrong.
///
/// The same in-process global-state trap as `environ`, and the same answer: serialise the tests
/// that touch it rather than hope they do not overlap.
/// Unconditional rather than behind a cargo feature, which it used to be: the tests that need it
/// are in other crates, and a feature that exists to serve tests is one `--all-features` turns on —
/// which would compile test scaffolding into a release binary because somebody asked for
/// "everything". Nothing outside a test calls it, so the linker drops it.
#[doc(hidden)]
#[must_use = "the depth is only held while the guard lives; `let _ = ` drops it immediately"]
pub fn held_at(depth: Depth) -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // A poisoned lock means some other test panicked while holding it. That is its problem, not
    // this test's, and refusing to run would turn one failure into all of them.
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    set_depth(depth);
    guard
}

/// What the terminal said its background is, once it has been asked.
static BACKGROUND: RwLock<Option<super::query::Background>> = RwLock::new(None);

/// Record the terminal's background, so the palette can suit it.
///
/// This changes what [`Syntax::default`] *is*, rather than installing a theme — and that ordering
/// is the whole point. The config is read afterwards and merged over the default, so a config that
/// names three colours still gets the light palette for the other twenty, and its own three still
/// win. Installing a theme here instead would be overwritten wholesale the moment the config was
/// read, which is exactly what happened on the first attempt.
pub fn set_background(background: super::query::Background) {
    if let Ok(mut slot) = BACKGROUND.write() {
        *slot = Some(background);
    }
}

/// The terminal's background, if it was asked and answered.
pub fn background() -> Option<super::query::Background> {
    BACKGROUND.read().ok().and_then(|b| *b)
}

/// Everything the interactive layer draws with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Theme {
    pub syntax: Syntax,
    pub pager: Pager,
    pub prompt: Prompt,
    /// Colours for the input widgets — `oslo.ui` in Lua, the `ui` builtin in scripts.
    pub ui: Ui,
}

/// The palette every prompt widget draws from.
///
/// **One accent, and then ordinary ANSI.** The accent is the shell's colour: it marks the thing
/// you are being asked about and the thing you have chosen, and nothing else competes with it.
/// Everything else is a basic ANSI colour rather than an RGB value, so a widget looks like it
/// belongs in whatever palette the terminal is using — which is the difference between a prompt
/// that sits in your theme and one that ignores it.
///
/// The set is small on purpose. gum has a flag for the colour of every part of every widget; the
/// result is that nobody sets any of them, and the ones who do end up with a prompt that matches
/// nothing else on their screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ui {
    /// The main colour: the cursor, the selected row, the checked box, the focused button.
    pub accent: Style,
    /// The question itself.
    pub question: Style,
    /// Placeholder text, hints, and the key legend along the bottom.
    pub muted: Style,
    /// An answer that was refused, and the `no` side of a confirm.
    pub error: Style,
    /// A finished answer, echoed back after the widget closes.
    pub done: Style,
}

impl Default for Ui {
    fn default() -> Self {
        let basic = |index: u8, bright: bool| Style::fg(Color::Basic { index, bright });
        Ui {
            // Bright magenta, which is gum's default accent and is unused by oslo's syntax
            // colours — so an input never looks like a piece of the command line behind it.
            accent: Style {
                bold: true,
                ..basic(5, true)
            },
            question: Style {
                bold: true,
                ..Style::default()
            },
            muted: basic(0, true),
            error: basic(1, true),
            done: basic(2, false),
        }
    }
}

/// Colours for the line as it is typed.
///
/// The names are fish's, because they are the ones people already have in a config somewhere and
/// because the set is a good specification of how deep highlighting should go. `builtin`,
/// `function` and `keyword` fall back to `command` when a theme leaves them out, which is what
/// keeps a two-line theme from looking half-finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Syntax {
    pub command: Style,
    pub builtin: Style,
    pub function: Style,
    pub keyword: Style,
    /// A command name that resolves to nothing. fish's most useful colour.
    pub error: Style,
    /// A command that runs what follows it as another user: `sudo`, `doas`, `su`.
    pub danger: Style,
    pub param: Style,
    /// A parameter that names a file which exists.
    pub valid_path: Style,
    pub option: Style,
    /// A glob metacharacter: `*`, `?`, `[…]`.
    pub glob: Style,
    /// A glob that matches nothing — the word will reach the command as its own text, or not at
    /// all under `failglob`. Not drawn when `nullglob` makes an empty match ordinary.
    pub glob_nomatch: Style,
    /// A stream coordinate: `{0:1}`, `{%0:0}`. Kin to `glob` and deliberately not the same colour —
    /// both turn one word into others, but a glob asks the filesystem and a coordinate asks the
    /// pipeline, and `{4}` versus bash's literal `{4}` is a distinction only colour can make here.
    pub coordinate: Style,
    /// A bare number.
    pub number: Style,
    /// The `NAME=` of an assignment.
    pub assignment: Style,
    /// `'…'` — literal throughout.
    pub single_quote: Style,
    /// The literal parts of `"…"`. What expands inside it takes the variable colour instead.
    pub double_quote: Style,
    pub escape: Style,
    pub operator: Style,
    pub redirection: Style,
    /// `;` and `&`.
    pub end: Style,
    pub comment: Style,
    pub variable: Style,
    pub autosuggestion: Style,
    /// The correction drawn after a line that looks mistyped.
    ///
    /// Reversed rather than coloured, and that is the whole design: a ghost suggestion is text you
    /// might be about to have, so it recedes; this is the shell disagreeing with what you typed,
    /// which is the opposite job. `oslo.theme.styles` overrides it like any other entry.
    pub repair: Style,
    pub match_bracket: Style,
}

impl Syntax {
    /// The palette for a light terminal.
    ///
    /// The dark defaults are tuned for a near-black background and become unreadable on white —
    /// `#50fa7b` on white is a pale green nobody can see. These are the same *roles* at darker
    /// values, so the meaning of each colour is unchanged and only its lightness moves.
    ///
    /// Reached only when the terminal actually answers `OSC 11` and says it is light. A terminal
    /// that stays silent keeps the dark palette, which is the safer guess.
    pub fn for_light_background() -> Syntax {
        // Sharpened, not lifted. "Brighter" on a white background means *more colour*, not more
        // light: raising the value of these would wash them out and cost the contrast they were
        // chosen for. Same transform, zero on the value axis.
        let rgb = |r: u8, g: u8, b: u8| Style::fg(Color::Rgb(r, g, b).intensified(0.0));
        Syntax {
            command: rgb(0x1a, 0x7f, 0x37),
            // The dark palette's builtin pink at a lightness a white background can carry.
            builtin: Style {
                bold: true,
                ..rgb(0xa8, 0x00, 0x74)
            },
            function: rgb(0x0a, 0x69, 0x8c),
            keyword: rgb(0xa6, 0x1c, 0x7b),
            error: Style {
                underline: true,
                ..rgb(0xcf, 0x22, 0x2e)
            },
            danger: Style {
                bold: true,
                bg: Some(Color::Rgb(0xcf, 0x22, 0x2e).intensified(0.0)),
                ..rgb(0xff, 0xff, 0xff)
            },
            param: Style::default(),
            valid_path: Style {
                underline: true,
                ..Style::default()
            },
            option: rgb(0xa8, 0x54, 0x00),
            glob: Style {
                bold: true,
                ..rgb(0xa6, 0x1c, 0x7b)
            },
            glob_nomatch: Style {
                underline: true,
                ..rgb(0xc4, 0x1a, 0x16)
            },
            coordinate: Style {
                bold: true,
                ..rgb(0x0a, 0x69, 0x8a)
            },
            number: rgb(0x69, 0x39, 0xb8),
            variable: rgb(0x69, 0x39, 0xb8),
            assignment: rgb(0x1a, 0x7f, 0x37),
            single_quote: rgb(0x87, 0x6d, 0x0b),
            double_quote: rgb(0xa8, 0x8a, 0x0e),
            escape: rgb(0xa6, 0x1c, 0x7b),
            operator: rgb(0x0a, 0x69, 0x8c),
            redirection: rgb(0xa8, 0x54, 0x00),
            end: rgb(0x6e, 0x77, 0x81),
            comment: rgb(0x6e, 0x77, 0x81),
            // Light in both palettes, and for the same reason: it has to read as not-yet-text.
            autosuggestion: Style::fg(Color::Indexed(250)),
            // The ghost's own colour, turned inside out: the correction is the same kind of
            // not-yet-text, saying the opposite thing about it.
            repair: Style {
                reverse: true,
                ..Style::fg(Color::Indexed(250))
            },
            match_bracket: Style {
                bold: true,
                ..Style::default()
            },
        }
    }
}

/// How far the dark palette's hues are lifted off the background, on HSV's value axis.
///
/// Positive because the default background is dark. The light palette passes `0.0` — see
/// [`Syntax::for_light_background`].
const LIFT: f32 = 0.12;

impl Default for Syntax {
    fn default() -> Self {
        // A light terminal gets the light palette as its *starting point*, so a config's own
        // colours are still merged over the top of it rather than over an unreadable one.
        if background() == Some(super::query::Background::Light) {
            return Syntax::for_light_background();
        }
        // Every hue here is lifted off the background before it is used. The palette was picked
        // for a black terminal and read as muted on the grey ones people actually use; rather than
        // re-choosing twenty hex values by eye, the lift is one number in `color.rs` and applies
        // evenly, so the relationships between the colours are the ones that were chosen.
        //
        // `intensified` leaves ANSI slots and near-greys alone, which is why it can be applied
        // here without any per-colour exceptions: the black on `sudo`'s red background and the
        // ghost-text grey come through untouched.
        let rgb = |r: u8, g: u8, b: u8| Style::fg(Color::Rgb(r, g, b).intensified(LIFT));
        Syntax {
            // **RGB, not the sixteen ANSI slots.** A palette tool like pywal remaps what
            // `basic(2)` means, so a theme built on the slots changes colour whenever the wallpaper
            // does. These are absolute: red stays red. Only the *syntax* palette is pinned this
            // way — the prompt and pager deliberately keep the slots, so they still follow the
            // terminal's scheme.
            command: rgb(0x50, 0xfa, 0x7b),
            // Pink, not the command green it used to share. A builtin is not a program: it has no
            // file, `which` cannot find it, and it can change the shell's own state in ways no
            // `$PATH` command can. Sharing green with commands hid the one distinction worth
            // drawing. This is exactly `Indexed(212)`, which is the dropdown's builtin pill, so a
            // builtin is the same colour wherever it appears.
            builtin: Style {
                bold: true,
                ..rgb(0xff, 0x87, 0xd7)
            },
            function: rgb(0x8b, 0xe9, 0xfd),
            keyword: rgb(0xff, 0x79, 0xc6),
            error: Style {
                underline: true,
                ..rgb(0xff, 0x55, 0x55)
            },
            // A command that runs everything after it as another user. Black on red, because it
            // is the one word in a line whose presence changes what every other word can do.
            danger: Style {
                bold: true,
                bg: Some(Color::Rgb(0xff, 0x55, 0x55).intensified(LIFT)),
                ..rgb(0x00, 0x00, 0x00)
            },
            param: Style::default(),
            // Underline rather than a colour: it has to compose with whatever colour the
            // parameter already has, and a second colour would fight with it.
            valid_path: Style {
                underline: true,
                ..Style::default()
            },
            option: rgb(0xff, 0xb8, 0x6c),
            // A glob is the one thing in a line that can turn one word into fifty, and it should
            // be impossible to miss.
            glob: Style {
                bold: true,
                ..rgb(0xff, 0x79, 0xc6)
            },
            glob_nomatch: Style {
                underline: true,
                ..rgb(0xff, 0x55, 0x55)
            },
            coordinate: Style {
                bold: true,
                ..rgb(0x8b, 0xe9, 0xfd)
            },
            number: rgb(0xbd, 0x93, 0xf9),
            assignment: rgb(0x50, 0xfa, 0x7b),
            // Two yellows: a single-quoted string is inert and takes the plainer one, a
            // double-quoted one still expands and is brighter to say so.
            single_quote: rgb(0xd8, 0xdf, 0x6e),
            double_quote: rgb(0xf1, 0xfa, 0x8c),
            escape: rgb(0xff, 0x79, 0xc6),
            operator: rgb(0x8b, 0xe9, 0xfd),
            redirection: rgb(0xff, 0xb8, 0x6c),
            end: rgb(0x62, 0x72, 0xa4),
            comment: rgb(0x62, 0x72, 0xa4),
            variable: rgb(0xbd, 0x93, 0xf9),
            // Colour 240, an explicit index rather than the bright-black slot: a ghost has to sit
            // *behind* the text you are typing, and asking for a specific grey is the only way to
            // say how far behind. The cost of naming an exact grey is that a sixteen-colour
            // terminal rounds it to whichever slot is nearest, which is not necessarily a dim one.
            autosuggestion: Style::fg(Color::Indexed(240)),
            repair: Style {
                reverse: true,
                ..Style::fg(Color::Indexed(240))
            },
            match_bracket: Style {
                bold: true,
                ..Style::default()
            },
        }
    }
}

/// Colours for the completion dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pager {
    /// The background every unselected row is drawn on.
    ///
    /// This is what makes the menu read as a block rather than as loose text: there is no border
    /// and no caption, so the colour is the only thing saying where it starts and stops.
    pub bg: Option<Color>,
    pub text: Style,
    pub text_sel: Style,
    pub sel_bg: Option<Color>,
    /// The background a kind badge takes **on the selected row only**.
    ///
    /// One colour for whichever kind is selected, rather than each kind keeping its own: the
    /// selected row is already marked out by `sel_bg`, and a badge that kept its usual colour
    /// there reads as a second, competing highlight. This is the same job IRIS does by inverting
    /// the pill — a different treatment for the row you are on.
    pub kind_sel: Option<Color>,
    /// The part of a candidate the user has already typed.
    pub match_: Style,
    pub desc: Style,
    pub desc_sel: Style,
    /// Every info column after the description: a file's size, a directory's entry count, what an
    /// alias expands to, and anything `oslo.completion.columns` adds.
    ///
    /// Dimmer than the description on purpose. These columns annotate a candidate where the
    /// description explains it, and a row with four equally loud columns is a row nothing stands
    /// out in — the label is what the eye is looking for.
    pub extra: Style,
    pub extra_sel: Style,
    pub scroll: Style,
    /// The pill, one entry per completion kind.
    pub kind: KindColors,
}

impl Pager {
    /// The style for info column `col`. Column 0 is the description; the rest share one style,
    /// because a theme that had to name a colour per column would break the moment a config added
    /// one more.
    pub fn column(&self, col: usize, selected: bool) -> Style {
        match (col, selected) {
            (0, false) => self.desc,
            (0, true) => self.desc_sel,
            (_, false) => self.extra,
            (_, true) => self.extra_sel,
        }
    }
}

impl Default for Pager {
    fn default() -> Self {
        Pager {
            bg: Some(Color::Indexed(236)),
            text: Style::default(),
            text_sel: Style {
                bold: true,
                ..Style::fg(Color::Basic {
                    index: 7,
                    bright: true,
                })
            },
            sel_bg: Some(Color::Indexed(238)),
            kind_sel: Some(Color::Indexed(242)),
            match_: Style {
                bold: true,
                ..Style::fg(Color::Basic {
                    index: 6,
                    bright: true,
                })
            },
            desc: Style::fg(Color::Indexed(245)),
            desc_sel: Style::fg(Color::Indexed(252)),
            extra: Style::fg(Color::Indexed(242)),
            extra_sel: Style::fg(Color::Indexed(248)),
            scroll: Style::fg(Color::Indexed(240)),
            kind: KindColors::default(),
        }
    }
}

/// The pill colours, by completion kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindColors {
    pub command: Style,
    pub builtin: Style,
    pub file: Style,
    pub dir: Style,
    pub variable: Style,
    pub history: Style,
    pub alias: Style,
    /// Anything a theme has no entry for.
    pub other: Style,
}

impl Default for KindColors {
    fn default() -> Self {
        // Dark text on a coloured field, which is what makes a pill read as a pill rather than as
        // coloured words. Indexed rather than 24-bit so the default theme looks the same on a
        // 256-colour terminal as on a modern one.
        let pill = |bg: u8| Style {
            fg: Some(Color::Indexed(233)),
            bg: Some(Color::Indexed(bg)),
            ..Style::default()
        };
        KindColors {
            command: pill(140),
            // The same pink the line highlighter gives a builtin, so the dropdown and the line
            // agree about what a builtin is. `Indexed(212)` is `#ff87d7`.
            builtin: pill(212),
            file: pill(245),
            dir: pill(240),
            variable: pill(215),
            history: pill(79),
            alias: pill(140),
            other: pill(245),
        }
    }
}

impl KindColors {
    /// The pill for a `CompletionCandidate::kind`.
    pub fn for_kind(&self, kind: &str) -> Style {
        match kind {
            "command" => self.command,
            "builtin" => self.builtin,
            "file" => self.file,
            "dir" | "directory" => self.dir,
            "variable" => self.variable,
            "history" => self.history,
            "alias" => self.alias,
            _ => self.other,
        }
    }
}

#[path = "prompt.rs"]
mod prompt_theme;
pub use prompt_theme::Prompt;

#[cfg(test)]
mod tests {
    use super::*;

    /// A coordinate must not take the same colour as what it sits beside, or the highlighting
    /// says nothing at all. `command` especially: `ssh {0:0}` puts the two next to each other.
    #[test]
    fn a_coordinate_is_a_colour_of_its_own() {
        for (name, s) in [
            ("dark", Syntax::default()),
            ("light", Syntax::for_light_background()),
        ] {
            assert_ne!(s.coordinate, s.command, "{name}: reads as a command");
            assert_ne!(s.coordinate, s.glob, "{name}: reads as a glob");
            assert_ne!(s.coordinate, s.param, "{name}: reads as a plain parameter");
            assert_ne!(s.coordinate, Style::default(), "{name}: unstyled");
        }
    }

    #[test]
    fn the_default_theme_paints_something_for_every_role() {
        let theme = Theme::default();
        let _held = held_at(Depth::Ansi256);
        // Not exhaustive by field name — the point is that no role is left silently plain except
        // the ones that are meant to be.
        assert!(!theme.syntax.command.is_plain());
        assert!(!theme.syntax.error.is_plain());
        assert!(theme.pager.bg.is_some());
        assert!(!theme.pager.kind.command.is_plain());
        // `param` is deliberately plain: an ordinary argument takes the terminal's own colour.
        assert!(theme.syntax.param.is_plain());
    }

    #[test]
    fn an_unknown_kind_still_gets_a_pill() {
        let kinds = KindColors::default();
        assert_eq!(kinds.for_kind("nonesuch"), kinds.other);
        assert_eq!(kinds.for_kind("dir"), kinds.dir);
        // Both spellings, since the completer says `dir` and a Lua theme may say `directory`.
        assert_eq!(kinds.for_kind("directory"), kinds.dir);
    }

    #[test]
    fn installing_a_theme_replaces_the_current_one() {
        let mut theme = Theme::default();
        theme.syntax.command = Style::fg(Color::Indexed(99));
        install(theme.clone());
        assert_eq!(current().syntax.command, Style::fg(Color::Indexed(99)));
        install(Theme::default());
    }
}
