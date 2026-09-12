mod array;
mod calls;
mod environ;
mod frames;
mod names;
mod options;
pub mod origin;
mod record;
mod registry;
mod seed;
#[cfg(test)]
mod tests;
mod traps;
mod vars;

use crate::env::nesting::{DepthGuard, MAX_FUNCTION_DEPTH, MAX_SCRIPT_DEPTH};
use crate::env::options::ShellOptions;
use environ::{environ_remove, environ_set, reject_unrepresentable};
use frames::ScopeFrame;
use oslo_base::ast::Command;
use oslo_base::error::Result;
use registry::BuiltinRegistry;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

pub use array::{ShellArray, array_literal_body};
pub use registry::{BuiltinFn, is_special_builtin};

/// One running process substitution: the descriptor the caller was given, and the child feeding it.
///
/// **Declared here, beside the list that holds it, rather than in `exec::procsub` where it is
/// opened and closed.** The store is the bottom of the shell's dependency graph — nothing it holds
/// may point back up at the executor — and this was the single field that did. Two OS handles is
/// all it is; the machinery that fills them in stays with the code that forks.
pub struct Substitution {
    pub fd: std::os::fd::RawFd,
    pub child: nix::unistd::Pid,
}

/// What a call frame entered without a name reads as, in `caller`'s output.
///
/// bash's own spelling for the same gap: `f() { caller; }; f` in `bash -c` prints `1 NULL`,
/// because there is no file for the frame to have come from.
pub const UNNAMED_FUNCTION: &str = "NULL";

