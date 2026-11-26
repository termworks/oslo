//! What `argc` asks the world, answered by this shell.
//!
//! `argc::Runtime` is how the parser reaches outside itself: the environment, `$PATH`, the
//! filesystem, and one method that runs a *script's own functions* to compute a default or a list of
//! choices. Upstream's implementation answers the last one by starting bash.
//!
//! # This one does not start anything
//!
//! ```text
//! # @option --dir=`_default_dir`     a default computed by a function
//! # @arg host[`_choice_host`]        completions computed by a function
//! ```
//!
//! Upstream runs those by exec'ing `bash script.sh ___internal___ _default_dir …` and reading its
//! stdout. oslo is a shell: it has the script's source, a parser and an evaluator, so it runs the
//! function **in a subshell of this process** and captures the output the way `$(…)` does. That is
//! faster, needs no bash on the machine, and works for code that has no file at all — a stored
//! function, whose `$0` is its name rather than a path.
//!
//! # Everything else is what the shell already knows
//!
//! `env_var` is this shell's variables, not the process environment: a variable set by the script
//! and not exported is still a variable the script can see, and `argc` asking about one has to get
//! the same answer the script would.

use crate::env::Environment;
use std::collections::HashMap;
use std::path::Path;

/// The shell, as `argc` sees it.
///
/// **Copy, because `argc::Runtime` demands it** — the trait is `Copy + Clone` so it can be handed to
/// several threads at once upstream. That rules out holding `&mut Environment`, so what is carried
/// is a raw pointer to it and the promise that this lives no longer than one builtin call.
/// **The environment is optional, and its absence is a real case.** `oslo.args.parse` is a Lua
/// binding that has to work from inside a registered builtin and a completion provider — the two
/// places the shell is holding its own state and cannot be borrowed. Without a shell to ask, the
/// process environment answers instead and a `` `_default_fn` `` computes to nothing, which is the
/// honest result rather than a refusal: a declaration that uses neither is the overwhelming case.
#[derive(Clone, Copy)]
pub struct Shell {
    env: Option<*mut Environment>,
}

impl Shell {
    /// The shell `argc` will ask its questions of.
    ///
    /// **The borrow is the contract.** Taking `&mut Environment` and keeping only a pointer means
    /// the caller cannot hand the same environment to anything else for as long as this lives, and
    /// both callers — the builtin and `oslo --argc-eval` — make one from their own `&mut` and drop
    /// it before returning.
    pub fn new(env: &mut Environment) -> Shell {
        Shell { env: Some(env) }
    }

    /// The same, with no shell behind it — for a caller that cannot borrow one.
    pub fn detached() -> Shell {
        Shell { env: None }
    }

    #[allow(clippy::mut_from_ref)]
    fn env(&self) -> Option<&mut Environment> {
        // SAFETY: the pointer came from a `&mut Environment` in `builtin_argc`, which owns the
        // borrow for the whole call and hands out no other reference to it.
        self.env.map(|env| unsafe { &mut *env })
    }
}

impl argc::Runtime for Shell {
    fn os(&self) -> String {
        // Linux only, and oslo says so everywhere else too.
        "linux".to_string()
    }

    fn shell_path(&self) -> argc::anyhow::Result<String> {
        // Runnable rather than merely named: argc starts this, and an install replaces the running
        // binary so its resolved path can no longer be executed. See `oslo_base::exe`.
        Ok(oslo_base::exe::path().to_string_lossy().into_owned())
    }

    fn bash_path(&self) -> Option<String> {
        // **This shell, not a bash somewhere.** What upstream wants here is something that can run
        // the script's functions, and that is what oslo is.
        self.shell_path().ok()
    }

    /// Run the named functions of `script_file` and answer with what each printed.
    ///
    /// In this process: the source is parsed and the function called in a subshell, so the script
    /// cannot change the shell that is parsing its arguments. The order of the answers is the order
    /// of `functions`, which is what the caller indexes by.
    fn exec_bash_functions(
        &self,
        script_file: &str,
        functions: &[&str],
        args: &[String],
        envs: HashMap<String, String>,
    ) -> Option<Vec<String>> {
        // No shell means no evaluator to run them in, and an empty answer per function is what the
        // parser reads as "this default computed to nothing".
        let Some(env) = self.env() else {
            return Some(functions.iter().map(|_| String::new()).collect());
        };
        let source = self.read_to_string(script_file)?;
        Some(
            functions
                .iter()
                .map(|function| super::call::capture(env, &source, function, args, &envs))
                .collect(),
        )
    }

    fn current_exe(&self) -> Option<String> {
        self.shell_path().ok()
    }

    fn current_dir(&self) -> Option<String> {
        std::env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
    }

    fn env_vars(&self) -> HashMap<String, String> {
        match self.env() {
            Some(env) => env.get_all_vars(),
            None => std::env::vars().collect(),
        }
    }

    fn env_var(&self, name: &str) -> Option<String> {
        match self.env() {
            Some(env) => env.get_var(name).map(str::to_string),
            None => std::env::var(name).ok(),
        }
    }

    /// `$PATH`, through the same table `hash` fills.
    ///
    /// Not the `which` crate: the shell already resolves command words this way, and a second
    /// answer to "where is `git`" is a second answer that can disagree with the first.
    fn which(&self, name: &str) -> Option<String> {
        crate::env::builtins::hash_lookup(name).map(|path| path.to_string_lossy().into_owned())
    }

    fn exist_path(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        Path::new(path)
            .parent()
            .map(|path| path.to_string_lossy().into_owned())
    }

    fn join_path(&self, path: &str, parts: &[&str]) -> String {
        let mut path = Path::new(path).to_path_buf();
        for part in parts {
            path = path.join(part);
        }
        path.to_string_lossy().into_owned()
    }

    fn chdir(&self, cwd: &str, cd: &str) -> Option<String> {
        let path = Path::new(cwd).join(cd).canonicalize().ok()?;
        Some(path.to_string_lossy().into_owned())
    }

    fn metadata(&self, path: &str) -> Option<(bool, bool, bool)> {
        use std::os::unix::fs::PermissionsExt;
        let mut meta = std::fs::symlink_metadata(path).ok()?;
        let is_symlink = meta.is_symlink();
        if is_symlink {
            meta = std::fs::metadata(path).ok()?;
        }
        Some((
            meta.is_dir(),
            is_symlink,
            meta.permissions().mode() & 0o111 != 0,
        ))
    }

    fn read_dir(&self, path: &str) -> Option<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path).ok()? {
            names.push(entry.ok()?.file_name().to_string_lossy().into_owned());
        }
        Some(names)
    }

    /// The script's text.
    ///
    /// **A stored function is looked up before the filesystem**, because it has no path: it runs
    /// from the database and knows only its own name. Without this, every `# @option` in one would
    /// be invisible to the parser that is meant to read it.
    fn read_to_string(&self, path: &str) -> Option<String> {
        if let Some(body) = stored_function(path) {
            return Some(body);
        }
        std::fs::read_to_string(path).ok()
    }
}

/// The body of the stored function called `name`, if there is one.
fn stored_function(name: &str) -> Option<String> {
    if name.contains('/') || !oslo_base::macros::valid_name(name) {
        return None;
    }
    let store = oslo_base::macros::open().ok()?;
    oslo_base::macros::get(&store, oslo_base::macros::Kind::Func, name)
        .filter(|entry| entry.active)
        .map(|entry| entry.body)
}
