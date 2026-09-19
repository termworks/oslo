//! The environment of a Nix dev shell, without entering one — direnv's `use flake` and `use nix`.
//!
//! Here rather than in the Lua API, where it started, because the delicate part is not the call:
//! it is the `IGNORED` list, the variables a dev shell exports that would wreck the shell you are
//! standing in. Getting that list wrong is silent and severe — see below.
//!
//! direnv's `use flake` is four lines of shell (`stdlib.sh`):
//!
//! ```sh
//! watch_file flake.nix; watch_file flake.lock
//! eval "$(nix print-dev-env --profile "$(direnv_layout_dir)/flake-profile" "$@")"
//! nix profile wipe-history --profile "$(direnv_layout_dir)/flake-profile"
//! ```
//!
//! The `eval` is the whole trick: `nix print-dev-env` emits **bash** that sets the environment, and
//! direnv hands it to the bash it is already running inside. oslo will not do that — evaluating a
//! hundred kilobytes of generated bash on every arrival is exactly the sort of thing a directory
//! environment must not do — so this uses `--json`, which is the same information as data.
//!
//! # The trap in `--json`, which cost an afternoon to find
//!
//! **The two forms do not contain the same variables.** Deriving the difference on nix 2.34 against
//! this repository's own flake:
//!
//! ```text
//! in --json but never emitted as bash:  HOME NIX_ENFORCE_PURITY NIX_LOG_FD TERM TZ
//! ```
//!
//! `nix` filters those out of the shell form because setting them would wreck the shell you are
//! standing in — `HOME` inside a derivation is `/homeless-shelter`. The JSON form applies no such
//! filter; it is a faithful dump of the builder's environment. So anything built on `--json` has to
//! reproduce that list, and a version that does not will silently repoint `$HOME` the moment you
//! `cd` into a flake. `IGNORED` is that list, widened to nix's full documented set so a variable
//! that is merely absent from this flake cannot appear in another one and break it.
//!
//! # The rule behind both of the bugs this module has had
//!
//! **`print-dev-env`'s bash output is curated for a shell to consume; `--json` is a raw dump of the
//! builder.** Everything the bash form does *besides* assigning values is invisible to `--json`:
//!
//! * variables it declines to emit at all — `HOME`, `TERM`, `TZ` and two more, handled by
//!   `IGNORED`;
//! * statements it runs after the assignments — the `PATH` restore, handled by `keeping_yours`.
//!
//! Both bugs were the same mistake twice: treating a snapshot of values as if it were the script.
//! A third difference of this kind is likely, so when something loaded from a flake behaves oddly,
//! diff the two forms before looking anywhere else — that is how both of these were found.

use crate::env::Environment;

pub mod cache;
pub mod json;
mod read;
pub use cache::forget;
use cache::{cached, remember};
pub use read::exported_from;
use read::{functions_from, locals_from};

/// Define them all in one pass.
///
/// **One `eval` rather than 110**, because each is a parse and this runs on arrival in a directory.
/// Measured at 40 ms for 66 KB in a debug build, which is the whole of what this feature costs.
fn define(env: &mut Environment, functions: &[(String, String)]) -> Result<(), String> {
    if functions.is_empty() {
        return Ok(());
    }
    let mut source = String::new();
    for (name, body) in functions {
        source.push_str(name);
        source.push_str(" ()\n{\n");
        source.push_str(body);
        source.push_str("\n}\n");
    }
    crate::env::builtins::builtin_eval(env, &["eval".to_string(), "--".to_string(), source])
        .map_err(|e| format!("defining the dev shell's functions: {e}"))?;
    Ok(())
}

