//! `--help`, and `--help --details`.
//!
//! # Colour is detected, never assumed
//!
//! Two questions, both of which have to say yes:
//!
//! - [`Depth::detect`] — `$NO_COLOR`, a `dumb` or empty `$TERM`, and how much colour the terminal
//!   admits to. A serial console gets 16 colours rather than nothing, which for a distro's
//!   `/bin/sh` is not a hypothetical.
//! - **is stdout a terminal** — `oslo --help | less` and `oslo --help > FILE` must not have
//!   escapes in them. `Depth::detect` cannot answer this: it reads the environment, and the
//!   environment is the same either way.
//!
//! There is no `--color` flag and no plain-help spelling. `NO_COLOR=1 oslo --help` is the
//! convention that already exists for this, and piping already does the right thing.
//!
//! # Why the reference is a separate view
//!
//! `--help` that printed all twenty-two shell options would bury the six things somebody running
//! it actually wanted. `--details` is where the long form lives, and the short form says so.

use crate::cli::tools::TOOLS;
use oslo::env::options::ALL;
use oslo::ui::theme::{Color, Depth, Style};
use std::fmt::Write as _;

pub mod menu;

/// How this run should be painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paint {
    depth: Depth,
}

impl Paint {
    /// What the terminal on the other end of stdout can take.
    pub fn detect() -> Paint {
        let depth = if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            Depth::detect()
        } else {
            // A pipe or a file. Escapes here are not colour, they are litter in somebody's
            // `grep`, and `$TERM` says nothing about it either way.
            Depth::None
        };
        Paint { depth }
    }

    /// No colour.
    pub fn plain() -> Paint {
        Paint { depth: Depth::None }
    }

    /// A given depth, for the tests that assert on the escapes themselves.
    #[cfg(test)]
    pub fn at(depth: Depth) -> Paint {
        Paint { depth }
    }

    fn paint(self, text: &str, style: Style) -> String {
        style.paint(text, self.depth)
    }

    /// A section heading.
    pub fn head(self, text: &str) -> String {
        self.paint(
            text,
            Style {
                bold: true,
                ..Style::fg(Color::Indexed(3))
            },
        )
    }

    /// A flag, a tool name, an option letter — the thing you type.
    pub fn key(self, text: &str) -> String {
        self.paint(text, Style::fg(Color::Indexed(6)))
    }

    /// A placeholder like `COMMAND` or `NAME`, which is not typed literally.
    pub fn slot(self, text: &str) -> String {
        self.paint(
            text,
            Style {
                italic: true,
                ..Style::fg(Color::Indexed(5))
            },
        )
    }

    /// An aside: the `(inactive)` mark, the note under a section.
    pub fn dim(self, text: &str) -> String {
        self.paint(text, Style::fg(Color::Indexed(8)))
    }

    /// A painter for one true colour, for a caller that draws many spans in it.
    ///
    /// Returns a closure rather than painting once because the warning box paints its border a
    /// piece at a time, and threading `Paint` through each call reads worse than holding a brush.
    pub fn rgb(self, (r, g, b): (u8, u8, u8)) -> impl Fn(&str) -> String {
        move |text| self.paint(text, Style::fg(Color::Rgb(r, g, b)))
    }
}

/// The width the description column starts at. Wide enough for `--profile=NAME`.
const COLUMN: usize = 20;

/// Formats an aligned help row.
///
/// **A key too wide for the column takes a line of its own** rather than being run into its
/// description. `saturating_sub` gave it zero padding, which read as `$OSLO_SSH_REMOTE_BINwhat oslo
/// is called` — one word made of two, in the one place a person goes to find out what the words are.
pub(crate) fn row(key: &str, painted_key: String, about: &str) -> String {
    let width = key.chars().count();
    if width >= COLUMN {
        return format!("  {painted_key}\n  {}{about}\n", " ".repeat(COLUMN));
    }
    format!("  {}{}{}\n", painted_key, " ".repeat(COLUMN - width), about)
}

/// The two invocation forms.
fn synopsis(paint: Paint) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{}", paint.head("USAGE"));
    let _ = writeln!(
        s,
        "  {} {} {}",
        paint.key("oslo"),
        paint.slot("[option]..."),
        paint.slot("[script [argument]...]")
    );
    let _ = writeln!(
        s,
        "  {} {} {} {} {}",
        paint.key("oslo"),
        paint.slot("[option]..."),
        paint.key("-c"),
        paint.slot("command"),
        paint.slot("[name [argument]...]")
    );
    let _ = writeln!(
        s,
        "  {} {} {}",
        paint.key("oslo"),
        paint.slot("<tool>"),
        paint.slot("[argument]...")
    );
    s
}

