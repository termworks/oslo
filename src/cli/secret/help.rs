//! What `oslo secret --help` says, and the three menus under it.
//!
//! The rows only; how they are drawn is [`crate::cli::help::menu`]'s, so this page and every other
//! tool's are one page with different words in it.

use crate::cli::help::menu::{CALL, Menu, SUBCOMMANDS as HEADING, Sub};

pub(crate) const MENU: Menu = Menu {
    path: &["secret"],
    call: "[--store NAME] <subcommand> [argument]...",
    heading: HEADING,
    subs: SUBCOMMANDS,
    notes: &[
        "Every subcommand takes --store NAME, which is also $OSLO_SECRET_STORE.",
        "A bare `oslo secret` lists the names kept in the store.",
    ],
    nested: &[&KEY, &RECIPIENT, &CIPHER],
};

const SUBCOMMANDS: &[Sub] = &[
    Sub {
        name: "set",
        args: "NAME",
        about: "keep a value, encrypted",
        flags: &[],
        note: "At a terminal it asks for the value, masked. Reading standard input to end of file \
               there would mean typing it in the clear, into the scrollback, and finishing with a \
               Ctrl-D nobody is told about. Piped, it reads stdin — and a trailing newline is \
               dropped, because a token with `\\n` on the end fails authentication in a way that \
               takes an hour to find.",
    },
    Sub {
        name: "get",
        args: "NAME",
        about: "write that value to standard output",
        flags: &[],
        note: "To stdout with nothing added to it, and nowhere else — no file, no clipboard, no \
               newline.",
    },
    Sub {
        name: "run",
        args: "[VAR=NAME]... [flags] -- CMD...",
        about: "run CMD with VAR set to the secret, and nothing else",
        flags: &[
            (
                "--file VAR=NAME",
                "a sealed in-memory file instead, VAR=/dev/fd/N",
            ),
            (
                "--env-file FILE",
                "NAME=value and NAME=secret:[STORE/]NAME lines",
            ),
            ("--all", "every secret, as its name upper-cased, - as _"),
            ("--prefix P", "before each name --all makes"),
        ],
        note: "The value reaches the child through its environment and never through the command \
               line, where every other process on the machine could read it out of `ps`. Repeat \
               `VAR=NAME` for more than one. A variable named twice takes the most explicit: a \
               pair over the file, the file over --all. Every secret is opened before CMD runs, \
               and one that fails stops it with no child started.",
    },
    Sub {
        name: "list",
        args: "",
        about: "the names kept here",
        flags: &[],
        note: "The names, never the values. A store whose crypto is a hook keeps its names \
               wherever that hook put them, and says so rather than answering with an empty list.",
    },
    Sub {
        name: "rm",
        args: "NAME",
        about: "forget one",
        flags: &[],
        note: "Removes the file. Anyone holding an older copy of the store still has it, which is \
               what rotating a leaked credential at its source is for.",
    },
    Sub {
        name: "rotate",
        args: "",
        about: "re-encrypt everything, as the store is now configured",
        flags: &[],
        note: "The deliberate second step after adding a recipient: adding one gives them what is \
               written from then on, and this gives them the rest. Separate because it rewrites \
               every file, and because \"who could read this before I changed it\" is a question \
               with a permanent answer. Every file is written as OSLO3; the OSLO2 ones are named, \
               and OSLO2 is refused in this store from then on.",
    },
    // These three lead to menus of their own: `--help` on one answers with that menu, so the row
    // says what it is and nothing more. Anything written twice is a thing that can disagree with
    // itself.
    Sub {
        name: "key",
        args: "list|add|rm|init",
        about: "where this store's key comes from",
        flags: &[],
        note: "",
    },
    Sub {
        name: "recipient",
        args: "[--export] | add|rm RECIPIENT",
        about: "who its files are written for",
        flags: &[],
        note: "",
    },
    Sub {
        name: "cipher",
        args: "encrypt|decrypt|list|rm -- CMD...",
        about: "hand this store's crypto to another program",
        flags: &[],
        note: "",
    },
    Sub {
        name: "stores",
        args: "",
        about: "every store this machine knows about",
        flags: &[],
        note: "Marking the one a bare `oslo secret` means. A plugin's store is listed under the \
               name the plugin reserved.",
    },
    Sub {
        name: "where",
        args: "",
        about: "the store and the key, and which of them may be committed",
        flags: &[],
        note: "Two directories with opposite rules: the whole point of the store being encrypted \
               is that it can be committed, and the whole point of the key being elsewhere is that \
               it cannot.",
    },
];

/// `oslo secret key`.
pub(crate) const KEY: Menu = Menu {
    path: &["secret", "key"],
    call: CALL,
    heading: HEADING,
    subs: KEYS,
    notes: &[
        "Tried in the order they are written, and the ones that run a program are tried last —",
        "so a store that opens with a key file never forks.",
        "The default is the profile's key, derived per store name: one `oslo profile export`",
        "then carries a machine's history and its secrets both.",
    ],
    nested: &[],
};

