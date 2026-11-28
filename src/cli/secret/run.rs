//! `oslo secret run VAR=NAME … -- CMD…` — one value, one child, and nowhere else.
//!
//! # Why this exists beside `$(oslo secret get …)`
//!
//! A command substitution puts the value in the *calling shell*, where it is a variable that every
//! later command inherits, that a `set` prints, and that a crash dump or a `/proc/PID/environ` read
//! can find for as long as that shell lives. This runs one program with one extra variable in its
//! environment and no shell in between, so the value's lifetime is the child's.
//!
//! It is still the child's environment, which is readable by the user who started it — that is what
//! an environment is. What it buys is that the value is not in anything else.
//!
//! # More than one at a time
//!
//! ```sh
//! oslo secret run --env-file .env -- ./deploy      # the variables a dotenv file lists
//! oslo secret run --all --prefix APP_ -- ./serve   # every secret, APP_GH_TOKEN and so on
//! oslo secret run --file KUBECONFIG=kube -- kubectl get pods
//! ```
//!
//! A variable named in more than one place takes the most explicit: a `VAR=NAME` pair over the
//! file, the file over `--all`. Every secret is opened before the command runs, and the first one
//! that fails stops it with no child started. See [`envfile`] and [`memfd`].

use oslo::secrets::{Store, Zeroizing};

use super::fail;
use super::help::MENU;

mod asked;
mod envfile;
mod memfd;

/// Where one variable's value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Wanted {
    Plain(String),
    /// A secret, from this store or the one named, as a value or as a file.
    Secret {
        store: Option<String>,
        name: String,
        file: bool,
    },
}

pub fn run(store: &Store, args: &[String]) -> i32 {
    let Some(at) = args.iter().position(|arg| arg == "--") else {
        return MENU.wrong("run", "needs a `--` before the command to run");
    };
    let (wanted, command) = args.split_at(at);
    let command = &command[1..];
    let asked = match asked::parse(wanted) {
        Ok(asked) => asked,
        Err(e) => return fail(&e),
    };
    if asked.is_empty() || command.is_empty() {
        return MENU.wrong("run", "needs both a VAR=NAME and a command");
    }
    let plan = match plan(store, &asked) {
        Ok(plan) => plan,
        Err(e) => return fail(&e),
    };

    let mut environment = Vec::with_capacity(plan.len());
    // Held open until the exec, which is what hands each one to the child.
    let mut files = Vec::new();
    for (variable, wanted) in plan {
        let (from, name, file) = match wanted {
            Wanted::Plain(value) => {
                environment.push((variable, Zeroizing::new(value)));
                continue;
            }
            Wanted::Secret { store, name, file } => (store, name, file),
        };
        let other = match from.as_deref().map(Store::named).transpose() {
            Ok(other) => other,
            Err(e) => return fail(&e),
        };
        match other.as_ref().unwrap_or(store).get(&name) {
            Ok(value) if file => match memfd::sealed(&value) {
                Ok(fd) => {
                    environment.push((variable, Zeroizing::new(memfd::path(&fd))));
                    files.push(fd);
                }
                Err(e) => return fail(&e),
            },
            Ok(value) => match std::str::from_utf8(&value) {
                Ok(text) => environment.push((variable, Zeroizing::new(text.to_string()))),
                Err(_) => {
                    return fail(&format!(
                        "{name}: not text, so it cannot be a variable; --file {variable}={name} \
                         hands it over as a file"
                    ));
                }
            },
            Err(e) => return fail(&e),
        }
    }

    let error = exec(command, &environment);
    drop(files);
    fail(&format!("{}: {error}", command[0]));
    127
}

/// Every variable the child gets and where its value comes from, most explicit last to win.
fn plan(store: &Store, asked: &asked::Asked) -> Result<Vec<(String, Wanted)>, String> {
    let mut plan: Vec<(String, Wanted)> = Vec::new();
    let mut put =
        |variable: String, wanted: Wanted| match plan.iter_mut().find(|(v, _)| *v == variable) {
            Some(slot) => slot.1 = wanted,
            None => plan.push((variable, wanted)),
        };

    if asked.all {
        if let Some(why) = store.unreachable_here() {
            return Err(why);
        }
        let mut made: Vec<(String, String)> = Vec::new();
        for name in store.names() {
            let variable = asked::variable_for(asked.prefix.as_deref(), &name)?;
            if let Some((_, earlier)) = made.iter().find(|(v, _)| *v == variable) {
                return Err(format!(
                    "{earlier} and {name} both become {variable} under --all"
                ));
            }
            made.push((variable.clone(), name.clone()));
            let wanted = Wanted::Secret {
                store: None,
                name,
                file: false,
            };
            put(variable, wanted);
        }
    }

    if let Some(path) = &asked.env_file {
        for line in envfile::read(path)? {
            let wanted = match line.value {
                envfile::Value::Plain(value) => Wanted::Plain(value),
                envfile::Value::Secret { store, name } => Wanted::Secret {
                    store,
                    name,
                    file: false,
                },
            };
            put(line.name, wanted);
        }
    }

    let mut named: Vec<&str> = Vec::new();
    for pair in &asked.pairs {
        if named.contains(&pair.variable.as_str()) {
            return Err(format!("{} is named twice", pair.variable));
        }
        named.push(&pair.variable);
        let wanted = Wanted::Secret {
            store: None,
            name: pair.name.clone(),
            file: pair.file,
        };
        put(pair.variable.clone(), wanted);
    }
    Ok(plan)
}

/// Replace this process with the command, so nothing is left holding the value.
fn exec(command: &[String], environment: &[(String, Zeroizing<String>)]) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(&command[0])
        .args(&command[1..])
        .envs(environment.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .exec()
}
