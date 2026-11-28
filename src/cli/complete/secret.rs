//! `oslo secret <Tab>`: the subcommands and their flags, the names a store keeps, the stores, and
//! a store's recipients.
//!
//! ```console
//! $ oslo secret get <Tab>            the names in the store this line means
//! $ oslo secret run GH_TOKEN=<Tab>   the same names, after the `=`
//! $ oslo secret run gh<Tab>          GH_TOKEN=, for each name a variable can be made of
//! $ oslo secret --store <Tab>        secrets.conf's stores, and the ones on disk
//! $ oslo secret recipient rm <Tab>   the recipients secrets.conf lists for the store
//! ```
//!
//! The rows and their one-line descriptions are [`crate::cli::secret::help`]'s, the same rows
//! `--help` prints. The answers are read from disk without opening anything: a name is a file's
//! header, never its sealed body, so nothing is decrypted to complete.

use crate::cli::help::menu::{Menu, Sub};
use crate::cli::secret::help;
use oslo::secrets::{self, Store};
use oslo_ui::spec::action::Offer;
use oslo_ui::spec::{Action, Arg, OptionSpec, Query, SubcommandSpec};
use std::path::Path;
use std::rc::Rc;

/// The spec under `oslo secret`.
pub(super) fn spec(about: &str) -> SubcommandSpec {
    let subcommands = rows(&help::MENU)
        .into_iter()
        .map(|mut sub| {
            match sub.name.as_str() {
                "set" | "get" | "rm" => sub.positional = vec![call(names)],
                "run" => {
                    sub.positional_any = call(variables);
                    for flag in &mut sub.options {
                        flag.values = match flag.names[0].as_str() {
                            "--file" => call(variables),
                            "--env-file" => files(),
                            // Free text: an empty list answers it, so no file name is offered.
                            _ => Action::List(Vec::new()),
                        };
                    }
                }
                "key" => sub.subcommands = rows(&help::KEY),
                "recipient" => {
                    sub.subcommands = rows(&help::RECIPIENT);
                    sub.options = vec![flag("--export", "the list alone, to hand to somebody")];
                    for nested in &mut sub.subcommands {
                        for from in nested
                            .options
                            .iter_mut()
                            .filter(|f| f.takes == Arg::Required)
                        {
                            from.values = files();
                        }
                        if nested.name == "rm" {
                            nested.positional_any = call(recipients);
                        }
                    }
                }
                "cipher" => sub.subcommands = rows(&help::CIPHER),
                _ => {}
            }
            sub
        })
        .collect();
    SubcommandSpec {
        name: "secret".to_string(),
        description: about.to_string(),
        subcommands,
        options: vec![flag("-h, --help", "what this tool does, and its options")],
        persistent: vec![OptionSpec {
            values: call(stores),
            ..flag("--store NAME", "the store to act on, for this invocation")
        }],
        ..SubcommandSpec::default()
    }
}

/// One subcommand per row of `menu`, with the row's flags as its own.
fn rows(menu: &Menu) -> Vec<SubcommandSpec> {
    menu.subs
        .iter()
        .filter(|row| cfg!(feature = "crypt") || !matches!(row.name, "key" | "recipient"))
        .map(|row| {
            let (flags, words): (Vec<_>, Vec<_>) = row
                .flags
                .iter()
                .partition(|(name, _)| name.starts_with('-'));
            let choices: Vec<String> = words
                .iter()
                .map(|(word, about)| {
                    let word = word.split_whitespace().next().unwrap_or_default();
                    format!("{word}\t{about}")
                })
                .collect();
            SubcommandSpec {
                name: row.name.to_string(),
                aliases: aliases(row),
                description: row.about.to_string(),
                options: flags
                    .iter()
                    .map(|(name, about)| flag(name, about))
                    .collect(),
                positional: match choices.is_empty() {
                    true => Vec::new(),
                    false => vec![Action::List(choices), files()],
                },
                ..SubcommandSpec::default()
            }
        })
        .collect()
}

/// The other spellings each subcommand answers to.
fn aliases(row: &Sub) -> Vec<String> {
    let names: &[&str] = match row.name {
        "list" => &["ls"],
        "rm" => &["remove", "forget"],
        "recipient" => &["recipients"],
        "where" => &["paths"],
        _ => &[],
    };
    names.iter().map(|name| name.to_string()).collect()
}

