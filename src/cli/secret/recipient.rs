//! `oslo secret recipient` — who a store's files are written for.
//!
//! ```sh
//! oslo secret recipient                        # the list
//! oslo secret recipient --export > RECIPIENTS  # to hand to somebody
//! oslo secret recipient add OSLO-PUB-1:…
//! oslo secret recipient add --from RECIPIENTS
//! oslo secret recipient rm OSLO-PUB-1:…
//! ```
//!
//! **Adding one does not re-encrypt anything.** A recipient added today can read what is written
//! after today; `oslo secret rotate` is the separate, deliberate step that gives them the rest, and
//! it is separate because it rewrites every file in the store and because "who could read this
//! before I changed it" is a question with a permanent answer.

use oslo::secrets::{Recipient, Store, conf};

use super::fail;
use super::help::RECIPIENT as MENU;
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
        Some("--export") => {
            if store.recipients.is_empty() {
                return match mine(store) {
                    Ok(Some(published)) => {
                        println!("{published}");
                        0
                    }
                    Ok(None) => fail(&format!(
                        "{}: no key yet — write a secret, or `oslo profile key init`",
                        store.name
                    )),
                    Err(e) => fail(&e),
                };
            }
            for recipient in &store.recipients {
                println!("{recipient}");
            }
            0
        }
        Some("add") => match wanted(&args[1..]) {
            Ok(recipients) => add(store, &recipients),
            Err(e) => fail(&e),
        },
        Some("rm" | "remove") => match wanted(&args[1..]) {
            Ok(recipients) => remove(store, &recipients),
            Err(e) => fail(&e),
        },
        Some(other) => MENU.unknown(&oslo::secrets::recipient::shown(other)),
    }
}

/// The list, with what this binary makes of each.
pub fn show(store: &Store) {
    if store.recipients.is_empty() {
        // **The implied one is printed, not merely named.** It is the half somebody else puts in
        // their `recipient add`, and with the key derived from the profile there is nowhere else to
        // read it from — `key init` used to be that place and no longer exists.
        match mine(store) {
            Ok(Some(published)) => println!("recipient {published}   implied, until one is added"),
            Ok(None) => {
                println!("recipient (none yet — write a secret, or `oslo profile key init`)")
            }
            Err(e) => println!("recipient (unreadable: {e})"),
        }
        return;
    }
    for recipient in &store.recipients {
        match recipient.public() {
            Ok(_) => println!("recipient {recipient}"),
            Err(e) => println!("recipient UNUSABLE: {e}"),
        }
    }
}

/// This store's own published half, when it has a key to derive one from.
///
/// **Only when the key already exists.** Asking who can read a store must not be the thing that
/// creates a key, so this looks and does not make.
fn mine(store: &Store) -> Result<Option<String>, String> {
    let profile = oslo::track::profile::current();
    let Some(key) = oslo::track::profile::key::read(&profile)? else {
        return Ok(None);
    };
    let derived = oslo::secrets::native::derive_store_key(&key, &store.name)?;
    Ok(Some(oslo::secrets::native::write_public(
        &oslo::secrets::native::public_of(&derived),
    )))
}

/// The recipients named on the command line, or read out of the file `--from` names.
///
/// A file is one per line with `#` comments — the shape `--export` writes, so handing somebody a
/// recipient and taking one back are the same file.
fn wanted(args: &[String]) -> Result<Vec<Recipient>, String> {
    if let Some(rest) = args.first().and_then(|a| a.strip_prefix("--from")) {
        let path = match rest.strip_prefix('=') {
            Some(path) => path.to_string(),
            None => args.get(1).cloned().ok_or("--from needs a file")?,
        };
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        return text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(Recipient::new)
            .collect();
    }
    if args.is_empty() {
        return Err("a recipient, or --from FILE".to_string());
    }
    args.iter().map(|arg| Recipient::new(arg)).collect()
}

/// **Checked before it is written.** A recipient this binary cannot use would fail on the next
/// `set` rather than now, which is the wrong end of the mistake to find out at.
fn add(store: &Store, recipients: &[Recipient]) -> i32 {
    for recipient in recipients {
        if let Err(e) = recipient.public() {
            return fail(&e);
        }
    }
    let _held = match conf::locked() {
        Ok(held) => held,
        Err(e) => return fail(&e),
    };
    let mut declared = match conf::read() {
        Ok(conf) => conf,
        Err(e) => return fail(&e),
    };
    // The implied recipient is this store's own key, and it stops being implied the moment a list
    // exists — so it is written down before anything joins it, or adding a colleague would quietly
    // stop encrypting to you.
    if store.recipients.is_empty() {
        match store.key_to_write_with() {
            Ok(key) => {
                let ours =
                    oslo::secrets::native::write_public(&oslo::secrets::native::public_of(&key));
                declared.add(&store.name, &format!("recipient {ours}"));
            }
            Err(e) => return fail(&e),
        }
    }
    for recipient in recipients {
        declared.add(&store.name, &format!("recipient {recipient}"));
    }
    match declared.write() {
        Ok(()) => 0,
        Err(e) => fail(&e),
    }
}

fn remove(store: &Store, recipients: &[Recipient]) -> i32 {
    let _held = match conf::locked() {
        Ok(held) => held,
        Err(e) => return fail(&e),
    };
    let mut declared = match conf::read() {
        Ok(conf) => conf,
        Err(e) => return fail(&e),
    };
    let mut gone = 0;
    for recipient in recipients {
        let line = format!("recipient {recipient}");
        gone += declared.remove(&store.name, |written| written == line);
    }
    if gone == 0 {
        return fail(&format!("{}: none of those are its recipients", store.name));
    }
    match declared.write() {
        Ok(()) => 0,
        Err(e) => fail(&e),
    }
}
