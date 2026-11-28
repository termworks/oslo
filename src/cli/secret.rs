//! `oslo secret` — keep a value encrypted, hand it out when something asks.
//!
//! ```sh
//! oslo secret set deploy-token          # asks for the value at a terminal, or reads stdin
//! oslo secret get deploy-token          # writes it to stdout, and nowhere else
//! oslo secret run TOKEN=deploy-token -- curl …
//! oslo secret list
//! oslo secret rm deploy-token
//! ```
//!
//! Every one of them takes `--store NAME`, which is also `$OSLO_SECRET_STORE`. The store, its keys
//! and its recipients are in [`oslo::secrets`]; `key` and `recipient` below are the commands that
//! write that configuration, and `where` is the one that shows it.

use crate::cli::help::Paint;
use help::MENU;
use oslo::secrets::{self, Store};

mod cipher;
pub(crate) mod help;
#[cfg(feature = "crypt")]
mod key;
#[cfg(feature = "crypt")]
mod recipient;
mod run;

pub fn run(args: &[String]) -> i32 {
    if let Some(page) = MENU.asked(args, Paint::detect()) {
        print!("{page}");
        return 0;
    }
    let (asked, args) = match store_argument(args) {
        Ok(taken) => taken,
        Err(e) => return fail(&e),
    };
    let store = || match Store::selected(asked.as_deref()) {
        Ok(store) => {
            warn_about_repositories(&store);
            Ok(store)
        }
        Err(e) => {
            eprintln!("oslo secret: {e}");
            Err(1)
        }
    };

    // The subcommands that read no operand at all. Checked in one place rather than in each arm,
    // because `extra` reports as it answers and asking it twice would say it twice.
    if let Some(name) = args.first().map(String::as_str)
        && matches!(
            name,
            "list" | "ls" | "stores" | "where" | "paths" | "rotate"
        )
        && let Some(bad) = MENU.extra(name, &args, 0)
    {
        return bad;
    }

    match args.first().map(String::as_str) {
        Some("set") => with_name(&args, |name| match store() {
            Ok(store) => set(&store, name),
            Err(code) => code,
        }),
        Some("get") => with_name(&args, |name| match store() {
            Ok(store) => get(&store, name),
            Err(code) => code,
        }),
        Some("rm" | "remove" | "forget") => with_name(&args, |name| match store() {
            Ok(store) => match store.forget(name) {
                Ok(()) => 0,
                Err(e) => fail(&e),
            },
            Err(code) => code,
        }),
        Some("list" | "ls") | None => match store() {
            Ok(store) => {
                // A hook-backed store keeps its names wherever its storage is, which may not be a
                // directory at all — so an empty listing here would be a wrong answer, not a short
                // one.
                if let Some(why) = store.unreachable_here() {
                    return fail(&why);
                }
                // Escaped, so a file somebody committed under a hostile name cannot write to the
                // terminal through a listing.
                for name in store.names() {
                    println!("{}", secrets::name::shown(&name));
                }
                0
            }
            Err(code) => code,
        },
        Some("run") => match store() {
            Ok(store) => run::run(&store, &args[1..]),
            Err(code) => code,
        },
        Some("rotate") => match store() {
            Ok(store) => match store.rotate() {
                Ok(moved) => {
                    println!("{moved} re-encrypted");
                    0
                }
                Err(e) => fail(&e),
            },
            Err(code) => code,
        },
        #[cfg(feature = "crypt")]
        Some("key") => match store() {
            Ok(store) => key::run(&store, &args[1..]),
            Err(code) => code,
        },
        #[cfg(feature = "crypt")]
        Some("recipient" | "recipients") => match store() {
            Ok(store) => recipient::run(&store, &args[1..]),
            Err(code) => code,
        },
        Some("cipher") => match store() {
            Ok(store) => cipher::run(&store, &args[1..]),
            Err(code) => code,
        },
        Some("stores") => stores(),
        // **Two directories with opposite rules, so say which is which.** The whole point of the
        // store being encrypted is that it can be committed; the whole point of the key being
        // elsewhere is that it cannot.
        Some("where" | "paths") => match store() {
            Ok(store) => {
                let unknown = std::path::PathBuf::from("(nowhere: no $HOME)");
                println!(
                    "store     {}   encrypted, safe to commit",
                    store.directory.display()
                );
                // The keys this store actually reads, not a default it may not be using.
                if !store.crypto.is_external() {
                    for path in store.key_paths() {
                        println!("key       {}   never commit this", path.display());
                    }
                    for source in store.keys.iter().filter(|s| s.is_external()) {
                        println!(
                            "key       {}   run when no key file opens it",
                            source.line()
                        );
                    }
                }
                println!(
                    "config    {}   names the keys, so never commit this either",
                    secrets::conf::path().unwrap_or(unknown).display()
                );
                println!();
                // A store whose crypto is another program's has no keys or recipients of oslo's to
                // show, and printing the defaults it is not using would be a lie about what opens
                // these files.
                #[cfg(feature = "crypt")]
                match store.crypto.is_external() {
                    true => cipher::show(&store),
                    false => {
                        key::show(&store);
                        recipient::show(&store);
                    }
                }
                #[cfg(not(feature = "crypt"))]
                cipher::show(&store);
                0
            }
            Err(code) => code,
        },
        Some(other) => MENU.unknown(other),
    }
}

