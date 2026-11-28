//! `oslo secret key` — where a store's keys come from.
//!
//! ```sh
//! oslo secret key list
//! oslo secret key add profile                       # the default: derived from the profile's key
//! oslo secret key add file ~/.ssh/oslo-identity
//! oslo secret key add command -- pass show oslo/age-identity
//! oslo secret key rm file ~/.ssh/oslo-identity
//! ```
//!
//! A store with several is a store with several places to look, tried in the order they are
//! written, and the ones that run a program are tried last however they are ordered — so a store
//! that opens with a key file never forks.
//!
//! **The default is the profile's key**, derived per store name. That is what makes one
//! `oslo profile export` carry a machine's history *and* its secrets: there is one secret on the
//! machine rather than two, and one thing to copy.

use oslo::secrets::{self, KeySource, Store};

use super::fail;
use super::help::KEY as MENU;
use crate::cli::help::Paint;

pub fn run(store: &Store, args: &[String]) -> i32 {
    if let Some(page) = MENU.asked(args, Paint::detect()) {
        print!("{page}");
        return 0;
    }
    match args.first().map(String::as_str) {
        Some("list" | "ls") | None => {
            show(store);
            0
        }
        #[cfg(feature = "crypt")]
        Some("init") => init(store),
        Some("profile") => add(store, &KeySource::Profile),
        Some("add") => match source(&args[1..]) {
            Ok(source) => add(store, &source),
            Err(e) => fail(&e),
        },
        Some("rm" | "remove") => match source(&args[1..]) {
            Ok(source) => remove(store, &source),
            Err(e) => fail(&e),
        },
        Some(other) => MENU.unknown(&secrets::recipient::shown(other)),
    }
}

/// What the store will try, in the order it will try it.
pub fn show(store: &Store) {
    for source in store.keys.iter().filter(|s| !s.is_external()) {
        println!("{}   {}", source.line(), state_of(source));
    }
    for source in store.keys.iter().filter(|s| s.is_external()) {
        println!("{}   {}", source.line(), state_of(source));
    }
}

/// Whether it is there, said in the fewest words that are true.
fn state_of(source: &KeySource) -> &'static str {
    match source {
        KeySource::Profile => "derived from this profile's key",
        KeySource::File(path) if path.exists() => "present",
        KeySource::File(_) => "not there yet",
        KeySource::Command(_) if secrets::key::no_exec() => "skipped: $OSLO_SECRET_NO_EXEC",
        KeySource::Command(_) => "run only when no key file opens it",
    }
}

/// `profile`, `file PATH` or `command ARG…`, with an optional `--` before the program.
fn source(args: &[String]) -> Result<KeySource, String> {
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--")
        .collect();
    if words.is_empty() {
        return Err("a key is `file PATH` or `command ARG…`".to_string());
    }
    KeySource::parse(&words.join(" "))
}

fn add(store: &Store, source: &KeySource) -> i32 {
    if store.name.starts_with(secrets::PLUGIN) && source.is_external() {
        return fail("a plugin's store may not run a command");
    }
    let mut conf = match secrets::conf::read() {
        Ok(conf) => conf,
        Err(e) => return fail(&e),
    };
    // **Only a store that already holds something keeps its implied key.** Every store's key is the
    // profile's until it says otherwise, so a store with files in it is sealed to that and dropping
    // it would make them unreadable — but a store with nothing in it is being *configured*, and
    // prepending a key the person did not ask for would mean the one they named never gets used.
    if conf.section(&store.name).is_none_or(|s| s.keys.is_empty())
        && !matches!(source, KeySource::Profile)
        && !store.names().is_empty()
    {
        conf.add(&store.name, &KeySource::Profile.line());
    }
    conf.add(&store.name, &source.line());
    match conf.write() {
        Ok(()) => 0,
        Err(e) => fail(&e),
    }
}

fn remove(store: &Store, source: &KeySource) -> i32 {
    let mut conf = match secrets::conf::read() {
        Ok(conf) => conf,
        Err(e) => return fail(&e),
    };
    // Compared as key sources rather than as text, so `~/k` written by hand and the `/home/you/k`
    // it expands to are the same key.
    let gone = conf.remove(&store.name, |written| {
        written
            .strip_prefix("key ")
            .and_then(|rest| KeySource::parse(rest).ok())
            .is_some_and(|written| &written == source)
    });
    if gone == 0 {
        return fail(&format!(
            "{}: {} is not one of its keys",
            store.name,
            source.line()
        ));
    }
    match conf.write() {
        Ok(()) => 0,
        Err(e) => fail(&e),
    }
}

/// Make this store's key file now, rather than on the first `set`.
///
/// Only a build with oslo's own age has a key of its own to make; otherwise the key belongs to
/// whatever program the store names.
#[cfg(feature = "crypt")]
fn init(store: &Store) -> i32 {
    let Some(path) = store.key_file() else {
        return fail(&format!(
            "{}: its key is the profile's — `oslo profile key init` makes that one, and \
             `oslo profile export` carries it to another machine",
            store.name
        ));
    };
    if path.exists() {
        eprintln!("oslo secret key: {} is already there", path.display());
        return 1;
    }
    match secrets::key::generate(&path) {
        // The public half is printed because it is the half that is *useful*: it goes in somebody
        // else's `recipient add`, and there is nowhere else to read it from.
        Ok(secret) => {
            println!("{}", path.display());
            println!(
                "{}",
                secrets::native::write_public(&secrets::native::public_of(&secret))
            );
            0
        }
        Err(e) => fail(&e),
    }
}