/// How wide `text` is on screen: the longest line, escapes not counted.
///
/// **Measured, not assumed.** An escape sequence has no width on a terminal and plenty in a
/// `String`, so `line.len()` would make a coloured page look far wider than it is — and the
/// warning box sized from it would run off the edge of the screen.
pub fn widest(text: &str) -> usize {
    text.lines().map(visible_width).max().unwrap_or(0)
}

/// The printed width of one line, skipping CSI sequences.
fn visible_width(line: &str) -> usize {
    let mut width = 0;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            width += 1;
            continue;
        }
        // `ESC [ … final`, where the final byte ends the sequence and is not itself drawn.
        for c in chars.by_ref() {
            if ('\x40'..='\x7e').contains(&c) && c != '[' {
                break;
            }
        }
    }
    width
}

/// The short help: what somebody running `oslo --help` came for.
pub fn short(paint: Paint) -> String {
    let mut s = synopsis(paint);

    let _ = writeln!(s, "\n{}", paint.head("OPTIONS"));
    for (key, about) in INVOCATION {
        s.push_str(&row(key, paint.key(key), about));
    }
    s.push_str(&row(
        "-e -u -x ...",
        format!(
            "{} {} {} {}",
            paint.key("-e"),
            paint.key("-u"),
            paint.key("-x"),
            paint.dim("...")
        ),
        "set a shell option, as `set` does (`+x` turns one off)",
    ));
    s.push_str(&row(
        "-o NAME",
        format!("{} {}", paint.key("-o"), paint.slot("NAME")),
        "the same by name, for the ones with no letter (`+o` off)",
    ));
    // **Only in a build that has it.** A flag listed here and refused by the parser would be worse
    // than one that is simply absent, and `argc` is behind a cargo feature.
    #[cfg(feature = "argc")]
    s.push_str(&row(
        "--argc-eval FILE",
        format!("{} {}", paint.key("--argc-eval"), paint.slot("FILE")),
        "parse a script's `# @option` comments, for a bash `eval`",
    ));
    for (key, about) in LONG {
        let painted = match key.split_once('=') {
            Some((flag, slot)) => format!("{}={}", paint.key(flag), paint.slot(slot)),
            None => paint.key(key),
        };
        s.push_str(&row(key, painted, about));
    }

    s.push_str(&tools_section(paint));
    s.push_str(&environment_section(paint));

    let _ = write!(
        s,
        "\n{}\n",
        paint.dim("`oslo --help --details` lists every shell option and what it does.")
    );
    s
}

/// The flags that are not `set` options and take no long form.
const INVOCATION: &[(&str, &str)] = &[
    ("-c COMMAND", "run COMMAND, then exit"),
    ("-s", "read commands from standard input"),
    ("-i", "force interactive mode"),
    ("-l, --login", "act as a login shell"),
    ("--norc", "do not read the configuration"),
    ("--noprofile", "do not read /etc/profile or ~/.profile"),
];

/// oslo's own flags. Long-only by design: the short letters belong to POSIX, which claimed the
/// whole alphabet in 1988 — `-h` is `hashall` and `-v` is `verbose`, which is why bash has no
/// `-h`/`-v` either.
///
/// **`--posix` is deliberately not here.** It still works, and the differential suite needs it to
/// run the corpus both ways — but nobody should reach for it, because a shell invoked as `sh`
/// already is one and every other invocation is oslo being itself. Listing it would advertise a
/// choice that is already made. `--help --details` documents `posix` under the shell options,
/// where somebody looking for it will find it.
const LONG: &[(&str, &str)] = &[
    (
        "--func",
        "run a Lua file pure: no oslo.*, io or os; prints what it returns",
    ),
    (
        "--jit",
        "compile hot Lua functions to machine code, in a JIT build",
    ),
    ("--details", "with --help: the full option reference"),
    ("--version", "print the version, then exit"),
    ("--help", "print this message, then exit"),
    // `--` is deliberately absent. It is a separator rather than an option, it means the same
    // thing in every program that uses getopt, and bash does not list it either. Where it does
    // something oslo-specific — forcing a tool name to be read as a path — it is documented under
    // TOOLS, which is where somebody needs it.
];

