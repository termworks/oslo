//! `oslo macros` — the small named things you keep, in one place.
//!
//! The rules live in [`oslo_base::macros`]; this parses words, opens an editor and prints. Every
//! mutation republishes the snapshot a starting shell reads, so the database and the file cannot
//! drift apart by anything short of editing the file by hand.

pub mod help;
mod list;

use crate::cli::help::Paint;
use oslo::macros::{self, Entry, Kind};

pub fn run(args: &[String]) -> i32 {
    if let Some(page) = help::MENU.asked(args, Paint::detect()) {
        print!("{page}");
        return 0;
    }
    let Some(command) = args.first().map(String::as_str) else {
        print!("{}", help::text(Paint::detect()));
        return 0;
    };
    let rest = &args[1..];
    // **A flag that silently does nothing is worse than one that does not exist.** `--replace` is
    // parsed by the shared argument reader, so every subcommand accepted it — and only `import`
    // ever read it, where it means "forget everything stored first". `oslo macros add --replace`
    // therefore looked like it asked for something and asked for nothing. Refused here rather than
    // in `add`, because `remove` and `edit` took it just as meaninglessly.
    if command != "import" && rest.iter().any(|word| word == "--replace") {
        return usage(&format!(
            "--replace is only for `import`, where it means forget everything stored first; \
             `{command}` has no use for it"
        ));
    }
    match command {
        "add" => add(rest),
        "remove" | "rm" => remove(rest),
        "edit" => edit(rest),
        "run" => run_macro(rest),
        // Takes nothing — the `_args` on `publish` says so — so a word after it was read as
        // nothing at all rather than as the mistake it is.
        "publish" => help::MENU
            .extra("publish", args, 0)
            .unwrap_or_else(|| publish(rest)),
        "show" | "list" => list::show(rest),
        "off" => switch(rest, false),
        "on" => switch(rest, true),
        "export" => list::export(rest),
        "import" => list::import(rest),
        other => help::MENU.unknown(other),
    }
}

/// Something was wrong with the words, said the way every tool says it.
fn usage(message: &str) -> i32 {
    help::MENU.missing(message)
}

fn fail(message: &str) -> i32 {
    eprintln!("oslo macros: {message}");
    1
}

/// The kind a flag asked for, and the words that were not flags.
#[derive(Debug)]
struct Asked {
    /// **`None` until a flag says.** Four kinds and a silent default is a trap: `add gs 'git status'`
    /// would quietly make an alias when the fingers meant an abbreviation.
    kind: Option<Kind>,
    edit: bool,
    plain: bool,
    replace: bool,
    session: bool,
    tags: Vec<String>,
    words: Vec<String>,
}

impl Asked {
    /// The kind, or the error that says the four names.
    fn kind(&self) -> Result<Kind, String> {
        self.kind
            .ok_or_else(|| "say which: --alias, --abbrev, --func, --script or --var".to_string())
    }
}

fn parse(args: &[String]) -> Result<Asked, String> {
    let mut asked = Asked {
        kind: None,
        edit: false,
        plain: false,
        replace: false,
        session: false,
        tags: Vec::new(),
        words: Vec::new(),
    };
    let mut waiting_for_tag = false;
    for arg in args {
        if waiting_for_tag {
            waiting_for_tag = false;
            if !macros::valid_tag(arg) {
                return Err(format!("{arg:?} is not a tag: one word, no spaces"));
            }
            asked.tags.push(arg.to_string());
            continue;
        }
        let kind = match arg.as_str() {
            "--alias" => Some(Kind::Alias),
            "--abbrev" | "--abbr" => Some(Kind::Abbrev),
            "--func" | "--function" => Some(Kind::Func),
            "--script" => Some(Kind::Script),
            "--var" | "--variable" | "--env" => Some(Kind::Var),
            _ => None,
        };
        if let Some(kind) = kind {
            if asked.kind.is_some_and(|had| had != kind) {
                return Err(
                    "one kind at a time: --alias, --abbrev, --func, --script or --var".to_string(),
                );
            }
            asked.kind = Some(kind);
            continue;
        }
        match arg.as_str() {
            "--edit" | "-e" => asked.edit = true,
            "--plain" => asked.plain = true,
            "--replace" => asked.replace = true,
            "--session" | "-s" => asked.session = true,
            "--tag" | "-t" => waiting_for_tag = true,
            // Only a *leading* dash is an option. A body is arbitrary text and often starts with
            // one — `oslo macros add --alias ll '-la'` is a thing somebody will write.
            other if other.starts_with("--") && asked.words.is_empty() => {
                return Err(format!("unknown option {other:?}"));
            }
            other => asked.words.push(other.to_string()),
        }
    }
    if waiting_for_tag {
        return Err("--tag needs a tag".to_string());
    }
    Ok(asked)
}

