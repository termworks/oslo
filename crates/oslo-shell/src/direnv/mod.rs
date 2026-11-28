//! Directory environments: one `.env.lua` per project.
//!
//! `direnv`, built in. See `docs/research/direnv.md` for what was read and what was deliberately not
//! copied; the short version is that most of direnv's machinery exists because it is an external
//! binary talking to a shell it did not write, and oslo *is* the shell. There is no bash subprocess,
//! no prompt hook and no `eval` protocol: the state lives in memory and the load happens on the
//! `cd` path.
//!
//! **One name, and it is oslo's own.** `.env.lua`, in the language the rest of the configuration is
//! written in.
//!
//! `.envrc` was read too, for a while, against a reimplementation of direnv's stdlib — 1100 lines
//! of Rust tracking 1.4k lines of someone else's bash so that `use flake` and `layout python` meant
//! here what they mean there. Every one of those is a place for the two to disagree in a way only a
//! user hits, and none of it is work oslo has to do: direnv exists, it is on the machine, and it is
//! good at this.
//!
//! So an `.envrc` project runs direnv, from one line in `init.lua`:
//!
//! ```lua
//! oslo.env.set("PROMPT_COMMAND", 'eval "$(direnv export bash)"')
//! ```
//!
//! `$PROMPT_COMMAND` is evaluated against the live environment before every prompt, which is what
//! direnv's own bash hook is built on — load *and* unload, with no oslo code in the middle. Pair it
//! with `oslo.command.when` and `oslo.feature.when` and each tool takes the directories that are
//! its own.
//!
//! Two things *are* copied, because neither is incidental to being an external binary:
//!
//! * **the allow gate.** A file in a directory getting to run code when you walk in is arbitrary
//!   code execution by anyone who can get you to clone a repository, and [`allow`] is the answer,
//!   hash for hash;
//! * **the undo record in the environment**, direnv's `DIRENV_DIFF`. This one was skipped at first,
//!   on the reasoning above, and skipping it was wrong — see [`carry`]. The variables have to go
//!   into the real `environ` for a child to see them at all, so every child inherits them; without
//!   a record travelling alongside, a child holds one project's `$PATH` in every directory it ever
//!   visits and has nothing to put back.
//!
//! # The lifecycle
//!
//! On a directory change, and only on a directory change:
//!
//! 1. find the nearest ancestor with rc files;
//! 2. if it is the same set, unchanged, do nothing — the common case, and it costs one `stat` each;
//! 3. otherwise **unload first**, always, so moving between two projects cannot leave the first
//!    one's variables behind;
//! 4. then load the new one, if it is allowed.
//!
//! A shell that inherited an environment starts at step 3 with the record its parent left, which is
//! how it comes to be holding something it never loaded and can still give it back.

pub mod allow;
pub mod carry;
pub mod diff;
pub mod find;
mod handle;
pub mod paths;

pub use handle::{install, installed, request_reload, take_reload_request, with};

use crate::env::Environment;
use allow::{Allow, Status};
use diff::{Change, Diff};
use find::Rc;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

/// What is loaded right now, if anything.
struct Loaded {
    /// The directory the rc files live in.
    owner: PathBuf,
    /// The files as they were when they were read, so an edit to any of them reloads.
    ///
    /// A list rather than the file alone, because `oslo.direnv.watch_file` adds a
    /// file the environment now depends on. Watching only the rc file is the oldest direnv bug
    /// report there is: a project whose `.env` is edited and whose shell goes on holding the values
    /// from before it.
    watch: Vec<(PathBuf, Stamp)>,
    /// The variables to put back on the way out, each with the export flag it had.
    undo: Diff<(String, bool)>,
    /// The builtins the directory added, to take back out on the way out.
    ///
    /// Names alone: a builtin is a function pointer with nothing to restore, so leaving removes
    /// the ones that arrived with the directory and touches no others.
    builtins: Vec<String>,
    /// The shell functions the directory added, on the same terms as the builtins above.
    functions: Vec<String>,
    /// The aliases to put back on the way out.
    ///
    /// Separate from `undo` rather than merged into it because an alias and a variable can share a
    /// name and mean different things — `ls` is a perfectly good alias and a perfectly good
    /// variable, and one map would have them overwrite each other.
    aliases: Diff<String>,
}