/// The dev shell's `PATH`, with everything of yours that it does not already have, behind it.
///
/// **This is not our invention — it is a line `--json` cannot carry.** The bash form of
/// `print-dev-env` is bookended:
///
/// ```sh
/// line    3:  nix_saved_PATH="$PATH"                           # save yours
/// line   91:  PATH='/nix/store/…gcc-wrapper/bin:…'             # replace with the dev shell's
/// line 2195:  PATH="$PATH${nix_saved_PATH:+:$nix_saved_PATH}"  # and put yours back, behind it
/// ```
///
/// direnv `eval`s that script, so line 2195 runs and a zsh user keeps `clear` and `git`. `--json`
/// is a snapshot of *values*: line 2195 is a statement, and `nix_saved_PATH` is not among its 144
/// variables at all. So the append has to be done here, or it does not happen.
///
/// Without it, a dev shell's `PATH` is a *build* environment — 36 store entries for this
/// repository's flake, with `ls` and `grep` because coreutils is a build input, and no `clear`, no
/// `git`, and nothing you installed. `cd` into the project and the shell quietly loses half its
/// commands, which is exactly what happened the first time this was used for real.
fn keeping_yours(dev: &str, outer: &str) -> String {
    let mut out: Vec<&str> = dev.split(':').filter(|e| !e.is_empty()).collect();
    for entry in outer.split(':').filter(|e| !e.is_empty()) {
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out.join(":")
}

/// The command direnv runs, with the profile that keeps the shell from being garbage-collected.
///
/// `--profile` is not decoration: without it the dev shell's store paths have no GC root, and the
/// next `nix store gc` deletes the toolchain out from under a directory that still points at it.
/// direnv puts the profile under its layout directory and so do we.
///
/// **`args` is passed through verbatim, and nothing here reads it.** direnv's `use_flake` ends in
/// `nix print-dev-env --profile "$profile" "$@"`, which is what makes
/// `use flake --option warn-dirty false` work: the flags are nix's, not direnv's. Taking the first
/// argument as the installable instead turned that line into `print-dev-env … '--option'` and nix
/// answered "flag '--option' requires 2 argument(s), but only 0 were given" — a message about a
/// flag nobody asked to be alone.
///
/// An empty list is left empty rather than defaulting to `.`: `print-dev-env` already resolves the
/// current directory's flake, which is how a bare `use flake` works in direnv too.
pub fn command(args: &[String], profile: &str) -> String {
    let mut out = format!(
        "nix --extra-experimental-features 'nix-command flakes' print-dev-env --json --profile {}",
        shell_quote(profile),
    );
    for arg in args {
        out.push(' ');
        out.push_str(&shell_quote(arg));
    }
    out
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The profile path, created if it is not there, under the project's `.direnv`.
pub fn profile() -> String {
    let profile = cache::root().join(".direnv/flake-profile");
    if let Some(parent) = profile.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    profile.to_string_lossy().into_owned()
}

/// Run `nix print-dev-env` with `args` and apply what it exports. Answers how many.
///
/// Called by `oslo.direnv.nix_develop()` from a `.env.lua` — the same thing direnv's `use flake`
/// does, done here so a `.env.lua` needs no direnv.
pub fn apply(env: &mut Environment, args: &[String]) -> Result<usize, String> {
    apply_with(env, args, Want::default())
}

/// What a project wants out of the dev shell beyond its exported variables.
///
/// **Both are off by default, and for the same reason.** Variables are data; these two are code
/// that somebody else wrote, run on every arrival in the directory. `nix develop` and nix-direnv
/// give you them, plain direnv does not, and oslo sides with plain direnv until a project says
/// otherwise — so that `cd` into a clone of a stranger's repository is not an invitation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Want {
    /// Run `shellHook` after the variables are set.
    pub hook: bool,
    /// Define the dev shell's shell functions — `runHook`, `buildPhase`, `substituteInPlace` and
    /// the hundred or so others stdenv brings.
    pub functions: bool,
    /// Evaluate with `--impure`, so the flake may read the environment.
    ///
    /// **`builtins.getEnv` answers `""` in a pure evaluation, whatever the variable holds.** That
    /// is nix's rule and not a mistake here: a flake that reads its surroundings evaluates to
    /// something different on two machines, so nix asks to be told that is wanted. Without this
    /// there was no way to ask at all — a string argument is the *installable*, so
    /// `nix_develop("--impure")` sent nix `--impure` as the thing to build.
    ///
    /// The evaluation is **not cached** when this is on: what the flake read is unknowable from
    /// here, so the cache's input stamps cannot say whether the answer still holds.
    pub impure: bool,
}

/// Why `print-dev-env` said nothing, **asked of nix rather than guessed at**.
///
/// This used to answer "is nix installed, and does this directory have a flake?" — two guesses, and
/// on the commonest failure both are wrong. `use flake .#tooling` in a project whose shell is called
/// `dev` reaches here with nix installed and a flake right there; nix's own message names the four
/// attribute paths it looked for and not one that exists. So oslo said something untrue while the
/// useful fact — *what this flake actually offers* — was one query away.
///
/// **Only on the failure path.** A `flake show` costs 34 ms warm and 455 ms cold, and paying that
/// on every arrival in a directory to prepare for an error that will not happen is exactly the sort
/// of tax a directory environment must not levy. Nothing here runs while `use flake` is working.
fn why_nothing(args: &[String]) -> String {
    if !json::available() {
        return "nix is not installed".to_string();
    }
    match shells_here() {
        Some(names) if !names.is_empty() => format!(
            "no dev shell called `{}`. This flake offers: {}",
            asked_for(args),
            names.join(", ")
        ),
        Some(_) => "this flake defines no dev shell for this system".to_string(),
        // The flake could not be read at all — nix has already said why on its own stderr, and
        // repeating a guess over the top of a real diagnostic helps nobody.
        None => "`nix print-dev-env` produced nothing; nix's own message says why".to_string(),
    }
}

/// The dev shell the line asked for, as it would be named in the flake.
///
/// `use flake .#tooling` asks for `tooling`; a bare `use flake` asks for `default`, which is what
/// nix resolves an installable with no attribute to. Flags are skipped, because
/// `use flake --option warn-dirty false` names no shell at all.
fn asked_for(args: &[String]) -> String {
    args.iter()
        .find(|arg| !arg.starts_with('-'))
        .and_then(|installable| installable.split_once('#'))
        .map(|(_, attribute)| attribute.to_string())
        .unwrap_or_else(|| "default".to_string())
}

/// The dev shells this flake offers for this machine.
///
/// **A second implementation of `oslo.nix.shells`, and deliberately.** That one is Lua, and the Lua
/// layer sits *above* the shell — this is an error message inside the shell, which cannot call up
/// into it. The duplication is bounded by what it is for: the worst a drifted copy can do is name
/// the wrong shells in a diagnostic.
fn shells_here() -> Option<Vec<String>> {
    let system = local_system()?;
    let shown = json::run(&["flake".to_string(), "show".to_string()], json::TIMEOUT).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&shown).ok()?;
    let shells = parsed.get("devShells")?.get(system)?.as_object()?;
    Some(shells.keys().cloned().collect())
}

