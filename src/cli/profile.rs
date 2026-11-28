//! `oslo profile` — whose history this is, and how two machines agree about it.
//!
//! ```sh
//! oslo profile                       # every profile, marking the one this shell is using
//! oslo profile show [NAME]           # where its files are, and its fingerprint
//! oslo profile key init [NAME]       # give it a key, so it can be synced
//! oslo profile export [NAME]         # the key, to carry to the other machine
//! oslo profile import                # read one from standard input
//! oslo profile sync user@host        # two-way, over ssh
//! ```
//!
//! # Why a key at all
//!
//! `default` here and `default` on a box you have an account on are two histories that share a
//! word. Syncing them because the words match would merge somebody else's commands into yours, so
//! a profile carries a key and [`sync`] refuses unless both ends hold the same one. Carrying it
//! across once is the step that says *these two are one profile*.
//!
//! The key is in `$XDG_STATE_HOME`, never in the store — the store is the thing that travels.

use crate::cli::help::Paint;
use help::MENU;
use oslo::track::profile;

pub(crate) mod help;
mod sync;

pub fn run(args: &[String]) -> i32 {
    if let Some(page) = MENU.asked(args, Paint::detect()) {
        print!("{page}");
        return 0;
    }
    let named = |at: usize| -> String {
        args.get(at)
            .filter(|word| !word.starts_with('-'))
            .cloned()
            .unwrap_or_else(profile::current)
    };

    match args.first().map(String::as_str) {
        // Each of these names one profile at most, so a second word is a mistake rather than a
        // second profile — `oslo profile show a b` reported on `a` and said nothing about `b`.
        None => list(),
        Some(name @ ("list" | "ls")) => MENU.extra(name, args, 0).unwrap_or_else(list),
        Some("show") => MENU
            .extra("show", args, 1)
            .unwrap_or_else(|| show(&named(1))),
        Some("fingerprint") => MENU
            .extra("fingerprint", args, 1)
            .unwrap_or_else(|| fingerprint(&named(1))),
        Some("key") => key(&args[1..]),
        Some("export") => MENU
            .extra("export", args, 1)
            .unwrap_or_else(|| export(&named(1))),
        Some("import") => MENU
            .extra("import", args, 1)
            .unwrap_or_else(|| import(&named(1))),
        Some("sync") => sync::run(&args[1..]),
        // The two halves the far end of a sync runs, over ssh rather than typed. In the help
        // because a command that exists and is undocumented is worse than one that does not.
        Some("send") => crate::cli::sync::part::send(&args[1..]),
        Some("receive") => crate::cli::sync::part::receive(&args[1..]),
        Some(other) => MENU.unknown(other),
    }
}

/// Every profile there is, with the one this shell uses marked.
fn list() -> i32 {
    let here = profile::current();
    for name in profile::available() {
        let mark = if name == here { "*" } else { " " };
        let synced = match profile::key::read(&name) {
            Ok(Some(key)) => profile::key::fingerprint(&key),
            // Said rather than left blank: a profile with no key is not broken, it simply cannot be
            // synced yet, and that is the one thing somebody looking at this list wants to know.
            Ok(None) => "no key — `oslo profile key init` to make one".to_string(),
            Err(e) => format!("unreadable: {e}"),
        };
        println!("{mark} {name}\t{synced}");
    }
    0
}

fn show(name: &str) -> i32 {
    if !profile::valid(name) {
        return fail(&format!("{name:?} is not a usable profile name"));
    }
    let data = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").ok();
    match profile::profile_dir(data.as_deref(), home.as_deref(), name) {
        Some(dir) => println!("store       {}", dir.display()),
        None => println!("store       (nowhere: no $XDG_DATA_HOME and no $HOME)"),
    }
    match profile::key::path(name) {
        Some(path) => println!(
            "key         {}   never commit this, never sync it",
            path.display()
        ),
        None => println!("key         (nowhere)"),
    }
    match profile::key::read(name) {
        Ok(Some(key)) => println!("fingerprint {}", profile::key::fingerprint(&key)),
        Ok(None) => println!("fingerprint (none yet — `oslo profile key init {name}`)"),
        Err(e) => return fail(&e),
    }
    0
}

/// The one thing `sync` sends over the wire, and the one thing a person compares by eye.
fn fingerprint(name: &str) -> i32 {
    match profile::key::read(name) {
        Ok(Some(key)) => {
            println!("{}", profile::key::fingerprint(&key));
            0
        }
        Ok(None) => fail(&format!(
            "{name}: has no key yet — `oslo profile key init {name}`"
        )),
        Err(e) => fail(&e),
    }
}

fn key(args: &[String]) -> i32 {
    if let Some(page) = help::KEY.asked(args, Paint::detect()) {
        print!("{page}");
        return 0;
    }
    let named = |at: usize| -> String {
        args.get(at)
            .filter(|word| !word.starts_with('-'))
            .cloned()
            .unwrap_or_else(profile::current)
    };
    match args.first().map(String::as_str) {
        Some("init") => match profile::key::generate(&named(1)) {
            Ok(key) => {
                println!("{}", profile::key::fingerprint(&key));
                0
            }
            Err(e) => fail(&e),
        },
        Some("path") => match profile::key::path(&named(1)) {
            Some(path) => {
                println!("{}", path.display());
                0
            }
            None => fail("no $XDG_STATE_HOME and no $HOME, so there is nowhere for one"),
        },
        None => help::KEY.missing("key needs `init` or `path`"),
        Some(other) => help::KEY.unknown(other),
    }
}

/// The key itself, for carrying to the machine that is to share this profile.
///
/// **To standard output and nothing else.** No file is written and no path is printed beside it, so
/// `oslo profile export | ssh other oslo profile import` is one line and leaves nothing behind on
/// either end.
fn export(name: &str) -> i32 {
    match profile::key::read(name) {
        Ok(Some(key)) => {
            print!("{}", profile::key::encode(&key).as_str());
            0
        }
        Ok(None) => fail(&format!(
            "{name}: has no key to export — `oslo profile key init {name}`"
        )),
        Err(e) => fail(&e),
    }
}

/// Take a key from standard input and make it this profile's.
///
/// **Replacing whatever was there**, because that is what importing means: the machine that already
/// has the profile is the authority, and this one is joining it.
fn import(name: &str) -> i32 {
    use std::io::Read;
    let mut text = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut text) {
        return fail(&format!("cannot read standard input: {e}"));
    }
    let key = match profile::key::decode(&text) {
        Ok(key) => key,
        Err(e) => return fail(&e),
    };
    match profile::key::install(name, &key) {
        Ok(path) => {
            println!("{}\t{}", profile::key::fingerprint(&key), path.display());
            0
        }
        Err(e) => fail(&e),
    }
}

pub(super) fn fail(message: &str) -> i32 {
    eprintln!("oslo profile: {message}");
    1
}