/// The shell's directory-environment state.
pub struct Direnv {
    allow: Allow,
    loaded: Option<Loaded>,
    /// Set when a decision has made the loaded state wrong, so the next [`Direnv::arrive`] does the
    /// work again even though the directory has not changed.
    stale: bool,
    /// Paths already reported as needing `direnv allow`, so the notice is printed once rather than
    /// on every prompt. direnv prints on every hook fire; that is noise, and noise gets ignored,
    /// which is the last thing a security prompt should be.
    told: Vec<PathBuf>,
}

/// What happened, for the caller to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Loaded `owner`, and what it did to the environment.
    ///
    /// The marks rather than a count: "11 variables" tells you nothing, and the whole reason to
    /// print anything at all when a directory changes is so you can see what it did to your shell
    /// without having to go and ask.
    Loaded {
        owner: PathBuf,
        changed: Vec<(String, Change)>,
        aliases: Vec<(String, Change)>,
        /// Names only, and only the ones that were not defined before.
        ///
        /// **Not a [`Change`] like the two above**, because a name set is all there is to compare:
        /// a function redefined under a name the shell already had looks identical to one left
        /// alone. Reporting it as `Added` would claim more than was measured — and it is the same
        /// limit the unload works under, which is why the two are the same list.
        functions: Vec<String>,
    },
    /// Left a directory environment behind.
    Unloaded { owner: PathBuf },
    /// Found something, but it has not been allowed. Nothing was read.
    Blocked { path: PathBuf },
    /// Found something explicitly refused.
    Denied { path: PathBuf },
    /// An allowed file was read and something in it went wrong.
    ///
    /// Reported rather than swallowed, and *beside* a `Loaded` rather than instead of it: whatever
    /// ran before the failure has taken effect, and pretending otherwise would leave the shell in a
    /// state it says it is not in.
    Failed { path: PathBuf, problem: String },
}

impl Direnv {
    pub fn new(xdg_data: Option<&str>, home: Option<&str>) -> Direnv {
        Direnv::adopting(xdg_data, home, std::env::var(carry::NAME).ok().as_deref())
    }

    /// [`Direnv::new`], told what it inherited rather than reading it from the environment.
    ///
    /// **An environment inherited from a parent is something this shell is holding.** Without the
    /// record a shell spawned from inside a project — a new pane, a nested `oslo`, an editor's
    /// terminal — starts believing nothing is loaded while holding every variable that project set.
    /// It then carries them into every directory it visits and no `cd` can shift them, because
    /// there is nothing recorded to put back. Adopting it means anywhere else unloads exactly as
    /// walking out would have. See [`carry`].
    ///
    /// **An inherited environment is adopted stale, so the first arrival re-runs the file even
    /// when the directory matches.** `execve` carries variables and nothing else, so a child
    /// standing in the project it inherited holds that project's `$PATH` while having none of its
    /// aliases, prompt or functions — the whole Lua half of the file never ran here. Treating that
    /// as loaded is what made `_b` report `command not found` in a shell whose `$PATH` was already
    /// the project's, with no `cd` able to fix it because the owner always matched. Being stale
    /// routes it through unload-then-load: the carried undo puts the variables back first, so the
    /// file runs against the environment it was written for rather than on top of its own output.
    ///
    /// Split from `new` so a test can hand it a record instead of writing one into the process's
    /// own environment — which is shared by every test running beside it.
    pub fn adopting(xdg_data: Option<&str>, home: Option<&str>, carried: Option<&str>) -> Direnv {
        let loaded = carried.and_then(carry::decode).map(|carried| Loaded {
            // The *file*, so what is watched is what would be re-read. The record carries a
            // directory; a directory's own stamp moves whenever anything inside it is written.
            // Whichever of the two names governs it — and none, if the file has since been
            // deleted, in which case the arrival unloads.
            watch: find::here(&carried.owner)
                .map(|rc| vec![(rc.path.clone(), stamp(&rc.path))])
                .unwrap_or_default(),
            owner: carried.owner,
            undo: carried.undo,
            // A child inherits variables and never aliases: `execve` carries the environment
            // and nothing else. Undoing aliases it was never given would restore the
            // *parent's* aliases into a shell that never had them.
            aliases: Diff::default(),
            // Empty for the same reason: a builtin lives in this process's registry, so a child
            // inherited none and has none to take back out. A shell function is the same — the
            // function table is this process's, and `execve` carries neither.
            builtins: Vec::new(),
            functions: Vec::new(),
        });
        Direnv {
            allow: Allow::new(xdg_data, home),
            stale: loaded.is_some(),
            loaded,
            told: Vec::new(),
        }
    }