/// Said once, wherever the command is going, for every key file the store reads: a key under a
/// `.git` is one `git add -A` from being published, and the person it happens to did not choose it
/// — they moved a directory.
fn warn_about_repositories(store: &Store) {
    for path in store.key_paths() {
        if let Some(repository) = secrets::in_a_repository(&path) {
            eprintln!(
                "oslo secret: the key {} is inside the git repository at {}",
                path.display(),
                repository.display()
            );
            eprintln!(
                "oslo secret: move it (`key file` in secrets.conf, or $OSLO_SECRET_IDENTITY), \
                 or the next commit publishes it"
            );
        }
    }
}

/// Take `--store NAME` out of the arguments wherever it is before a `--`, so it can be written
/// before or after the command the way a person expects a global option to be. Everything from the
/// `--` on is kept as written: it is the command `run` and `cipher` hand on.
fn store_argument(args: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut name = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            rest.push(arg.clone());
            rest.extend(args.by_ref().cloned());
            break;
        }
        match arg.strip_prefix("--store") {
            Some("") => match args.next() {
                Some(value) => name = Some(value.clone()),
                None => return Err("--store needs the name of a store".to_string()),
            },
            Some(attached) => match attached.strip_prefix('=') {
                Some(value) => name = Some(value.to_string()),
                None => rest.push(arg.clone()),
            },
            None => rest.push(arg.clone()),
        }
    }
    Ok((name, rest))
}

/// **One name, and only one.** `set`, `get` and `rm` each act on a single secret; a second word
/// was read as nothing at all, so `oslo secret rm old new` forgot `old` and said nothing about
/// `new` — a silence that reads exactly like success.
fn with_name(args: &[String], then: impl FnOnce(&str) -> i32) -> i32 {
    let command = args.first().map(String::as_str);
    match args.get(1) {
        Some(name) => match command.and_then(|c| MENU.extra(c, args, 1)) {
            Some(bad) => bad,
            None => then(name),
        },
        None => match command {
            Some(command) => MENU.wrong(command, "needs the name of a secret"),
            None => MENU.missing("needs a subcommand"),
        },
    }
}

/// Read the value from standard input and keep it.
///
/// **A trailing newline is dropped**, because the value came from a line somebody typed or from a
/// `printf` in a script, and a token with `\n` on the end fails authentication in a way that takes
/// an hour to find.
///
/// **At a terminal it is asked for instead**, masked, by the shell's own `ui input`. Standard input
/// there is the keyboard, so reading it to end of file means the value is typed in the clear, into
/// the scrollback, and finished with a Ctrl-D nobody is told about.
fn set(store: &Store, name: &str) -> i32 {
    use std::io::{IsTerminal, Read};
    let mut value = Vec::new();
    if std::io::stdin().is_terminal() {
        let asked = oslo_ui::ask::input(&oslo_ui::ask::Input {
            prompt: format!("{name}: "),
            password: true,
            required: true,
            ..Default::default()
        });
        match asked {
            oslo_ui::ask::Answer::Given(typed) => value = typed.into_bytes(),
            oslo_ui::ask::Answer::Cancelled => return 1,
            oslo_ui::ask::Answer::NoTerminal => return fail("nothing to read the value from"),
        }
    } else if let Err(e) = std::io::stdin().read_to_end(&mut value) {
        return fail(&format!("cannot read standard input: {e}"));
    }
    if value.last() == Some(&b'\n') {
        value.pop();
    }
    match store.set(name, &value) {
        Ok(()) => 0,
        Err(e) => fail(&e),
    }
}

/// Write the value to standard output, with nothing added to it.
fn get(store: &Store, name: &str) -> i32 {
    use std::io::Write;
    match store.get(name) {
        Ok(value) => {
            let mut out = std::io::stdout();
            if out.write_all(&value).is_err() || out.flush().is_err() {
                return fail("cannot write the value");
            }
            0
        }
        Err(e) => fail(&e),
    }
}

/// Every store declared, and the one a bare `oslo secret` means.
fn stores() -> i32 {
    let declared = match secrets::conf::read() {
        Ok(conf) => conf,
        Err(e) => return fail(&e),
    };
    let mut names: Vec<String> = declared.sections.iter().map(|s| s.name.clone()).collect();
    if !names.iter().any(|name| name == secrets::USER) {
        names.insert(0, secrets::USER.to_string());
    }
    let default = declared
        .default
        .unwrap_or_else(|| secrets::USER.to_string());
    for name in names {
        let here = if name == default { "*" } else { " " };
        match Store::named(&name) {
            Ok(store) => println!("{here} {name}\t{}", store.directory.display()),
            Err(e) => println!("{here} {name}\t{e}"),
        }
    }
    0
}

pub fn fail(message: &str) -> i32 {
    eprintln!("oslo secret: {message}");
    1
}