/// The system nix builds for here — `x86_64-linux`.
fn local_system() -> Option<String> {
    let shown = json::run(&["config".to_string(), "show".to_string()], json::TIMEOUT).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&shown).ok()?;
    Some(parsed.get("system")?.get("value")?.as_str()?.to_string())
}

/// The same as [`apply`], plus whatever `want` asks for.
pub fn apply_with(env: &mut Environment, args: &[String], want: Want) -> Result<usize, String> {
    // nix's own flag, in nix's own argument list — the caller's words are forwarded verbatim, and
    // this goes in front of them so an installable stays the first thing that is not a flag.
    let args = &match want.impure {
        true => [vec!["--impure".to_string()], args.to_vec()].concat(),
        false => args.to_vec(),
    };
    // **An impure evaluation is never served from the cache, and never written to it.** It read
    // something this side cannot see — an environment variable, a file outside the flake — so the
    // input stamps the key is built from cannot say whether the answer still holds. Paying the
    // evaluation on every arrival is the price of asking a flake to read its surroundings.
    let json = match cached(args).filter(|_| !want.impure) {
        Some(remembered) => remembered,
        None => {
            let fresh = crate::exec::eval_command_substitution(env, &command(args, &profile()))
                .map_err(|e| e.to_string())?;
            if fresh.trim().is_empty() {
                return Err(why_nothing(args));
            }
            // Written only after it parses, so a truncated or error-shaped answer is not the thing
            // every later arrival is served from.
            exported_from(&fresh)?;
            if !want.impure {
                remember(args, &fresh);
            }
            fresh
        }
    };
    let exported = exported_from(&json)?;
    let count = exported.len();
    let outer_path = env.get_var("PATH").unwrap_or_default().to_string();
    let mut script = None;
    for (name, value) in exported {
        if name == "PATH" {
            env.set_var(&name, &keeping_yours(&value, &outer_path), true);
            continue;
        }
        if want.hook && name == "shellHook" {
            script = Some(value.clone());
        }
        env.set_var(&name, &value, true);
    }

    // **Before the hook**, which is written expecting them: a `shellHook` that calls `runHook` or
    // `addToSearchPath` is ordinary, and defining the functions afterwards would make that fail for
    // no reason a reader could see.
    if want.functions {
        // The state before the code that reads it, so a function defined below cannot be called
        // against a half-built shell by anything that runs in between.
        let (scalars, arrays) = locals_from(&json)?;
        for (name, value) in &scalars {
            env.set_var(name, value, false);
        }
        for (name, values) in &arrays {
            env.set_array(
                name,
                crate::env::scope::ShellArray::from_values(values.clone()),
            );
        }
        define(env, &functions_from(&json)?)?;
    }
    // **After the variables, not before.** The hook is written expecting the shell it is entering:
    // the flake's own `$MESHTASTIC_PROTO_DIR` and the rest have to be set for it to have anything
    // to say. Run through oslo rather than bash — a `.env.lua` project has already chosen this
    // shell, and shelling out would make the hook's `$PATH` a different one from the caller's.
    if let Some(script) = script.filter(|s| !s.trim().is_empty())
        && let Err(err) =
            crate::env::builtins::builtin_eval(env, &["eval".to_string(), "--".to_string(), script])
    {
        return Err(format!("shellHook: {err}"));
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::read::{IGNORED, usable_name};
    use super::*;

    /// The name in the diagnostic has to be the one the line asked for, or the message is another
    /// guess of the kind this replaced.
    #[test]
    fn the_shell_a_line_asked_for_is_the_one_named_back() {
        let words =
            |list: &[&str]| -> Vec<String> { list.iter().map(|w| (*w).to_string()).collect() };
        assert_eq!(asked_for(&words(&[".#tooling"])), "tooling");
        assert_eq!(asked_for(&words(&["..#other"])), "other");
        // No attribute named at all is `default`, which is what nix resolves it to.
        assert_eq!(asked_for(&words(&[])), "default");
        assert_eq!(asked_for(&words(&["."])), "default");
        // `use flake --option warn-dirty false` names no shell; the flag must not be read as one.
        assert_eq!(
            asked_for(&words(&["--option", "warn-dirty false"])),
            "default"
        );
        assert_eq!(
            asked_for(&words(&["--option", "warn-dirty false", ".#dev"])),
            "default",
            "the flag's own value is not an installable either"
        );
    }

    #[test]
    fn the_builders_home_never_escapes() {
        let json = r#"{"variables":{"HOME":{"type":"exported","value":"/homeless-shelter"},
                       "CC":{"type":"exported","value":"gcc"}}}"#;
        let got = exported_from(json).expect("parsed");
        assert_eq!(got, vec![("CC".to_string(), "gcc".to_string())]);
    }

    #[test]
    fn the_whole_ignore_list_is_dropped() {
        for name in IGNORED {
            let json = format!(r#"{{"variables":{{"{name}":{{"type":"exported","value":"x"}}}}}}"#);
            assert!(
                exported_from(&json).expect("parsed").is_empty(),
                "{name} must not escape the dev shell"
            );
        }
    }

    #[test]
    fn only_exported_variables_are_taken() {
        let json = r#"{"variables":{"A":{"type":"var","value":"1"},
                       "B":{"type":"array","value":["x"]},
                       "C":{"type":"exported","value":"3"}}}"#;
        let got = exported_from(json).expect("parsed");
        assert_eq!(got, vec![("C".to_string(), "3".to_string())]);
    }

    /// The body arrives without a name and without braces. Pasting it as-is would not be a
    /// definition at all — it would be a list of commands that runs the moment it is imported.
    #[test]
    fn a_function_is_rebuilt_from_a_bare_body() {
        let json = r#"{"bashFunctions":{"runHook":"\n    local h=\"$1\";\n    return 0\n"}}"#;
        let got = functions_from(json).expect("parsed");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "runHook");
        assert!(got[0].1.contains("local h="), "the body is kept verbatim");
        assert!(!got[0].1.contains("runHook ()"), "the name is not in it");
    }

    /// The `var` and `array` entries are two thirds of `variables` and were dropped whole. They are
    /// not environment — they are stdenv's own state, and its functions are written against them:
    /// `configurePhase` holds a phase *name*, `preConfigureHooks` is the list `runHook` walks.
    #[test]
    fn the_locals_and_arrays_come_across() {
        let json = r#"{"variables":{
            "CC":{"type":"exported","value":"gcc"},
            "prefix":{"type":"var","value":"/nix/store/x"},
            "preConfigureHooks":{"type":"array","value":["a","b"]},
            "HOME":{"type":"var","value":"/homeless-shelter"}
        }}"#;
        let (scalars, arrays) = locals_from(json).expect("parsed");
        assert!(scalars.contains(&("prefix".to_string(), "/nix/store/x".to_string())));
        assert_eq!(
            arrays,
            vec![(
                "preConfigureHooks".to_string(),
                vec!["a".to_string(), "b".to_string()]
            )]
        );
        // The exported ones are the other function's business, and `IGNORED` still applies —
        // a builder's `HOME` must not arrive by this door either.
        assert!(
            scalars
                .iter()
                .all(|(name, _)| name != "CC" && name != "HOME")
        );
    }
    /// An older nix, or a shell with no functions. Nothing to define is not a failure.
    #[test]
    fn no_functions_key_is_not_an_error() {
        assert_eq!(
            functions_from(r#"{"variables":{}}"#).expect("parsed"),
            Vec::new()
        );
    }

    /// bash accepts function names oslo's parser will not. One that cannot be defined is skipped
    /// rather than allowed to fail the import of the other hundred.
    #[test]
    fn a_name_oslo_cannot_define_is_skipped() {
        for bad in ["pkg-config_hook", "a.b", "9lives", ""] {
            assert!(!usable_name(bad), "{bad:?} must be skipped");
        }
        for good in ["runHook", "_addToEnv", "ccWrapper_addCVars", "a:b"] {
            assert!(usable_name(good), "{good:?} must be kept");
        }
    }

    #[test]
    fn exported_bash_functions_are_dropped() {
        let json = r#"{"variables":{"BASH_FUNC_foo%%":{"type":"exported","value":"() { :; }"}}}"#;
        assert!(exported_from(json).expect("parsed").is_empty());
    }

    #[test]
    fn the_command_names_a_profile_and_quotes_its_arguments() {
        let got = command(&[String::from("..#dev shell")], ".direnv/flake-profile");
        assert!(got.contains("--profile '.direnv/flake-profile'"));
        assert!(got.ends_with("'..#dev shell'"));
    }

    #[test]
    fn your_own_path_survives_behind_the_dev_shells() {
        let got = keeping_yours("/nix/a:/nix/b", "/usr/bin:/nix/a:/home/me/bin");
        assert_eq!(got, "/nix/a:/nix/b:/usr/bin:/home/me/bin");
    }

    #[test]
    fn an_entry_in_both_appears_once() {
        assert_eq!(keeping_yours("/x", "/x"), "/x");
    }

    #[test]
    fn output_that_is_not_print_dev_env_is_refused_by_name() {
        let problem = exported_from(r#"{"nope":1}"#).expect_err("refused");
        assert!(problem.contains("print-dev-env"), "{problem}");
    }

    /// `shellHook` is exported like anything else, so it lands in the environment either way. What
    /// the flag decides is whether it is also *run*.
    #[test]
    fn the_hook_is_a_variable_before_it_is_a_script() {
        let json = r#"{"variables":{"shellHook":{"type":"exported","value":"echo hi"},
                       "CC":{"type":"exported","value":"gcc"}}}"#;
        let got = exported_from(json).expect("parsed");
        assert!(
            got.contains(&("shellHook".to_string(), "echo hi".to_string())),
            "the hook is still imported as a variable: {got:?}"
        );
    }
}