    /// The allow store, for the `direnv` builtin.
    pub fn permissions(&self) -> &Allow {
        &self.allow
    }

    /// The directory whose environment is loaded, if any.
    pub fn active(&self) -> Option<&Path> {
        self.loaded.as_ref().map(|l| l.owner.as_path())
    }

    /// The names the loaded environment is holding — variables first, then aliases.
    pub fn holding(&self) -> Vec<&str> {
        let Some(loaded) = self.loaded.as_ref() else {
            return Vec::new();
        };
        let mut names = loaded.undo.names();
        names.extend(loaded.aliases.names());
        names
    }

    /// Forget that a path was reported, so the notice prints again after `direnv deny` or an edit.
    pub fn tell_again(&mut self, path: &Path) {
        self.told.retain(|told| told != path);
    }

    /// Mark the loaded state as no longer reflecting the decisions on record.
    ///
    /// **Not `loaded = None`.** Dropping the record is what an early version did, and it leaks: the
    /// undo diff is the only thing that knows how to take the variables back out, so forgetting it
    /// after `direnv deny` would leave a denied environment applied with no way to remove it. The
    /// record is kept and merely marked stale, so the next arrival unloads properly first.
    pub fn invalidate(&mut self) {
        self.stale = true;
        self.told.clear();
    }

    /// Bring the environment into line with `dir`. The whole feature, from the caller's side.
    ///
    /// `restore` puts back whatever the caller recorded at load time and this module cannot
    /// describe — the Lua prompt. Called on the way out, before the variables are restored, and
    /// always *before* `run`, which is what makes moving straight from one project to another put
    /// the base prompt back before the next file gets to set its own.
    ///
    /// `run` evaluates the `.env.lua` and reports what went wrong. A callback rather than something
    /// this module does, because running Lua needs the engine and that belongs to the read loop.
    ///
    /// **The environment lock is not held while it runs, and must not be.** The file's whole job is
    /// to call `oslo.env.set` and friends, and those take the same lock with `try_lock` — holding it
    /// here made every one of them fail with "shell state is busy" while the load reported success.
    /// So the lock is taken twice, briefly, for the before and after snapshots, and released around
    /// the call in between.
    pub fn arrive(
        &mut self,
        env: &Mutex<Environment>,
        dir: &Path,
        run: &mut dyn FnMut(&Rc) -> Result<(), String>,
        restore: &mut dyn FnMut(),
    ) -> Vec<Event> {
        // **The `direnv` feature, read as "there is no file here".** Turning it off has to *unload*
        // whatever is currently loaded, not merely decline to load the next one — a config that
        // turns oslo's own directory environments off on the way into a project
        // would otherwise keep the last project's variables for the rest of the session.
        //
        // Answering `None` here gets that for free: the code below already unloads before it loads,
        // and then returns early because there is nothing to load. It is the same path as walking
        // into a directory that has no `.env.lua`.
        let rc = oslo_base::feature::on(oslo_base::feature::at::DIRENV)
            .then(|| find::applicable(dir))
            .flatten();
        let owner = rc.as_ref().and_then(find::owner);

        // Standing in the same environment, with nothing edited: the overwhelmingly common case.
        if !self.stale
            && let Some(loaded) = &self.loaded
            && owner.as_deref() == Some(loaded.owner.as_path())
            && !self.changed_underneath()
        {
            return Vec::new();
        }
        self.stale = false;

        let mut events = Vec::new();
        // Unload before load, always. Moving from one project to another must not carry anything
        // across, and doing this in the other order silently merges the two.
        if let Some(event) = self.unload(env, restore) {
            events.push(event);
        }
        let Some(rc) = rc else {
            return events;
        };

        events.extend(self.load(env, &rc, run));
        events
    }

    /// Whether any loaded file has been written since it was read.
    fn changed_underneath(&self) -> bool {
        let Some(loaded) = &self.loaded else {
            return false;
        };
        loaded.watch.iter().any(|(path, when)| stamp(path) != *when)
    }

