//! The aliases, written as a file another shell can source.
//!
//! ```sh
//! # in ~/.bashrc or ~/.zshrc
//! [ -f ~/.local/share/oslo/macros/macros.sh ] && . ~/.local/share/oslo/macros/macros.sh
//! ```
//!
//! # The half that a `$PATH` directory cannot carry
//!
//! [`super::scripts`] answers "how does bash *run* a stored script" — a file on `$PATH`. It cannot
//! answer the same question for an alias, because an alias is not a program: it has to be *defined
//! inside* the shell that will use it, which means a file that shell sources. That is exactly what
//! `~/.config/profile/aliases.sh` was before any of this, so this is the same file with the database
//! as its source instead of an editor.
//!
//! Derived, like everything else here: rewritten by every mutation, never edited, safe to delete.
//!
//! # What is in it, and what cannot be
//!
//! Aliases, and functions whose bodies are shell. **Not abbreviations** — an abbreviation is expanded
//! into the line as you type it, by the line editor, and there is nothing for bash to be told. **Not
//! a function written in Lua**, for the same reason in a different direction: bash has no Lua.
//!
//! And not scripts, which are files in [`super::scripts`] and would be a second copy here.

use super::{Entry, Kind};

/// The file another shell sources. Beside the database rather than in the `$PATH` directory,
/// because it is not a program and nothing should ever try to execute it.
pub fn path() -> Option<std::path::PathBuf> {
    Some(super::directory()?.join("macros.sh"))
}

/// Write it, from what the database now holds.
pub fn publish(entries: &[Entry]) -> Result<(), String> {
    let Some(path) = path() else {
        return Err("nowhere to write the aliases".to_string());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, text(entries)).map_err(|e| format!("{}: {e}", path.display()))
}

/// What the file says.
pub fn text(entries: &[Entry]) -> String {
    let mut out = String::from(
        "# Written by `oslo macros`. Every change rewrites this file; nothing here is edited by\n\
         # hand and deleting it costs nothing — `oslo macros publish` writes it again.\n\
         #\n\
         # Source it from ~/.bashrc or ~/.zshrc to have these aliases in a shell that is not oslo.\n\
         # oslo does not read it: it reads the database.\n\
         #\n\
         # A stored variable is here only when its body is a plain value. One whose body runs a\n\
         # command — `$(oslo secret get …)` — is not: oslo runs that the first time the name is\n\
         # read, and a file that is sourced has no such moment, so writing it here would run every\n\
         # one of them at every shell start.\n\n",
    );
    for entry in entries.iter().filter(|entry| entry.active) {
        match entry.kind {
            Kind::Alias => {
                out.push_str(&format!("alias {}={}\n", entry.name, quoted(&entry.body)));
            }
            // A function is shell code that runs *in* the calling shell, which is what a function
            // definition gives bash too — so the one kind that cannot be a file on `$PATH` is the
            // one that travels perfectly in a sourced file.
            Kind::Func if entry.kind.extension(&entry.body) == "sh" => {
                out.push_str(&format!(
                    "{}() {{\n{}\n}}\n",
                    entry.name,
                    entry.body.trim_end()
                ));
            }
            // **A variable travels only when its body is a value.** In oslo the body is run the
            // first time the name is read; a sourced file has no such moment, so a
            // `$(oslo secret get …)` written here would decrypt at every bash start — for every
            // secret, whether or not anything wanted one. Those are left out and said so in the
            // header. What is left is a plain value, which means the same thing in both shells.
            // **Quoted**, now that a value may contain a space. `export EDITOR=code --wait` is not
            // an assignment with an argument, it is `export` given two words — bash takes the first
            // and treats the second as another name to export.
            Kind::Var if super::is_a_value(&entry.body) => {
                out.push_str(&format!("export {}={}\n", entry.name, quoted(&entry.body)));
            }
            _ => {}
        }
    }
    out
}

/// One body, quoted so the shell reading it back gets exactly these bytes.
///
/// Single quotes, because an alias body is full of `$` and backticks that must not be expanded when
/// the file is *sourced* — they are expanded when the alias is *used*. The one character single
/// quotes cannot hold is a single quote, spelled the way every shell spells it.
fn quoted(body: &str) -> String {
    format!("'{}'", body.trim_end().replace('\'', "'\\''"))
}

#[cfg(test)]
#[path = "sourced/tests.rs"]
mod tests;
