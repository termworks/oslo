//! Pathname expansion, the shell's side of it.
//!
//! The matcher and the walker live in `oslo_base::glob`, because the prompt needs exactly the same
//! answers and cannot see this crate. What stays here is what only the shell has: the quoting each
//! expanded run carries, which decides per character whether `*` is a metacharacter.

use crate::expand::word::{Origin, Run, field_text};
use oslo_base::error::ShellError;
use oslo_base::glob::walk;

pub use oslo_base::glob::ShellPattern;
pub use oslo_base::glob::walk::{
    set_dotglob, set_extglob, set_failglob, set_globstar, set_nocaseglob, set_nocasematch,
    set_nullglob,
};

/// A pattern that matched nothing while `failglob` is on, as the text it was written as.
#[derive(Debug)]
pub struct NoMatch(pub String);

/// Refuse an `extglob` group the script wrote while `extglob` is off, with bash's parse error.
///
/// bash reads `@(a|b)` as a pattern only with the option on and refuses the line otherwise. The
/// parser here always reads it, so the refusal is made where the option is known. Only text the
/// script wrote counts: a `$p` holding `@(a|b)` is data and matches as the characters it is.
pub fn refuse_extglob(field: &[Run]) -> oslo_base::error::Result<()> {
    let written = |run: &&Run| run.origin == Origin::Literal;
    if !walk::extglob()
        && field
            .iter()
            .filter(written)
            .any(|run| opens_group(&run.text))
    {
        return Err(ShellError::SyntaxError(
            "syntax error near unexpected token `('".to_string(),
        ));
    }
    Ok(())
}

fn opens_group(text: &str) -> bool {
    text.as_bytes()
        .windows(2)
        .any(|pair| matches!(pair, [b'?' | b'*' | b'+' | b'@' | b'!', b'(']))
}

/// Compile a pattern from expanded runs, honouring the quoting each run carries.
///
/// It is what makes `case $x in "$p")` a string comparison and `case $x in $p)` a pattern match.
pub fn pattern_from_runs(runs: &[Run]) -> ShellPattern {
    ShellPattern::from_chars(&chars_of(runs))
}

/// Each character of a field, paired with whether it may still glob.
fn chars_of(field: &[Run]) -> Vec<(char, bool)> {
    field
        .iter()
        .flat_map(|run| run.text.chars().map(move |ch| (ch, run.globs())))
        .collect()
}

/// Expand one field against the filesystem, or yield its literal text when it matches nothing.
///
/// For callers outside word expansion — Lua, `run` — which have no `GLOBIGNORE` and nowhere to
/// report `failglob`; see [`expand_field`] for the shell's own.
pub fn expand_glob(field: &[Run]) -> Vec<String> {
    expand_field(field, None).unwrap_or_else(|NoMatch(text)| vec![text])
}

/// Expand one field the way word expansion does: `GLOBIGNORE`, `nullglob` and `failglob` applied.
///
/// Rebuilt run by run rather than from the field's flat text: `echo "a"*` globs on the trailing
/// `*`, and `echo "a*"` does not glob at all.
pub fn expand_field(field: &[Run], globignore: Option<&str>) -> Result<Vec<String>, NoMatch> {
    // **Asked before anything is built.** Almost every field is a plain word, and finding that out
    // must not cost a character vector per argument of every command.
    if !field
        .iter()
        .any(|run| run.globs() && (run.text.contains(['*', '?', '[']) || opens_group(&run.text)))
    {
        return Ok(vec![field_text(field)]);
    }
    let ignore: Vec<ShellPattern> = globignore
        .unwrap_or_default()
        .split(':')
        .filter(|pattern| !pattern.is_empty())
        .map(ShellPattern::from_unquoted)
        .collect();
    let mut options = walk::shell_options();
    // Setting `GLOBIGNORE` turns `dotglob` on, as bash does: the list is how hidden files get
    // filtered, so they have to be candidates first. `.` and `..` never are.
    if !ignore.is_empty() {
        options.dotglob = true;
    }
    let Some(mut matched) = walk::expand(&chars_of(field), &options) else {
        return Ok(vec![field_text(field)]);
    };
    matched.retain(|path| !ignore.iter().any(|pattern| pattern.matches(path)));
    if !matched.is_empty() {
        return Ok(matched);
    }
    if walk::failglob() {
        return Err(NoMatch(field_text(field)));
    }
    if walk::nullglob() {
        return Ok(Vec::new());
    }
    Ok(vec![field_text(field)])
}