/// Whether `name` is a valid shell variable name: `[A-Za-z_][A-Za-z0-9_]*`.
///
/// `export`, `local` and `readonly` reject anything else *before* it can reach the process
/// environment, because `std::env::set_var` panics on a name the OS refuses (empty, or
/// containing `=`) and a panic in the interpreter loop takes an interactive session with it.
pub fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub struct Environment {
    vars: HashMap<String, (String, bool)>, // (value, is_exported)
    /// Indexed arrays, in the same namespace as [`Self::vars`]: a name is a scalar or an array,
    /// never both. See the `array` submodule for why the two tables stay separate.
    arrays: HashMap<String, ShellArray>,
    positional: Vec<String>,
    pub last_status: i32,
    pub pid: u32,
    pub last_bg_pid: Option<u32>,
    pub shell_name: String,
    /// This process's own pid, which differs from [`Self::pid`] inside a forked subshell: `$$`
    /// keeps reporting the *invoking* shell (POSIX; `bash -c 'echo $$; (echo $$)'` prints one
    /// number twice), so job control and `$BASHPID` need the real one kept separately.
    current_pid: u32,
    /// Exit status of every stage of the most recent pipeline, left to right. Published as the
    /// `PIPESTATUS` array by [`Self::set_pipeline_status`]; `pipefail` reads the same vector.
    pipeline_status: Vec<i32>,
    /// Exit status of the last command substitution, until something consumes it.
    substitution_status: Option<i32>,
    /// How many lines of the file come *before* the chunk now running.
    ///
    /// **Nearly always zero.** A script that parses is evaluated whole, and every line number in
    /// its tree is already the file's own. A script that does *not* parse takes the slow path in
    /// `run_line_at_a_time`, which runs one command at a time — and each of those is parsed on its
    /// own, so its tree counts from 1 whatever line of the file it came from. Without this, one
    /// syntax error anywhere in a script made every diagnostic before it say `line 1`.
    line_offset: u32,
    /// What `$LINENO` was last set to by [`Self::note_line`], or 0 when something else touched it.
    published_line: u32,
    aliases: HashMap<String, String>,
    /// Stored variables that have not been asked for yet: name to the *recipe* for its value.
    ///
    /// `oslo macros add --var GITHUB_TOKEN '$(oslo secret get gh-token)'` puts a line here, not a
    /// token. The first expansion of `$GITHUB_TOKEN` runs it, exports the result, and takes the
    /// entry out — so the cost of a variable that decrypts something is paid by the command that
    /// needed it and by no other, and a shell that never mentions the name never runs it at all.
    lazy: HashMap<String, String>,
    functions: HashMap<String, Arc<Command>>,
    /// Every builtin this shell has. The one list consulted by [`Self::is_builtin`], the
    /// dispatcher in `exec::simple` and `type`; see the `registry` submodule.
    builtins: BuiltinRegistry,
    /// Process substitutions opened while expanding the command now being prepared.
    ///
    /// Lives here because the descriptor has to outlive *expansion* — the program opens
    /// `/dev/fd/N` only once every word is expanded and the command runs — but must not outlive
    /// the command. `crate::exec::simple` closes them when it is done.
    pub procsubs: Vec<Substitution>,
    signal_traps: HashMap<String, String>,
    /// Traps this shell inherited and then reset because it is a subshell.
    ///
    /// POSIX resets a subshell's traps to their default *action*, but carves out command
    /// substitution so that `saved=$(trap)` — the save-and-restore idiom — still reports what the
    /// parent had. Keeping the strings here separates the two: nothing ever *runs* from this map,
    /// and [`Self::listable_traps`] is the only thing that reads it.
    inherited_traps: HashMap<String, String>,
    readonly_vars: HashSet<String>,
    /// Names declared with `-i`, whose assignments are evaluated as arithmetic. See
    /// [`Environment::set_integer`].
    integer_vars: HashSet<String>,
    /// Names `export` was told about before they existed.
    ///
    /// `export V` with no value marks `V` for export and leaves it **unset**: bash gives an empty
    /// `${V+set}` and no `V=` in `env`, and a later `V=1` is exported. Creating it empty instead
    /// made it answer "set" to every test, and put a spurious `V=` in every child's environment.
    /// [`Self::set_var`] spends the intention when the name is finally assigned.
    pending_exports: HashSet<String>,
    dir_stack: Vec<PathBuf>,
    scope_stack: Vec<ScopeFrame>,
    /// How many loops are currently executing in this shell.
    ///
    /// `break` and `continue` unwind as errors, which would abandon the rest of the enclosing
    /// command list. Outside any loop they must instead do nothing at all — `break; echo hi`
    /// still prints `hi` — so the builtins consult this before signalling.
    loop_depth: usize,
    /// How deep the current shell-function call chain is.
    function_depth: DepthGuard,
    /// The name of every shell function on that chain, outermost first. Pushed and popped with
    /// the counter above; read by `caller`.
    call_stack: Vec<String>,
    /// How the code now running was reached: `main` for a script file, `source` for a sourced one.
    ///
    /// **Kept apart from [`Self::call_stack`] on purpose.** bash puts these in `$FUNCNAME` — a
    /// function called from a script file sees `f main` — but they are not function frames, and
    /// `caller` decides "am I inside a function at all" by asking whether that stack is empty. A
    /// sentinel pushed there would make `caller` answer yes at the top level of every script, and
    /// the `while caller $i` idiom depends on it answering no.
    script_frames: Vec<String>,
    /// The files whose commands are running, innermost last — for a diagnostic's location only.
    /// See [`Environment::enter_source_file`].
    source_files: Vec<String>,
    /// The `line_offset` each entry of `source_files` displaced, to be put back on the way out.
    source_offsets: Vec<u32>,
    /// Whether the command just before this one was an `exit` refused over stopped jobs.
    ///
    /// See `builtins::control::refuse_over_stopped_jobs`. On `Environment` rather than a global
    /// because a subshell must not inherit half a confirmation.
    exit_warned: bool,
    /// How deep the current `source`/`eval` chain is.
    script_depth: DepthGuard,
    /// The `set -e`/`set -o pipefail` options. Read through the accessors in the `options`
    /// submodule, which is where the whole option API lives.
    options: ShellOptions,
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        // **`env::vars()` panics on a byte that is not UTF-8**, and this runs for every mode the
        // shell has — `-c`, a script, a subshell, the prompt. One Latin-1 byte anywhere in the
        // environment therefore killed the process with `SIGABRT` before a single command ran, and
        // the ordinary way to get one is a directory named in a locale that is not this one.
        //
        // Such a name is **skipped rather than mangled**. The shell cannot hold it — a Rust
        // `String` is UTF-8 — and replacing the byte would hand a child a value that is neither
        // what was set nor what it expects. Skipped, the variable is simply not oslo's to read, and
        // the real bytes still reach every child through the environment `execve` inherits.
        for (key, value) in env::vars_os() {
            if let (Some(key), Some(value)) = (key.to_str(), value.to_str()) {
                vars.insert(key.to_string(), (value.to_string(), true));
            }
        }

        let pid = std::process::id();

        // **No aliases here.** See [`Environment::seed_interactive_aliases`]: the three
        // conveniences oslo ships belong to a person at a prompt, and this constructor is also
        // every script, every `sh -c` and every subshell.
        let aliases = HashMap::new();

        let mut env_struct = Self {
            vars,
            arrays: HashMap::new(),
            positional: Vec::new(),
            last_status: 0,
            pid,
            last_bg_pid: None,
            shell_name: "oslo".to_string(),
            current_pid: pid,
            pipeline_status: vec![0],
            substitution_status: None,
            published_line: 0,
            line_offset: 0,
            aliases,
            lazy: HashMap::new(),
            functions: HashMap::new(),
            builtins: BuiltinRegistry::default(),
            procsubs: Vec::new(),
            signal_traps: HashMap::new(),
            inherited_traps: HashMap::new(),
            readonly_vars: HashSet::new(),
            integer_vars: HashSet::new(),
            pending_exports: HashSet::new(),
            dir_stack: Vec::new(),
            scope_stack: Vec::new(),
            loop_depth: 0,
            function_depth: DepthGuard::new(MAX_FUNCTION_DEPTH),
            call_stack: Vec::new(),
            script_frames: Vec::new(),
            source_files: Vec::new(),
            source_offsets: Vec::new(),
            exit_warned: false,
            script_depth: DepthGuard::new(MAX_SCRIPT_DEPTH),
            options: ShellOptions::default(),
        };

        crate::env::dynamic::start();
        crate::env::builtins::register_default_builtins(&mut env_struct);
        env_struct.seed_process_vars();
        env_struct.seed_field_separator();
        env_struct.seed_working_directory();
        env_struct.seed_option_index();
        env_struct.seed_compatibility_vars();
        env_struct
    }

    /// Mark this environment as the one a freshly forked subshell is running in.
    ///
    /// A subshell *is* the shell: `fork` already copied every variable with its export flag,
    /// every function, alias, positional parameter, `$0`, `$?`, readonly mark and the directory
    /// stack, so the child keeps what it inherited and only genuinely subshell-local state is
    /// refreshed here. Rebuilding an `Environment::new()` instead lost all of that *and*
    /// re-exported private variables into the child's `environ` — `x=1; (env | grep '^x=')`
    /// used to leak a variable the parent never exported.
    ///
    /// Refreshed: the recorded pid, and the traps, which POSIX resets to their default action in
    /// a subshell (`trap 'echo T' EXIT; (:)` prints `T` once). Deliberately kept: `$$`, the
    /// invoking shell's pid, and `$!`, which bash inherits into a subshell.
    pub fn enter_subshell(&mut self) {
        self.current_pid = std::process::id();
        // Moved rather than dropped: the actions stop applying, but `$(trap)` still has to be
        // able to report them. See [`Self::inherited_traps`].
        self.inherited_traps = std::mem::take(&mut self.signal_traps);
    }

    /// This process's real pid, as opposed to `$$`. See [`Self::enter_subshell`].
    pub fn current_pid(&self) -> u32 {
        self.current_pid
    }

    pub fn enter_loop(&mut self) {
        self.loop_depth += 1;
    }

    pub fn exit_loop(&mut self) {
        self.loop_depth = self.loop_depth.saturating_sub(1);
    }

    /// How many loops enclose the command being run.
    ///
    /// `break n` is clamped to it: POSIX says that when `n` is greater than the number of enclosing
    /// loops, the **outermost** one is exited — not that the script is abandoned.
    pub fn loops(&self) -> usize {
        self.loop_depth
    }

    /// Whether a `break` or `continue` has a loop to act on.
    pub fn in_loop(&self) -> bool {
        self.loop_depth > 0
    }

    /// Begin a nested `source` or `eval`; `Err` once the chain is too deep to be safe.
    pub fn enter_nested_script(&mut self) -> Result<()> {
        self.script_depth.enter()
    }

    pub fn exit_nested_script(&mut self) {
        self.script_depth.exit()
    }

    pub fn set_readonly(&mut self, name: &str) {
        self.readonly_vars.insert(name.to_string());
    }

    pub fn is_readonly(&self, name: &str) -> bool {
        self.readonly_vars.contains(name)
    }

    /// Mark `name` read-only *for the innermost scope*, as `local -r` and `declare -r` do.
    ///
    /// **The mark leaves with the scope.** It used to go into the process-wide set with nothing to
    /// take it out again, so `f() { local -r x=1; }; f` left `x` frozen for the rest of the
    /// session — a name that could never be assigned and had no value under it. bash scopes the
    /// mark for `local` and `declare` and keeps it global for the `readonly` builtin, which is why
    /// this is a second entry point rather than a change to [`Self::set_readonly`].
    ///
    /// At the top level there is no frame to leave, so this is the global mark.
    pub fn set_readonly_here(&mut self, name: &str) {
        // Already read-only means this scope is not what made it so, and must not release it.
        if !self.readonly_vars.contains(name) {
            self.note_scope_readonly(name);
        }
        self.set_readonly(name);
    }

    /// Drop the read-only mark. Only [`Environment::pop_scope`] calls this.
    pub(super) fn release_readonly(&mut self, name: &str) {
        self.readonly_vars.remove(name);
    }

    /// Mark `name` as holding an integer: every assignment to it is arithmetic from now on.
    ///
    /// A name set, the way `readonly` is one, because the attribute belongs to the *name* and has
    /// to outlive any particular value — `local -i n; n=4*5` declares first and assigns after.
    pub fn set_integer(&mut self, name: &str) {
        self.integer_vars.insert(name.to_string());
    }

    pub fn is_integer(&self, name: &str) -> bool {
        self.integer_vars.contains(name)
    }

    /// Drop the attribute, for `declare +i`.
    pub fn clear_integer(&mut self, name: &str) {
        self.integer_vars.remove(name);
    }

    pub fn push_dir(&mut self, path: PathBuf) {
        self.dir_stack.push(path);
    }

    pub fn pop_dir(&mut self) -> Option<PathBuf> {
        self.dir_stack.pop()
    }

    pub fn get_dir_stack(&self) -> &[PathBuf] {
        &self.dir_stack
    }

    /// Run `name` as a builtin, or `None` if it is not one.
    ///
    /// The only way a builtin is ever invoked. A caller that has already asked
    /// [`Self::is_builtin`] still goes through here rather than through a name-to-function
    /// `match` of its own: the second list is what let `register_custom_builtin("echo", …)`
    /// register a function nothing would ever call (PLAN R5.6, R9.8).
    pub fn exec_custom_builtin(&mut self, name: &str, args: &[String]) -> Option<Result<i32>> {
        let func = self.builtins.lookup(name)?;
        // Published here because this is the only way in, so one write covers every builtin and
        // every private helper under it — see `origin::here`.
        let _origin = origin::Published::new(self.origin());
        Some(func(self, args))
    }
    pub fn set_positional(&mut self, params: Vec<String>) {
        self.positional = params;
    }

    pub fn get_positional(&self) -> &[String] {
        &self.positional
    }

    /// Install `params` as `$1…` and hand back what was there.
    ///
    /// What a function call wants. Reading the caller's parameters out and putting them back
    /// afterwards copied every one of them twice per call; both halves are moves here.
    pub fn swap_positional(&mut self, params: Vec<String>) -> Vec<String> {
        std::mem::replace(&mut self.positional, params)
    }
}

/// The editor's view of the shell.
///
/// **The store implements the interface rather than the editor holding the store.** Completion and
/// highlighting need to ask eight questions — what is `$PS2`, what is on `$PATH`, which names are
/// builtins, aliases or functions — and holding an `Environment` to ask them pointed the interface
/// layer at the shell, which is built on top of it. See [`oslo_ui::shell::Shell`].
impl oslo_ui::shell::Shell for Environment {
    fn interactive(&self) -> bool {
        Environment::interactive(self)
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.get_var(name)
    }

    fn vars(&self) -> HashMap<String, String> {
        self.get_all_vars()
    }

    fn is_builtin(&self, name: &str) -> bool {
        Environment::is_builtin(self, name)
    }

    fn builtin_names(&self) -> Vec<String> {
        Environment::builtin_names(self)
            .map(str::to_string)
            .collect()
    }

    fn alias(&self, name: &str) -> Option<&str> {
        self.get_alias(name)
    }

    fn aliases(&self) -> &HashMap<String, String> {
        self.get_aliases()
    }

    fn is_function(&self, name: &str) -> bool {
        self.get_function(name).is_some()
    }

    fn functions(&self) -> &HashMap<String, Arc<Command>> {
        self.get_functions()
    }
}