fn add(args: &[String]) -> i32 {
    let asked = match parse(args) {
        Ok(asked) => asked,
        Err(problem) => return usage(&problem),
    };
    let kind = match asked.kind() {
        Ok(kind) => kind,
        Err(problem) => return usage(&problem),
    };
    let Some(name) = asked.words.first().cloned() else {
        return usage("add needs a name");
    };
    // **A variable may be written the way it would be typed.** `--var GITHUB_TOKEN=$(oslo secret
    // get gh-token)` and `--var GITHUB_TOKEN '$(oslo secret get gh-token)'` store the same thing;
    // the first is what somebody's fingers already know, and refusing it would be pedantry.
    let (name, first_word_body) = match kind {
        Kind::Var => match name.split_once('=') {
            Some((name, body)) => (name.to_string(), Some(body.to_string())),
            None => (name, None),
        },
        _ => (name, None),
    };
    if !macros::valid_name(&name) {
        return fail(&format!(
            "{name:?} is not a name: it becomes a command word, so no spaces and nothing that \
             could be an operator"
        ));
    }

    // **A function and a script are always written in the editor**, and an inline body for one is
    // refused rather than accepted: taking it would store a one-line function because that is what
    // fitted on the command line, which is how you end up with a function written as one line.
    let inline = match &first_word_body {
        Some(body) if asked.words.len() == 1 => body.clone(),
        Some(body) => format!("{body} {}", asked.words[1..].join(" ")),
        None => asked.words[1..].join(" "),
    };
    let editor_only = matches!(kind, Kind::Func | Kind::Script);
    // **Decided before the database is opened**, because it is a decision about the words. Opening
    // first made the answer depend on the store: a database that could not be opened turned a usage
    // error into a failure to open, which is a different exit status for the same command line.
    if editor_only && !inline.is_empty() {
        return usage(&format!(
            "a {} is written in the editor: `oslo macros add --{} {name}` and no body",
            kind.word(),
            kind.word()
        ));
    }

    let store = match macros::open() {
        Ok(store) => store,
        Err(problem) => return fail(&problem),
    };

    // **`add` is where an ambiguous name is created, and it was the one command that allowed it.**
    // `remove` and `edit` both refuse a name owned by more than one kind and ask which was meant —
    // `macros.rs` calls the state "a mistake rather than a feature" — but nothing stopped it being
    // made. So the two commands that refuse were refusing a state only this one could produce.
    //
    // Before the editor opens, so a refusal does not cost a writing session. Storing over the *same*
    // kind is an ordinary update and is not this.
    if let Some(other) = macros::kinds_of(&store, &name)
        .into_iter()
        .find(|owner| *owner != kind)
    {
        return fail(&format!(
            "{name} is already a {}: remove that first, or choose another name",
            other.word()
        ));
    }

    // **Before the editor opens, the same as `edit`.** A function is reached only once `$PATH` has
    // failed, and a script only where nothing earlier on `$PATH` has its name, so one that is
    // already answered is being written for nothing — and hearing that after a writing session is
    // hearing it too late.
    warn_if_shadowed(kind, &name);
    if kind == Kind::Script {
        return add_script(&name, &asked.tags);
    }

    let body = if editor_only || asked.edit || inline.is_empty() {
        let starting = if inline.is_empty() {
            macros::get(&store, kind, &name)
                .map(|e| e.body)
                .unwrap_or_else(|| starter(kind, &name))
        } else {
            inline
        };
        match oslo_runtime::editor::edit(&starting, kind.extension(&starting)) {
            Ok(Some(body)) => body,
            Ok(None) => {
                println!("unchanged");
                return 0;
            }
            Err(problem) => return fail(&problem),
        }
    } else {
        inline
    };

    if body.trim().is_empty() {
        return fail("nothing to store: the body is empty");
    }
    // The tags asked for, or — when none were — whatever it already had, so editing a macro does
    // not silently strip its labels.
    let tags = if asked.tags.is_empty() {
        macros::get(&store, kind, &name)
            .map(|old| old.tags)
            .unwrap_or_default()
    } else {
        asked.tags.clone()
    };
    let entry = Entry::new(kind, &name, &body).tagged(&tags);
    if let Err(problem) = macros::put_and_publish(&store, &entry) {
        return fail(&problem);
    }
    println!("{} {name}", kind.word());
    0
}