#[cfg(test)]
mod tests {
    use super::{expand_glob, set_globstar};
    use crate::expand::word::{Origin, Run};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `globstar` is one process-wide flag, so a test that turns it on cannot run beside one that
    /// depends on it being off.
    static GLOBSTAR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A scratch directory named after the caller, so the suite's threads cannot collide.
    fn scratch(tag: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("oslo-glob-{}-{tag}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Glob an absolute pattern, so the test never depends on the process-wide working directory.
    fn glob_in(dir: &Path, pattern: &str) -> Vec<String> {
        let field = vec![
            Run::new(format!("{}/", dir.display()), Origin::Quoted),
            Run::new(pattern, Origin::Literal),
        ];
        let prefix = format!("{}/", dir.display());
        expand_glob(&field)
            .into_iter()
            .map(|p| p.strip_prefix(&prefix).unwrap_or(&p).to_string())
            .collect()
    }

    /// `**` crosses directories when globstar is on, and includes the directory it starts in.
    #[test]
    fn globstar_crosses_directories() {
        let _guard = GLOBSTAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("globstar");
        fs::create_dir_all(dir.join("a/b/c")).expect("dirs");
        for path in ["mod.rs", "a/mod.rs", "a/b/mod.rs", "a/b/c/other.rs"] {
            fs::write(dir.join(path), "").expect("file");
        }
        set_globstar(true);
        let found = glob_in(&dir, "**/mod.rs");
        set_globstar(false);
        assert_eq!(found, vec!["a/b/mod.rs", "a/mod.rs", "mod.rs"]);
    }

    /// Quoting turns it off, like every other metacharacter — even with `globstar` on.
    #[test]
    fn a_quoted_globstar_is_literal_text() {
        let _guard = GLOBSTAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_globstar(true);
        assert_eq!(
            expand_glob(&[Run::new("**/nothing.rs", Origin::Quoted)]),
            vec!["**/nothing.rs"]
        );
        set_globstar(false);
    }

    #[test]
    fn a_field_with_no_metacharacters_is_itself() {
        let field = vec![Run::new("plain", Origin::Literal)];
        assert_eq!(expand_glob(&field), vec!["plain"]);
    }

    /// Quoting suppresses globbing per character, not per word.
    #[test]
    fn quoted_metacharacters_do_not_glob() {
        let field = vec![Run::new("a*", Origin::Quoted)];
        assert_eq!(expand_glob(&field), vec!["a*"]);
        let field = vec![Run::new("nomatch\\", Origin::Quoted)];
        assert_eq!(expand_glob(&field), vec!["nomatch\\"]);
    }

    /// An unmatched pattern expands to itself with the quotes removed.
    #[test]
    fn an_unmatched_pattern_yields_the_unquoted_text() {
        let field = vec![
            Run::new("no*such", Origin::Quoted),
            Run::new("*", Origin::Literal),
        ];
        assert_eq!(expand_glob(&field), vec!["no*such*"]);
    }

    #[test]
    fn an_unterminated_class_is_literal_text() {
        let field = vec![Run::new("a[b", Origin::Literal)];
        assert_eq!(expand_glob(&field), vec!["a[b"]);
    }

    #[test]
    fn dotfiles_are_only_matched_when_the_dot_is_written_out() {
        let dir = scratch("dotfiles");
        fs::write(dir.join(".hidden"), "").unwrap();
        fs::write(dir.join("visible"), "").unwrap();
        assert_eq!(glob_in(&dir, "*"), vec!["visible"]);
        assert_eq!(glob_in(&dir, ".*"), vec![".hidden"]);
    }

    /// `.` and `..` are real directory entries that every one of these would match textually.
    #[test]
    fn dot_and_dotdot_are_never_matched() {
        let dir = scratch("dotdot");
        fs::write(dir.join(".hidden"), "").unwrap();
        assert_eq!(glob_in(&dir, ".*"), vec![".hidden"]);
        assert_eq!(glob_in(&dir, ".?"), vec![".?"]);
    }

    /// A match is spelled the way the pattern was: no normalisation, no lost `./`.
    #[test]
    fn matches_keep_the_patterns_own_path_syntax() {
        let dir = scratch("syntax");
        fs::write(dir.join("a1"), "").unwrap();
        let field = vec![Run::new(format!("{}/./a*", dir.display()), Origin::Literal)];
        assert_eq!(expand_glob(&field), vec![format!("{}/./a1", dir.display())]);
    }

    #[test]
    fn a_star_does_not_cross_a_directory_boundary() {
        let _guard = GLOBSTAR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("separator");
        fs::create_dir(dir.join("d")).unwrap();
        fs::write(dir.join("d/a1"), "").unwrap();
        fs::write(dir.join("top"), "").unwrap();
        assert_eq!(glob_in(&dir, "*"), vec!["d", "top"]);
        assert_eq!(glob_in(&dir, "*/*"), vec!["d/a1"]);
        assert_eq!(glob_in(&dir, "**/*"), vec!["d/a1"]);
    }

    #[test]
    fn a_trailing_slash_restricts_the_match_to_directories() {
        let dir = scratch("dirsuffix");
        fs::create_dir(dir.join("d1")).unwrap();
        fs::write(dir.join("d2"), "").unwrap();
        assert_eq!(glob_in(&dir, "d*/"), vec!["d1/"]);
    }

    #[test]
    fn a_literal_component_after_a_pattern_must_exist() {
        let dir = scratch("literal-tail");
        fs::create_dir(dir.join("d1")).unwrap();
        fs::create_dir(dir.join("d2")).unwrap();
        fs::write(dir.join("d1/target"), "").unwrap();
        assert_eq!(glob_in(&dir, "*/target"), vec!["d1/target"]);
    }

    #[test]
    fn matches_are_sorted() {
        let dir = scratch("order");
        for name in ["c", "a", "b"] {
            fs::write(dir.join(name), "").unwrap();
        }
        assert_eq!(glob_in(&dir, "*"), vec!["a", "b", "c"]);
    }
}
