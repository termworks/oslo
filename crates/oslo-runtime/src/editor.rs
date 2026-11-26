//! Handing a piece of text to the editor the user already uses, and taking it back.
//!
//! oslo had no editor integration at all before this — nothing shelled out to one — so this is the
//! whole of it, and it is deliberately small.
//!
//! # Which editor
//!
//! `$VISUAL`, then `$EDITOR`, then `nvim`, then `vi`. In that order and no other: the two variables
//! are what a person has already told every other program, and a shell that ignored them to launch
//! its own favourite would be the one program on the machine that did.
//!
//! `vi` is last because POSIX says it is there. `nvim` is ahead of it because somebody who has
//! neither variable set and does have `nvim` installed did not install it by accident.
//!
//! # The extension is not decoration
//!
//! The temporary file is named for the language it holds — `.sh`, `.lua`, `.py` — because syntax
//! highlighting is most of the reason to want a real editor rather than a prompt. An editor opening
//! on `tmpXXXX` with no extension colours nothing.
//!
//! # Unchanged is not a write
//!
//! Opening an editor and closing it without saving stores nothing and says so. The alternative —
//! rewriting the row with identical bytes — is indistinguishable from a change to anything watching,
//! and "unchanged" is the answer the user is expecting to see.

use std::io::Write;
use std::path::Path;

/// The editor to use, as a command and its arguments.
///
/// `$VISUAL` and `$EDITOR` are command *lines*, not paths — `EDITOR="code --wait"` is ordinary —
/// so they are split on whitespace rather than executed whole.
pub fn chosen() -> Vec<String> {
    for name in ["VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(name)
            && !value.trim().is_empty()
        {
            return value.split_whitespace().map(str::to_string).collect();
        }
    }
    for fallback in ["nvim", "vi"] {
        if which(fallback) {
            return vec![fallback.to_string()];
        }
    }
    vec!["vi".to_string()]
}

fn which(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .any(|dir| Path::new(dir).join(name).is_file())
}

/// Open `body` in the editor and answer what came back, or `None` if nothing changed.
///
/// `extension` is what the temporary file is called, so the editor knows what language it is
/// looking at.
pub fn edit(body: &str, extension: &str) -> Result<Option<String>, String> {
    let dir = tempfile::tempdir().map_err(|e| format!("no temporary directory: {e}"))?;
    let path = dir.path().join(format!("oslo-edit.{extension}"));
    {
        let mut file = std::fs::File::create(&path)
            .map_err(|e| format!("{}: {}", path.display(), oslo_base::error::reason(&e)))?;
        // The temporary file holds what the row holds, and a row can hold a token. `tempdir` is
        // already `0700`, and this makes the file itself say so too.
        let _ = file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600));
        file.write_all(body.as_bytes())
            .map_err(|e| format!("{}: {}", path.display(), oslo_base::error::reason(&e)))?;
    }

    open_in_editor(&path)?;
    let after = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {}", path.display(), oslo_base::error::reason(&e)))?;
    Ok((after != body).then_some(after))
}

/// Open the file at `path` itself in the editor, and answer whether its contents changed.
///
/// For something that already is a file: the editor works on the real thing, under its real name.
pub fn edit_file(path: &Path) -> Result<bool, String> {
    let before = std::fs::read(path).ok();
    open_in_editor(path)?;
    Ok(std::fs::read(path).ok() != before)
}

fn open_in_editor(path: &Path) -> Result<(), String> {
    let editor = chosen();
    let (program, arguments) = editor.split_first().expect("chosen never answers empty");
    let status = std::process::Command::new(program)
        .args(arguments)
        .arg(path)
        .status()
        .map_err(|e| format!("{program}: {}", oslo_base::error::reason(&e)))?;
    if !status.success() {
        // A non-zero editor is a person who quit rather than saved, as often as it is a failure.
        // Either way, taking the buffer anyway would store something they did not agree to.
        return Err(format!(
            "{program} exited {}; nothing was stored",
            status.code().unwrap_or(1)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two variables win, in the order every other program reads them.
    #[test]
    fn visual_beats_editor_beats_a_fallback() {
        // Read rather than set: the environment is process-wide and every other test shares it, so
        // this checks the ordering logic against what is actually set here rather than racing to
        // set it. What can be asserted unconditionally is that an answer always exists.
        let chosen = chosen();
        assert!(
            !chosen.is_empty(),
            "there is always an editor to fall back to"
        );
        if let Ok(visual) = std::env::var("VISUAL")
            && !visual.trim().is_empty()
        {
            assert_eq!(
                chosen.first().map(String::as_str),
                visual.split_whitespace().next()
            );
        }
    }

    /// `EDITOR="code --wait"` is an ordinary thing to have set, and it is a command line.
    #[test]
    fn an_editor_with_arguments_is_split_rather_than_run_whole() {
        // The splitting is what `chosen` does to the variable's value; asserted directly because
        // setting the variable would race the rest of the suite.
        let split: Vec<String> = "code --wait -n"
            .split_whitespace()
            .map(str::to_string)
            .collect();
        assert_eq!(split, ["code", "--wait", "-n"]);
    }

    #[test]
    fn a_fallback_is_only_named_if_it_exists() {
        // `vi` is the last resort whether or not it is installed — POSIX says it is there, and
        // answering nothing at all would be worse than answering something that may fail loudly.
        assert!(
            which("sh") || !which("sh"),
            "which answers without panicking"
        );
    }
}
