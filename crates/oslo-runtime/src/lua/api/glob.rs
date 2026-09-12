//! `oslo.glob`, `oslo.fs.glob`, `oslo.fs.match` and `oslo.shopt`: the glob engine, from Lua.
//!
//! **One implementation.** `oslo.glob` and `oslo.fs.glob` were two copies of a wrapper that read
//! "the result equals the pattern" as no match — so a file named exactly `c.txt` could never be
//! found — and whose `**` depended on whatever `shopt` the shell happened to have run. Both now
//! take the options the prompt's `pattern(…)` and the `glob` builtin take: the qualifiers of
//! `oslo_base::glob::qualify`.
//!
//! ```lua
//! oslo.glob("**/*.log", { older = "7d", larger = "1M", newest = 5 })
//! oslo.glob("src/**/*.rs", { rows = true })     -- { path, name, ext, kind, size, modified, depth }
//! oslo.fs.match("abc.txt", "a*.txt")            -- true, and nothing on disk is read
//! oslo.shopt("nullglob", true)                  -- the builtin's `shopt -s nullglob`
//! ```

use super::util::{failed, list, ok, record, text};
use oslo_base::glob::qualify::{self, Entry};
use oslo_base::glob::{ShellPattern, walk};
use oslo_base::value::{LuaError, LuaResult, Value};

/// `oslo.glob(pattern [, opts])`: the matching paths, or one table per path with `rows = true`.
///
/// `**` crosses directories unless `globstar = false`: a Lua caller asked for a pattern, and the
/// shell's `shopt` state is not part of the question.
pub(super) fn glob(args: &[Value], function: &str) -> LuaResult<Vec<Value>> {
    let pattern = text(args, 1, function)?;
    let mut items: Vec<Vec<String>> = Vec::new();
    let (mut rows, mut globstar) = (false, true);
    if let Some(Value::Table(opts)) = args.get(1) {
        for (key, value) in opts.borrow().pairs() {
            let Value::Str(key) = key else { continue };
            let key: &str = &key;
            match key {
                "rows" => rows = value.truthy(),
                "globstar" => globstar = value.truthy(),
                _ => {
                    let mut item: Vec<String> = match key {
                        "path_re" => vec!["path".into(), "re".into()],
                        other => vec![other.to_string()],
                    };
                    match value {
                        Value::Bool(true) => {}
                        Value::Bool(false) | Value::Nil => continue,
                        Value::Str(s) => item.push(s.to_string()),
                        Value::Number(n) => item.push(n.to_string()),
                        other => {
                            return Err(LuaError::new(format!(
                                "{function}: `{key}` wants a string, a number or true, got {}",
                                other.type_name()
                            )));
                        }
                    }
                    items.push(item);
                }
            }
        }
    }
    let q = qualify::from_items(&items).map_err(|e| LuaError::new(format!("{function}: {e}")))?;
    let mut options = walk::shell_options();
    options.globstar = globstar;
    options.dotglob |= q.hidden;
    options.nocase |= q.nocase;
    let chars: Vec<(char, bool)> = pattern.chars().map(|c| (c, true)).collect();
    let matched = match walk::expand(&chars, &options) {
        Some(matched) => matched,
        // Nothing in it globs, so it names itself — when it exists.
        None if std::fs::symlink_metadata(oslo_base::lossless::to_os(&pattern)).is_ok() => {
            vec![pattern.clone()]
        }
        None => Vec::new(),
    };
    let base = qualify::base_of(&pattern);
    let paths = qualify::apply(matched, &q, base, options.collate);
    if !rows {
        return ok(list(paths.into_iter().map(Value::str)));
    }
    ok(list(paths.into_iter().map(|path| {
        let e = Entry::of(path, base);
        record(vec![
            ("path", Value::str(e.path.as_str())),
            ("name", Value::str(e.name())),
            ("ext", Value::str(e.ext())),
            ("kind", Value::str(e.kind())),
            ("size", Value::int(e.size as i64)),
            ("modified", Value::int((e.mtime / 1_000_000_000) as i64)),
            ("depth", Value::int(e.depth as i64)),
        ])
    })))
}

/// `oslo.fs.match(name, pattern [, nocase])`: whether `name` matches, without the filesystem.
pub(super) fn matches(args: &[Value]) -> LuaResult<Vec<Value>> {
    let name = text(args, 1, "oslo.fs.match")?;
    let pattern = text(args, 2, "oslo.fs.match")?;
    let nocase = args.get(2).is_some_and(Value::truthy);
    ok(Value::Bool(
        ShellPattern::from_unquoted(&pattern).matches_case(&name, nocase),
    ))
}

/// `oslo.shopt(name [, on])`: an option's state, or set it exactly as `shopt -s`/`-u` would.
pub(super) fn shopt(args: &[Value]) -> LuaResult<Vec<Value>> {
    let name = text(args, 1, "oslo.shopt")?;
    match args.get(1) {
        None | Some(Value::Nil) => match oslo_shell::env::builtins::option_state(&name) {
            Some(on) => ok(Value::Bool(on)),
            None => failed("oslo.shopt", format!("{name}: invalid shell option name")),
        },
        Some(value) => match oslo_shell::env::builtins::set_option(&name, value.truthy()) {
            Ok(()) => ok(Value::Bool(true)),
            Err(problem) => failed("oslo.shopt", problem),
        },
    }
}
