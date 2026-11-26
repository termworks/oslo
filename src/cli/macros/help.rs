//! What `oslo macros --help` says.
//!
//! The rows only; how they are drawn is [`crate::cli::help::menu`]'s, so this page and every other
//! tool's are one page with different words in it.

use crate::cli::help::Paint;
use crate::cli::help::menu::{CALL, Menu, SUBCOMMANDS as HEADING, Sub};

pub(crate) const MENU: Menu = Menu {
    path: &["macros"],
    call: CALL,
    heading: HEADING,
    subs: SUBCOMMANDS,
    notes: &[
        "Aliases and abbreviations reach a shell when it starts; a function is found when",
        "you call it, after $PATH has already failed. A script is a file in ~/.local/sbin",
        "(or $OSLO_MACROS_BIN): any file there with a #! line is listed, and runs from $PATH.",
    ],
    nested: &[],
};

pub(super) const SUBCOMMANDS: &[Sub] = &[
    Sub {
        name: "add",
        args: "--KIND NAME [BODY]",
        about: "store an alias, an abbreviation, a function, a script or a variable",
        flags: &[
            ("--alias", "a word replaced before the line is parsed"),
            (
                "--abbrev",
                "an abbreviation: expanded into the line as you type it",
            ),
            ("--func", "a function: opens your editor, shell or Lua"),
            (
                "--script",
                "a script: a file in ~/.local/sbin, opened in your editor",
            ),
            (
                "--var",
                "an environment variable; the body runs the first time the name is read",
            ),
            ("--tag TAG", "label it; repeat for more than one"),
            (
                "--edit",
                "open the body in the editor instead of taking it as an argument",
            ),
        ],
        note: "The kind is required: five kinds and a silent default is a trap. A BODY on the \
               command line is for an alias, an abbreviation and a variable — a function and a \
               script always open the editor, because neither fits on a line and pretending \
               otherwise invites a function written as one. An alias replaces the word before the \
               line is parsed, so nothing you can see is what runs; an abbreviation is expanded \
               *into your line*, so what runs is what you read and what the history records. A \
               variable is the odd one: its body is a recipe, not a value, and it is run once, in \
               the shell that first reads the name — which is what makes \
               `--var GITHUB_TOKEN='$(oslo secret get gh-token)'` cost nothing until something \
               needs the token. It may be written that way, as one word with an `=`, or as a name \
               and a body. A script's tags are its `# tags: a, b` line; `--tag` rewrites it.",
    },
    Sub {
        name: "remove",
        args: "NAME",
        about: "forget one",
        flags: &[
            ("--alias", "remove the alias of that name"),
            ("--abbrev", "remove the abbreviation"),
            ("--func", "remove the function"),
            ("--script", "remove the script"),
            ("--var", "remove the variable"),
        ],
        note: "The kind is only needed when the name is more than one thing at once — removing \
               the only macro called `gs` is not a question. Removing one that shadowed an alias \
               your configuration defines puts the configured one back, in every shell already \
               running. To keep it and stop it applying, turn it off in `show` instead.",
    },
    Sub {
        name: "edit",
        args: "NAME",
        about: "open one in your editor",
        flags: &[("--alias", "when the name is more than one kind at once")],
        note: "The kind is only needed when the name is more than one thing. A script opens as \
               the file itself. **A function that also exists as a file on `$PATH` is shadowed by \
               that file** — the shell finds a stored one only after the `$PATH` search has failed \
               — and so is a script with a namesake earlier on `$PATH`; this says which file is \
               running before it opens the editor.",
    },
    Sub {
        name: "show",
        args: "[NAME]",
        about: "the manager: the list, narrowed as you type",
        flags: &[
            (
                "--plain",
                "one line each on stdout, for a script rather than a person",
            ),
            ("←  →", "the tag: all of them, then each one in use"),
            (
                "tab",
                "the source: what is stored, or what your config defines",
            ),
            (
                "enter",
                "open it in your editor — every kind, including an alias",
            ),
            ("del", "forget it, after asking"),
            ("space", "off for this session; three times, off everywhere"),
        ],
        note: "A function or a script is many lines and a list of many-line entries is not a list, \
               so each is flattened to one row: when it was made, its kind, its name, its first \
               line and its tags. With a NAME it prints that one in full instead. Piped, or with \
               --plain, there is no widget and no colour — a manager only a person can read would \
               be one you cannot script. A name more than one kind holds is marked `!`, and \
               reported on stderr.",
    },
    Sub {
        name: "off",
        args: "NAME",
        about: "stop it applying, without forgetting it",
        flags: &[
            ("--session", "in this shell only, until it closes"),
            ("--alias", "only that kind of it; by default, every kind"),
        ],
        note: "What Space and Space ×3 do in `show`, spelled out for something that is not a \
               person. Without `--session` it is off in every shell, now and next time — running \
               shells notice before their next prompt; a script moves into `.off/` beside the \
               others, so bash and everything else stop finding it too. An alias you turned off \
               uncovers the one your configuration defines, if there is one, because off means \
               *this one does not apply* rather than *this name does nothing*.",
    },
    Sub {
        name: "on",
        args: "NAME",
        about: "put one back",
        flags: &[("--session", "in this shell only")],
        note: "The other half of `off`, and the same rules. `on --session` cancels a session \
               switch; it does not turn on something that is off everywhere, because those are two \
               different switches and one is not a way to override the other.",
    },
    Sub {
        name: "run",
        args: "NAME [ARG]...",
        about: "run a stored macro from something that is not oslo",
        flags: &[],
        note: "Everything after NAME belongs to the macro, including words that look like options. \
               oslo itself needs none of this — it finds a stored function by name — but bash, \
               tmux, a systemd unit and a `.desktop` file cannot read the database, and this is \
               their way in. A script is a file and needs no door; this runs it too, from \
               wherever `$PATH` is.",
    },
    Sub {
        name: "publish",
        args: "",
        about: "write the derived copies again",
        flags: &[],
        note: "Every change writes the snapshot a starting shell reads and the aliases another \
               shell sources; this writes them again. Ask for it when one of those files was \
               deleted, or when a database was filled before they existed. Scripts are files \
               already and have nothing to publish.",
    },
    Sub {
        name: "export",
        args: "[FILE]",
        about: "write every entry out as text",
        flags: &[],
        note: "A database is not a dotfiles repository: an alias in `init.lua` is \
               version-controlled, diffable and copied to a new machine with the rest of your \
               configuration, and one in here is none of those. This is the way back out. Without \
               a FILE it writes to stdout. A header line carries the kind, the name, `off` if it \
               is turned off and `#tag` for each tag; the body follows, indented by one tab.",
    },
    Sub {
        name: "import",
        args: "[FILE]",
        about: "read entries back in",
        flags: &[("--replace", "forget everything stored first")],
        note: "The format `export` writes. Without `--replace` an entry with a name already \
               stored overwrites that one and the rest are left alone, so importing twice is the \
               same as importing once. A script is written as its file; `--replace` deletes every \
               script file first.",
    },
];

/// The overview.
pub fn text(paint: Paint) -> String {
    MENU.overview(paint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subcommand_has_help_and_a_reason() {
        for sub in SUBCOMMANDS {
            assert!(
                MENU.subcommand(sub.name, Paint::plain()).is_some(),
                "{} has no help",
                sub.name
            );
            assert!(!sub.about.is_empty(), "{} says nothing", sub.name);
            assert!(sub.note.len() > 40, "{}'s note explains nothing", sub.name);
        }
    }

    #[test]
    fn a_name_nobody_listed_has_no_help() {
        assert!(MENU.subcommand("nonsense", Paint::plain()).is_none());
    }

    /// The overview names every subcommand, or one is reachable and undocumented.
    #[test]
    fn the_overview_lists_them_all() {
        let overview = MENU.overview(Paint::plain());
        for sub in SUBCOMMANDS {
            assert!(overview.contains(sub.name), "{} is missing", sub.name);
        }
    }
}