/// A flag as a help row writes it: `-h, --help`, or `--from FILE` for one that takes a value.
fn flag(written: &str, about: &str) -> OptionSpec {
    let (names, value) = match written.split_once(' ') {
        Some((names, value)) if !names.ends_with(',') => (names, Some(value)),
        _ => (written, None),
    };
    OptionSpec {
        takes: match value {
            Some(_) => Arg::Required,
            None => Arg::None,
        },
        ..OptionSpec::new(
            names.split(", ").map(str::to_string).collect(),
            about.to_string(),
        )
    }
}

fn files() -> Action {
    Action::list(["$files"])
}

fn call(answer: fn(&Query) -> Vec<Offer>) -> Action {
    Action::Call(Rc::new(answer))
}

/// The store the line means: its `--store`, else what `oslo secret` would pick.
fn store_of(query: &Query) -> Option<Store> {
    Store::selected(query.flags.get("STORE").map(String::as_str)).ok()
}

/// The names the store keeps.
fn names(query: &Query) -> Vec<Offer> {
    let names = store_of(query)
        .map(|store| store.names())
        .unwrap_or_default();
    past(names, &query.before)
}

/// Each value that starts with `cut` — what a `=` or `:` break took off the word — less that part.
fn past(values: Vec<String>, cut: &str) -> Vec<Offer> {
    values
        .into_iter()
        .filter_map(|value| value.strip_prefix(cut).map(Offer::plain))
        .collect()
}

/// A `run` word: a name after `VAR=`, else `VAR=` for each name that makes a variable.
fn variables(query: &Query) -> Vec<Offer> {
    let names = store_of(query)
        .map(|store| store.names())
        .unwrap_or_default();
    variable_offers(names, &query.before)
}

fn variable_offers(names: Vec<String>, before: &str) -> Vec<Offer> {
    if let Some((variable, cut)) = before.split_once('=')
        && !variable.is_empty()
        && !variable.starts_with('-')
    {
        return past(names, cut);
    }
    if !before.is_empty() {
        return Vec::new();
    }
    names
        .into_iter()
        .filter_map(|name| {
            let variable = name.to_uppercase().replace('-', "_");
            // Only where `VAR=` alone reads back as this name.
            (variable.to_lowercase().replace('_', "-") == name
                && variable.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && variable
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .then(|| Offer {
                value: format!("{variable}="),
                description: Some(name),
                tag: None,
            })
        })
        .collect()
}

/// Every store `--store` could name: `user`, `secrets.conf`'s sections, then the directories.
fn stores(_: &Query) -> Vec<Offer> {
    let declared = secrets::conf::read()
        .map(|conf| conf.sections.into_iter().map(|s| s.name).collect())
        .unwrap_or_default();
    let data = secrets::directory().and_then(|user| user.parent().map(Path::to_path_buf));
    store_offers(declared, data.as_deref())
}

fn store_offers(declared: Vec<String>, data: Option<&Path>) -> Vec<Offer> {
    let mut offers: Vec<Offer> = Vec::new();
    let mut add = |name: String, about: &str| {
        if !offers.iter().any(|offer| offer.value == name) {
            offers.push(Offer {
                value: name,
                description: Some(about.to_string()),
                tag: None,
            });
        }
    };
    add(secrets::USER.to_string(), "built in");
    for name in declared {
        add(name, "in secrets.conf");
    }
    let listed = |directory: &Path| -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    if let Some(data) = data {
        for name in listed(&data.join("stores")) {
            if secrets::conf::valid_store_name(&name).is_ok() {
                add(name, "on disk");
            }
        }
        for name in listed(&data.join("plugins")) {
            if let Some(plugin) = name.strip_suffix(".secrets") {
                add(format!("{}{plugin}", secrets::PLUGIN), "a plugin's own");
            }
        }
    }
    offers
}

/// The recipients `secrets.conf` lists for the store.
fn recipients(query: &Query) -> Vec<Offer> {
    let listed = store_of(query)
        .map(|store| store.recipients.iter().map(ToString::to_string).collect())
        .unwrap_or_default();
    past(listed, &query.before)
}

#[cfg(test)]
#[path = "secret/tests.rs"]
mod tests;