const KEYS: &[Sub] = &[
    Sub {
        name: "list",
        args: "",
        about: "what this store will try, in the order it will try it",
        flags: &[],
        note: "With whether each is there, said in the fewest words that are true.",
    },
    Sub {
        name: "add",
        args: "profile | file PATH | command ARG...",
        about: "another place to look for the key",
        flags: &[
            ("profile", "derive it from this profile's key (the default)"),
            ("file PATH", "read the identity out of a file"),
            (
                "command ARG...",
                "run a program; its output is the identity",
            ),
        ],
        note: "A store that already holds files keeps its implied profile key when you add \
               another, because those files are sealed to it and dropping it would make them \
               unreadable. An empty store is being configured instead, so the key you name is the \
               only one it gets. A plugin's store may not run a command.",
    },
    Sub {
        name: "rm",
        args: "file PATH | command ARG...",
        about: "stop looking there",
        flags: &[],
        note: "Written exactly as `list` prints it. Removing the key a file was sealed to does not \
               delete the file — it makes it unreadable, which is worse, so `rotate` first.",
    },
    Sub {
        name: "init",
        args: "",
        about: "generate the key file now, rather than on the first `set`",
        flags: &[],
        note: "Only a store with a key file of its own has one to make: a store on the profile's \
               key is answered by `oslo profile key init`, and one whose crypto is another \
               program's has a key that belongs to that program. Prints the public half, because \
               that is the half that is useful — it goes in somebody else's `recipient add`.",
    },
];

/// `oslo secret recipient`.
pub(crate) const RECIPIENT: Menu = Menu {
    path: &["secret", "recipient"],
    call: CALL,
    heading: HEADING,
    subs: RECIPIENTS,
    notes: &["Adding one does not re-encrypt anything: `oslo secret rotate` is that step."],
    nested: &[],
};

const RECIPIENTS: &[Sub] = &[
    Sub {
        name: "list",
        args: "",
        about: "who this store's files are written for",
        flags: &[("--export", "the list alone, to hand to somebody")],
        note: "A store with none named is written for itself, and `--export` prints the public \
               half that implies so there is something to hand over.",
    },
    Sub {
        name: "add",
        args: "RECIPIENT | --from FILE",
        about: "write future files for them too",
        flags: &[("--from FILE", "read them out of a file, one per line")],
        note: "They can read what is written after today, and what they write opens here. \
               `oslo secret rotate` gives them the rest, and is separate on purpose.",
    },
    Sub {
        name: "rm",
        args: "RECIPIENT | --from FILE",
        about: "stop writing for them",
        flags: &[("--from FILE", "read them out of a file, one per line")],
        note: "Future files only, and what they wrote no longer opens here, so `rotate` before \
               this too. Every file already written is still readable by them, which is what \
               `rotate` after it is for — and what nothing can do about the copies they have.",
    },
];

/// `oslo secret cipher`.
pub(crate) const CIPHER: Menu = Menu {
    path: &["secret", "cipher"],
    call: CALL,
    heading: HEADING,
    subs: CIPHERS,
    notes: &[
        "A subprocess, so it works in cron and in a script — unlike a hook, which only runs",
        "where the config that defined it was read. This is the route to a key oslo cannot",
        "compute itself: one in a YubiKey or a smartcard, where the private half never leaves.",
    ],
    nested: &[],
};

const CIPHERS: &[Sub] = &[
    Sub {
        name: "encrypt",
        args: "-- CMD...",
        about: "plaintext in, ciphertext out",
        flags: &[],
        note: "Standard error is inherited, so a device that wants a touch or a PIN can say so.",
    },
    Sub {
        name: "decrypt",
        args: "-- CMD...",
        about: "ciphertext in, plaintext out",
        flags: &[],
        note: "Set both halves. A store with one handed over and the other not is a store that \
               writes files it cannot read, and the day that is discovered is the worst one.",
    },
    Sub {
        name: "list",
        args: "",
        about: "what does this store's crypto, if not oslo",
        flags: &[],
        note: "Naming the half that is missing, if one is.",
    },
    Sub {
        name: "rm",
        args: "",
        about: "back to oslo's own crypto",
        flags: &[],
        note: "Files written by the other program stay as they are, and oslo cannot read them — \
               `rotate` before this, while the program that can is still configured.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::help::Paint;

    /// Every nested menu is reachable from a row on the front page, or it is undiscoverable.
    #[test]
    fn the_nested_menus_are_listed_above() {
        let front = MENU.overview(Paint::plain());
        for nested in [&KEY, &RECIPIENT, &CIPHER] {
            let name = nested.path[1];
            assert!(front.contains(name), "{name} is not on the front page");
            assert!(
                MENU.subcommand(name, Paint::plain()).is_some(),
                "{name} has no page of its own"
            );
        }
    }
}