#[cfg(test)]
mod command_tests {
    use super::command;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    /// `use flake --option warn-dirty false` is a real line in a real `.envrc`, and every word
    /// after `use flake` belongs to `nix`. Reading the first one as the installable sent nix a
    /// lone `--option`, which it rejects for wanting two arguments it was never given.
    #[test]
    fn every_argument_reaches_nix_in_order() {
        let built = command(
            &args(&["--option", "warn-dirty", "false"]),
            "/p/flake-profile",
        );
        assert!(
            built.ends_with("'--option' 'warn-dirty' 'false'"),
            "{built}"
        );
        assert!(built.contains("--profile '/p/flake-profile'"), "{built}");
    }

    /// A bare `use flake` names nothing, and `print-dev-env` resolves this directory itself —
    /// which is what direnv relies on, its `"$@"` being empty in exactly this case.
    #[test]
    fn no_arguments_means_no_installable_rather_than_a_dot() {
        let built = command(&[], "/p/flake-profile");
        assert!(built.ends_with("--profile '/p/flake-profile'"), "{built}");
    }

    /// A named installable still arrives, and quoting survives a word that would otherwise split.
    #[test]
    fn an_installable_and_an_awkward_word_are_quoted() {
        let built = command(&args(&[".#other", "a b"]), "/p");
        assert!(built.ends_with("'.#other' 'a b'"), "{built}");
    }

    /// **`--impure` goes in front of the installable, not after it.** `asked_for` reads the first
    /// word that is not a flag as the dev shell's name, and nix resolves it the same way — so a
    /// flag appended at the end of `nix_develop{ flake = "..#other", impure = true }` would be
    /// fine, while one put anywhere before it must still not be mistaken for the installable.
    #[test]
    fn impure_is_a_flag_and_the_installable_is_still_the_first_word() {
        let built = command(&args(&["--impure", ".#other"]), "/p");
        assert!(built.ends_with("'--impure' '.#other'"), "{built}");
        assert_eq!(super::asked_for(&args(&["--impure", ".#other"])), "other");
    }
}