/// What an empty function or script starts life as, so the editor opens on something rather than
/// on a blank buffer nobody knows the shape of.
fn starter(kind: Kind, name: &str) -> String {
    match kind {
        Kind::Func => {
            format!("# {name} — a shell function. The body runs with \"$@\" as its arguments.\n\n")
        }
        Kind::Script => format!(
            "#!/usr/bin/env bash\n# tags:\n# {name} — a script. Change the shebang for another \
             language.\n\n"
        ),
        _ => String::new(),
    }
}

/// `add --script NAME`: the file itself, opened in the editor where it lives.
///
/// A new script starts as the starter and is removed again when the editor saves nothing. `--tag`
/// rewrites the `# tags:` line before the editor opens.
fn add_script(name: &str, tags: &[String]) -> i32 {
    let existing = macros::scripts::get(name);
    if existing.is_none() || !tags.is_empty() {
        let entry = match &existing {
            Some(entry) => entry.clone().tagged(tags),
            None => Entry::new(Kind::Script, name, &starter(Kind::Script, name)).tagged(tags),
        };
        if let Err(problem) = macros::scripts::write(&entry) {
            return fail(&problem);
        }
    }
    let Some(path) = macros::scripts::path_of(name) else {
        return fail(&format!("{name}: the script could not be written"));
    };
    match oslo_runtime::editor::edit_file(&path) {
        Ok(false) if existing.is_none() => {
            macros::scripts::remove(name);
            println!("unchanged");
            0
        }
        Ok(false) if tags.is_empty() => {
            println!("unchanged");
            0
        }
        Ok(_) => {
            println!("script {name}");
            0
        }
        Err(problem) => {
            if existing.is_none() {
                macros::scripts::remove(name);
            }
            fail(&problem)
        }
    }
}