    /// Put back everything the loaded environment changed.
    fn unload(&mut self, env: &Mutex<Environment>, restore: &mut dyn FnMut()) -> Option<Event> {
        let loaded = self.loaded.take()?;
        // Anything the caller has to put back that this module cannot describe — the Lua prompt,
        // today. Run before the variables so a prompt function that reads one sees the directory's
        // value while it is still set, rather than the restored one it is about to be handed.
        restore();
        let mut guard = lock(env)?;
        for (name, was) in loaded.undo.to_apply() {
            match was {
                // The export flag is restored with the value. A variable that was shell-local
                // before the directory exported it has to come back local, or leaving would hand
                // every later child a variable that was never in its environment.
                Some((value, exported)) => {
                    // Unset first. `set_var`'s export flag is OR'd with whatever the variable
                    // already carries, so setting it to `false` on a currently-exported variable
                    // does nothing — the only way back to shell-local is to clear the entry and
                    // write it again.
                    guard.unset_var(name);
                    guard.set_var(name, value, *exported);
                }
                None => guard.unset_var(name),
            }
        }
        for (name, value) in loaded.aliases.to_apply() {
            match value {
                Some(value) => guard.set_alias(name, value),
                None => guard.remove_alias(name),
            }
        }
        // A builtin the directory registered is the directory's; leaving takes it back out. Without
        // this it stayed callable for the rest of the session, running the project's code with the
        // project's environment already put back.
        for name in &loaded.builtins {
            guard.unregister_custom_builtin(name);
        }
        // And a shell function it defined, which is the same claim on the same namespace.
        for name in &loaded.functions {
            guard.remove_function(name);
        }
        // Last, and unconditionally: the record describes an environment that no longer applies,
        // and leaving it behind would have the next child undo variables that are already back.
        guard.unset_var(carry::NAME);
        drop(guard);
        Some(Event::Unloaded {
            owner: loaded.owner,
        })
    }

    /// Check the gate, then read.
    fn load(
        &mut self,
        env: &Mutex<Environment>,
        rc: &Rc,
        run: &mut dyn FnMut(&Rc) -> Result<(), String>,
    ) -> Vec<Event> {
        match self.allow.status(&rc.path) {
            Status::Denied => {
                return vec![Event::Denied {
                    path: rc.path.clone(),
                }];
            }
            Status::NotAllowed => {
                // Reported once per path. A warning printed on every prompt is a warning nobody
                // reads, which is the worst outcome for a warning about running someone's code.
                if self.told.contains(&rc.path) {
                    return Vec::new();
                }
                self.told.push(rc.path.clone());
                return vec![Event::Blocked {
                    path: rc.path.clone(),
                }];
            }
            Status::Allowed => {}
        }

        let Some((before, aliases_before, builtins_before, functions_before)) =
            lock(env).map(|guard| shell_state(&guard))
        else {
            return Vec::new();
        };
        // Stamped *before* the run, so a file edited while it is being read is noticed rather than
        // recorded as already current.
        let mut watch = vec![(rc.path.clone(), stamp(&rc.path))];
        // No lock held here. See the note on `arrive`.
        let outcome = run(rc);
        // Whatever the file asked for while it ran: `watch_file`, and the files `dotenv` and
        // `source_env` read on its behalf. Only knowable now, which is why this is not one list.
        watch.extend(
            paths::take_watches()
                .into_iter()
                .map(|path| (stamp(&path), path))
                .map(|(when, path)| (path, when)),
        );
        let Some((after, aliases_after, builtins_after, functions_after)) =
            lock(env).map(|guard| shell_state(&guard))
        else {
            return Vec::new();
        };
        // **A file that failed half way still had its first half take effect**, so the diff is
        // taken regardless and the failure reported beside it. Discarding the load here would
        // leave those variables set with nothing recorded to unset them.
        // Taken forwards for reporting and reversed for undoing — the same comparison read in
        // both directions, so what is announced and what is put back cannot disagree.
        let forward = Diff::between(&before, &after);
        let changed: Vec<(String, Change)> = forward
            .changes()
            .into_iter()
            .map(|(name, kind)| (name.to_string(), kind))
            .collect();
        let undo = forward.reverse();
        let alias_diff = Diff::between(&aliases_before, &aliases_after);
        let alias_changes: Vec<(String, Change)> = alias_diff
            .changes()
            .into_iter()
            .map(|(name, kind)| (name.to_string(), kind))
            .collect();
        let aliases = alias_diff.reverse();

        // Named once and used twice: what leaving has to undefine is exactly what arriving
        // announces, so the two cannot drift into disagreeing about what the file defined.
        let mut functions: Vec<String> = functions_after
            .difference(&functions_before)
            .cloned()
            .collect();
        functions.sort();

        let Some(owner) = find::owner(rc) else {
            return Vec::new();
        };
        // **The undo travels with the variables it undoes.** Written after the snapshots, so it is
        // not itself part of the diff; exported, because the shells that need it are the ones this
        // environment reaches by `execve`. See [`carry`].
        if let Some(mut guard) = lock(env) {
            guard.set_var(carry::NAME, &carry::encode(&owner, &undo), true);
        }
        self.loaded = Some(Loaded {
            owner: owner.clone(),
            watch,
            undo,
            // Only the ones that were not there before: a directory re-registering a name the shell
            // already had must not take the shell's own away when it is left.
            builtins: builtins_after
                .difference(&builtins_before)
                .cloned()
                .collect(),
            // The same rule, for the same reason — see `shell_state`. A project's `.env.lua` that
            // sources its helpers had been leaving them defined for the rest of the session, which
            // is the one thing this whole module exists to prevent.
            functions: functions.clone(),
            aliases,
        });
        let mut events = vec![Event::Loaded {
            owner,
            changed,
            aliases: alias_changes,
            functions,
        }];
        if let Err(problem) = outcome {
            events.push(Event::Failed {
                path: rc.path.clone(),
                problem,
            });
        }
        events
    }
}