/// The tools.
///
/// Nothing to install and nothing to mark: a tool is reachable the moment the binary is.
fn tools_section(paint: Paint) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "\n{}  {}",
        paint.head("TOOLS"),
        paint.dim("a script of the same name always wins")
    );
    for tool in TOOLS {
        s.push_str(&row(tool.name, paint.key(tool.name), tool.about));
    }
    // The one place `--` does something oslo-specific, so the one place worth saying it.
    let _ = writeln!(
        s,
        "\n  {}",
        paint.dim("`oslo ./history` and `oslo -- history` read the name as a path instead.")
    );
    s
}

/// The variables you *set* to change what oslo does.
///
/// Three kinds are deliberately absent, and the rule is the same for all of them — **if setting it
/// is not how you change the thing, it does not belong here.**
///
/// - **`$OSLO_MODE`** is written by oslo for prompt functions to read. Setting it changes nothing.
/// - **`$NO_COLOR`** is honoured, but it is a convention oslo did not invent and behaves the same
///   in every program that respects it.
/// - **Anything the config decides.** The editing mode, the toggle key, the theme and the
///   suggestion keys live in `init.lua`, and a variable that shadowed one would be a second
///   place for the same setting to disagree from itself.
///
/// `$PATH`, `$PS1`, `$IFS`, `$ENV` and the rest of POSIX's are absent for a different reason: they
/// behave as POSIX says, and a `/bin/sh` documenting those in its own `--help` would be
/// documenting the shell rather than itself.
const ENVIRONMENT: &[(&str, &str)] = &[
    (
        "OSLO_PROFILE",
        "which history and tracking store to use (default: default)",
    ),
    (
        "OSLO_DEFAULT_MODE",
        "`lua` to start prompts in Lua; anything else is shell",
    ),
    (
        "OSLO_ALLHIST",
        "record `sh -c` commands in the history too (off unless set)",
    ),
    (
        "OSLO_SESSION",
        "this shell's session; exported, so a child can name the one it is in",
    ),
    ("XDG_CONFIG_HOME", "where init.lua lives (~/.config)"),
    (
        "XDG_DATA_HOME",
        "where the store lives (~/.local/share/oslo)",
    ),
];

fn environment_section(paint: Paint) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "\n{}  {}",
        paint.head("ENVIRONMENT"),
        paint.dim("everything else is init.lua, or POSIX's")
    );
    for (name, about) in ENVIRONMENT {
        s.push_str(&row(name, paint.key(name), about));
    }
    s
}

/// `--help --details`: every shell option, what it does, and whether oslo implements it.
pub fn details(paint: Paint) -> String {
    let mut s = short(paint);

    let _ = writeln!(
        s,
        "\n{}  {}",
        paint.head("SHELL OPTIONS"),
        paint.dim("set -x / set -o xtrace / set +x to turn off")
    );

    for option in ALL {
        // The invocation flags are reported by `$-` but `set` cannot change them, so they are not
        // options anybody can pass here — they appear under their own heading below.
        if !option.settable() {
            continue;
        }
        let letter = option
            .letter
            .map_or_else(|| "  ".to_string(), |c| format!("-{c}"));
        let name = option.name.unwrap_or_default();
        let key = format!("{letter} {name}");
        let pad = COLUMN.saturating_sub(key.chars().count());
        let _ = writeln!(
            s,
            "  {} {}{}{}",
            paint.key(&letter),
            paint.key(name),
            " ".repeat(pad),
            option.about
        );
        // **Said here, not discovered by running it.** An option oslo accepts and does not act on
        // is the one thing a script cannot detect, so the reference is where it gets admitted.
        if let Some(why) = option.unsupported {
            let _ = writeln!(
                s,
                "  {}{}",
                " ".repeat(COLUMN + 1),
                paint.dim(&format!("not implemented: {why}"))
            );
        }
    }

    let _ = writeln!(
        s,
        "\n{}  {}",
        paint.head("INVOCATION FLAGS"),
        paint.dim("reported by $-; `set` cannot change them")
    );
    for option in ALL {
        if option.settable() {
            continue;
        }
        let letter = option.letter.map_or_else(String::new, |c| format!("-{c}"));
        s.push_str(&row(&letter, paint.key(&letter), option.about));
    }
    s
}

#[cfg(test)]
#[path = "help/tests.rs"]
mod tests;