fn remove(args: &[String]) -> i32 {
    let asked = match parse(args) {
        Ok(asked) => asked,
        Err(problem) => return usage(&problem),
    };
    let Some(name) = asked.words.first() else {
        return usage("remove needs a name");
    };
    let store = match macros::open() {
        Ok(store) => store,
        Err(problem) => return fail(&problem),
    };
    // **The kind is only asked for when the name is ambiguous.** `remove gs` is unambiguous
    // whenever one kind has that name, and making somebody name the kind to delete the only thing
    // called `gs` is a question with one possible answer.
    let kind = match asked.kind.or_else(|| {
        let only = macros::kinds_of(&store, name);
        (only.len() == 1).then(|| only[0])
    }) {
        Some(kind) => kind,
        None => match macros::kinds_of(&store, name).as_slice() {
            [] => return fail(&format!("nothing called {name}")),
            several => {
                let words: Vec<String> =
                    several.iter().map(|k| format!("--{}", k.word())).collect();
                return usage(&format!(
                    "{name} is more than one thing — say which: {}",
                    words.join(", ")
                ));
            }
        },
    };
    match macros::remove_and_publish(&store, kind, name) {
        Ok(true) => {
            println!("removed {} {name}", kind.word());
            0
        }
        Ok(false) => {
            let others = macros::kinds_of(&store, name);
            if others.is_empty() {
                fail(&format!("no {} called {name}", kind.word()))
            } else {
                let words: Vec<&str> = others.iter().map(|k| k.word()).collect();
                fail(&format!(
                    "no {} called {name} — it is a {} ({})",
                    kind.word(),
                    words.join(" and a "),
                    words
                        .iter()
                        .map(|w| format!("--{w}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
        Err(problem) => fail(&problem),
    }
}

/// `publish` — write the derived copies again.
///
/// Every mutation does this already, so the only reason to ask is a file that was deleted, or a
/// database that was filled before the copies existed at all. Scripts are files and need no copy.
fn publish(_args: &[String]) -> i32 {
    let store = match macros::open() {
        Ok(store) => store,
        Err(problem) => return fail(&problem),
    };
    if let Err(problem) = macros::publish(&store) {
        return fail(&problem);
    }
    let aliases = macros::database_entries(&store)
        .iter()
        .filter(|entry| entry.kind == Kind::Alias && entry.active)
        .count();
    if let Some(path) = macros::sourced::path() {
        println!("{aliases} aliases  → {}", path.display());
        println!();
        println!("For a shell that is not oslo, source that file:");
        println!("  [ -f {} ] && . {}", path.display(), path.display());
    }
    0
}

/// `run NAME [ARG]...` — run a stored macro from something that is not oslo.
///
/// **Everything after the name belongs to the macro**, including words that look like options:
/// `oslo macros run deploy --dry-run` passes `--dry-run` to `deploy` and not to oslo. That is the
/// same rule `-c` follows, and for the same reason.
fn run_macro(args: &[String]) -> i32 {
    let Some((name, rest)) = args.split_first() else {
        return usage("run needs a name: `oslo macros run NAME [ARG]...`");
    };
    let mut env = oslo::env::Environment::new();
    match oslo::exec::stored::run_named(&mut env, name, rest) {
        Some(status) => status,
        None => fail(&format!(
            "nothing runnable called {name} — `oslo macros show {name}` says what it is"
        )),
    }
}

/// `edit NAME` — open one in `$EDITOR` and store what comes back.
///
/// `add` already opens the editor for a name it knows, but only by way of adding one; asking to
/// *edit* what you have should not be spelled as adding it again.
fn edit(args: &[String]) -> i32 {
    let asked = match parse(args) {
        Ok(asked) => asked,
        Err(problem) => return usage(&problem),
    };
    let Some(name) = asked.words.first() else {
        return usage("edit needs a name");
    };
    let store = match macros::open() {
        Ok(store) => store,
        Err(problem) => return fail(&problem),
    };
    // The kind is only asked for when the name is more than one thing, exactly as `remove` does it.
    let kind = match asked
        .kind
        .or_else(|| match macros::kinds_of(&store, name).as_slice() {
            [only] => Some(*only),
            _ => None,
        }) {
        Some(kind) => kind,
        None => match macros::kinds_of(&store, name).as_slice() {
            [] => return fail(&format!("nothing called {name}")),
            several => {
                let words: Vec<String> =
                    several.iter().map(|k| format!("--{}", k.word())).collect();
                return usage(&format!(
                    "{name} is more than one thing — say which: {}",
                    words.join(", ")
                ));
            }
        },
    };
    let Some(entry) = macros::get(&store, kind, name) else {
        return fail(&format!("no {} called {name}", kind.word()));
    };

    // **Said before the editor opens, not after it closes.** A file of the same name earlier on
    // `$PATH` is what actually runs, and editing the stored one would be editing something the
    // shell never reaches. Whoever asked for this should know that while they still have the
    // choice of editing the other file instead.
    warn_if_shadowed(kind, name);

    if kind == Kind::Script {
        let Some(path) = macros::scripts::path_of(name) else {
            return fail(&format!("no script called {name}"));
        };
        return match oslo_runtime::editor::edit_file(&path) {
            Ok(true) => {
                println!("script {name}");
                0
            }
            Ok(false) => {
                println!("unchanged");
                0
            }
            Err(problem) => fail(&problem),
        };
    }

    match oslo_runtime::editor::edit(&entry.body, kind.extension(&entry.body)) {
        Ok(None) => {
            println!("unchanged");
            0
        }
        Ok(Some(body)) => match macros::put_and_publish(&store, &Entry { body, ..entry }) {
            Ok(()) => {
                println!("{} {name}", kind.word());
                0
            }
            Err(problem) => fail(&problem),
        },
        Err(problem) => fail(&problem),
    }
}

/// Say so when `$PATH` already answers to this name, so a stored copy that can never run is not
/// stored in silence.
///
/// **Called by `add` as well as `edit`, which it was not.** The message was written for `edit`
/// only, so `oslo macros add --script date` stored something the shell would never reach and said
/// nothing at all — and `oslo macros edit date`, afterwards, explained why. The moment worth
/// warning at is the one where the thing is created.
fn warn_if_shadowed(kind: Kind, name: &str) {
    let Some(path) = shadowing_file(kind, name) else {
        return;
    };
    eprintln!("oslo macros: {name} runs from {path} — this is the stored copy, and `$PATH`");
    eprintln!("oslo macros: wins over it. Edit that file, or take it off `$PATH`, if you meant");
    eprintln!("oslo macros: the one that runs.");
}

/// The file on `$PATH` that would answer to `name` before a stored macro does.
///
/// Only for the kinds that are *found by name* when a command runs. An alias and an abbreviation
/// are applied before the search happens, so nothing on `$PATH` can shadow one. A script is
/// shadowed by any file `$PATH` finds first that is not the script itself.
fn shadowing_file(kind: Kind, name: &str) -> Option<String> {
    if !matches!(kind, Kind::Func | Kind::Script) {
        return None;
    }
    let found = oslo::env::builtins::hash_lookup(name)?;
    let own = macros::scripts::directory().map(|dir| dir.join(name));
    if kind == Kind::Script && own.as_deref() == Some(found.as_path()) {
        return None;
    }
    Some(found.to_string_lossy().into_owned())
}

/// `off` and `on` — the screen's Space and Space ×3, for something that is not a person.
///
/// Both switches have a spelling here because a manager only a person can drive is one you cannot
/// put in a script, and because "off for this session" is the one thing a *hook* might want to say.
fn switch(args: &[String], on: bool) -> i32 {
    let asked = match parse(args) {
        Ok(asked) => asked,
        Err(problem) => return usage(&problem),
    };
    let Some(name) = asked.words.first() else {
        return usage(&format!("{} needs a name", if on { "on" } else { "off" }));
    };
    if asked.session {
        // The session this process is in, which for anything a shell started is the *shell's* —
        // see `$OSLO_SESSION` in `env::scope::seed`.
        let session = oslo::track::session::id();
        return match macros::live::session::set(&session, name, !on) {
            Ok(()) => {
                println!(
                    "{name} is {} in this session",
                    if on { "on" } else { "off" }
                );
                0
            }
            Err(problem) => fail(&problem),
        };
    }

    let store = match macros::open() {
        Ok(store) => store,
        Err(problem) => return fail(&problem),
    };
    let kinds = match asked.kind {
        Some(kind) => vec![kind],
        None => macros::kinds_of(&store, name),
    };
    if kinds.is_empty() {
        return fail(&format!("nothing called {name}"));
    }
    for kind in kinds {
        if macros::set_active(&store, kind, name, on).is_none() {
            return fail(&format!("no {} called {name}", kind.word()));
        }
    }
    if let Err(problem) = macros::publish(&store) {
        return fail(&problem);
    }
    println!("{name} is {}", if on { "on" } else { "off" });
    0
}

#[cfg(test)]
#[path = "macros/tests.rs"]
mod tests;