/// Everything a `.env.lua` can set that leaving must put back: variables, aliases, builtins.
///
/// Taken as one call so every half is read under the same lock, which is what makes the before and
/// after snapshots describe the same instant.
type Vars = BTreeMap<String, (String, bool)>;
type State = (
    Vars,
    BTreeMap<String, String>,
    BTreeSet<String>,
    BTreeSet<String>,
);

fn shell_state(env: &Environment) -> State {
    (
        env.all_vars()
            .into_iter()
            .filter(|(name, _, _)| !maintained(name))
            .map(|(name, value, exported)| (name, (value, exported)))
            .collect(),
        env.get_aliases().clone().into_iter().collect(),
        // Names only. A builtin is a function pointer with no previous value to restore: one the
        // directory added is removed on the way out, and one that was already there is left alone.
        env.builtin_names().map(str::to_string).collect(),
        // **Names only here too, and for a blunter reason.** A shell function's body is an AST
        // behind an `Arc`; a directory that redefines one the shell already had would need that
        // body kept to put it back, and keeping it would mean this module holding a parsed command.
        // So the rule is the builtins' rule: what the directory *added* leaves with it, and what it
        // redefined stays redefined. A `.env.lua` that sources a project's helpers is the case this
        // is for, and those are names the shell did not have.
        env.get_functions().keys().cloned().collect(),
    )
}

/// Variables the shell keeps for itself, which a directory environment neither sets nor undoes.
///
/// Reading a file moves `LINENO`, so a file that assigned nothing at all still came out of the
/// diff having "changed" it — announced on arrival and dutifully restored on the way out. `PWD` and
/// `OLDPWD` are the same mistake with teeth: a directory file runs with the working directory set to its
/// own, so both differ across the load by construction, and putting them "back" on the way out
/// would be a directory environment quietly moving the shell.
fn maintained(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "_",
        "BASHOPTS",
        "BASHPID",
        "EPOCHREALTIME",
        "EPOCHSECONDS",
        "LINENO",
        "OLDPWD",
        "PIPESTATUS",
        "PPID",
        "PWD",
        "RANDOM",
        "SECONDS",
        "SHELLOPTS",
        "SHLVL",
    ];
    NAMES.contains(&name)
}

/// The environment, or `None` if another holder has poisoned or is holding the lock.
///
/// A directory environment that cannot reach the shell state does nothing at all, which is the only
/// safe direction: half-applying it would leave variables set with no record of how to remove them.
fn lock(env: &Mutex<Environment>) -> Option<MutexGuard<'_, Environment>> {
    env.lock().ok()
}

/// What a file looked like when it was read: when it changed, and how long it was.
///
/// **Not the mtime alone.** Its granularity is one second on some filesystems, and editors that
/// write through a temporary file can even preserve it — so an rc file edited and re-entered
/// quickly would be reloaded without being re-checked against the allow list, which is the one
/// failure this whole module exists to prevent. The length is free from the same `stat` and closes
/// the common case. A file edited to the same length within the mtime granularity still slips
/// through, and the hash check on the next genuine reload is what catches that.
type Stamp = (Option<SystemTime>, Option<u64>);

fn stamp(path: &Path) -> Stamp {
    match std::fs::metadata(path) {
        Ok(meta) => (meta.modified().ok(), Some(meta.len())),
        Err(_) => (None, None),
    }
}

#[cfg(test)]
mod tests;
