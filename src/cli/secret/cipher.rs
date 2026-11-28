//! `oslo secret cipher` — hand a store's crypto to another program.
//!
//! ```sh
//! oslo secret --store yubi cipher encrypt -- age -R ~/.config/age/recipients.txt
//! oslo secret --store yubi cipher decrypt -- age --decrypt --identity ~/.config/age/yubikey.txt
//! oslo secret --store yubi cipher list
//! oslo secret --store yubi cipher rm
//! ```
//!
//! This is the route to a key oslo cannot compute itself — one held in a YubiKey, a smartcard, or
//! anything else where the private half never leaves the device. `age` speaks the age plugin
//! protocol and calls `age-plugin-yubikey`; oslo hands it the bytes.

use oslo::secrets::{self, Crypto, Store, cipher, conf};

use super::fail;
use super::help::CIPHER as MENU;
use crate::cli::help::Paint;

pub fn run(store: &Store, args: &[String]) -> i32 {
    if let Some(page) = MENU.asked(args, Paint::detect()) {
        print!("{page}");
        return 0;
    }
    match args.first().map(String::as_str) {
        None | Some("list" | "ls") => {
            show(store);
            0
        }
        Some(half @ ("encrypt" | "decrypt")) => set(store, half, &args[1..]),
        Some("rm" | "remove") => remove(store),
        Some(other) => MENU.unknown(&secrets::recipient::shown(other)),
    }
}

/// What does this store's crypto, if not oslo.
pub fn show(store: &Store) {
    for line in store.crypto.lines() {
        println!("{line}");
    }
    if store.crypto == Crypto::Hook {
        println!("  a hook, so only a shell that has read your config can open this store");
        return;
    }
    let Crypto::Command(cipher) = &store.crypto else {
        return;
    };
    // **Said, rather than left to be noticed.** A store with one half handed over and the other not
    // is a store that writes files it cannot read, and the day that is discovered is the worst one.
    match (&cipher.encrypt, &cipher.decrypt) {
        (Some(_), None) => {
            println!("  no `decrypt command`: this store can be written and not read")
        }
        (None, Some(_)) => {
            println!("  no `encrypt command`: this store can be read and not written")
        }
        _ => {}
    }
}

fn set(store: &Store, half: &str, args: &[String]) -> i32 {
    if store.name.starts_with(secrets::PLUGIN) {
        return fail("a plugin's store may not run a command");
    }
    let argv: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--")
        .collect();
    if argv.is_empty() {
        return MENU.wrong(half, "needs the program to run, after a `--`");
    }
    let line = format!("{half} command {}", argv.join(" "));
    if let Err(e) = cipher::parse(half, &format!("command {}", argv.join(" "))) {
        return fail(&e);
    }

    let mut declared = match conf::read() {
        Ok(conf) => conf,
        Err(e) => return fail(&e),
    };
    let starts = format!("{half} command ");
    declared.remove(&store.name, |written| written.starts_with(&starts));
    declared.add(&store.name, &line);
    match declared.write() {
        Ok(()) => 0,
        Err(e) => fail(&e),
    }
}

fn remove(store: &Store) -> i32 {
    let mut declared = match conf::read() {
        Ok(conf) => conf,
        Err(e) => return fail(&e),
    };
    let gone = declared.remove(&store.name, |written| {
        written.starts_with("encrypt command ") || written.starts_with("decrypt command ")
    });
    if gone == 0 {
        return fail(&format!("{}: does its own crypto already", store.name));
    }
    match declared.write() {
        Ok(()) => 0,
        Err(e) => fail(&e),
    }
}
