//! Every `shopt` name bash 5.3 has, its default, and where its state lives.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Where an option's state is kept and who acts on it.
pub(super) enum Support {
    /// Switchable: the hook pushes the new state into the module that acts on it.
    Hook(fn(bool)),
    /// Switchable: the state lives here, and readers ask [`on`].
    Stored,
    /// Reflects how the shell was started; `shopt -s`/`-u` change nothing.
    ReadOnly,
    /// One of the `compatNN` names, which share one compatibility level.
    Compat(u32),
}

pub(super) struct ShoptOption {
    pub name: &'static str,
    pub support: Support,
    pub default: bool,
}

const fn hook(name: &'static str, apply: fn(bool)) -> ShoptOption {
    ShoptOption {
        name,
        support: Support::Hook(apply),
        default: false,
    }
}

/// A hook whose option starts on; the module it drives starts in the matching state.
const fn hook_on(name: &'static str, apply: fn(bool)) -> ShoptOption {
    ShoptOption {
        name,
        support: Support::Hook(apply),
        default: true,
    }
}

const fn stored(name: &'static str, default: bool) -> ShoptOption {
    ShoptOption {
        name,
        support: Support::Stored,
        default,
    }
}

const fn read_only(name: &'static str) -> ShoptOption {
    ShoptOption {
        name,
        support: Support::ReadOnly,
        default: false,
    }
}

const fn compat(name: &'static str, level: u32) -> ShoptOption {
    ShoptOption {
        name,
        support: Support::Compat(level),
        default: false,
    }
}

/// The options, in the alphabetical order bash lists them in, with bash's non-interactive defaults.
#[rustfmt::skip]
pub(super) const OPTIONS: &[ShoptOption] = &[
    stored("array_expand_once", false),
    stored("assoc_expand_once", false),
    hook("autocd", crate::exec::simple::set_autocd),
    stored("bash_source_fullpath", false),
    stored("cdable_vars", false),
    stored("cdspell", false),
    stored("checkhash", false),
    stored("checkjobs", false),
    stored("checkwinsize", true),
    stored("cmdhist", true),
    compat("compat31", 31),
    compat("compat32", 32),
    compat("compat40", 40),
    compat("compat41", 41),
    compat("compat42", 42),
    compat("compat43", 43),
    compat("compat44", 44),
    stored("complete_fullquote", true),
    stored("direxpand", false),
    stored("dirspell", false),
    hook("dotglob", crate::expand::glob::set_dotglob),
    stored("execfail", false),
    stored("expand_aliases", false),
    stored("extdebug", false),
    hook("extglob", crate::expand::glob::set_extglob),
    stored("extquote", true),
    hook("failglob", crate::expand::glob::set_failglob),
    stored("force_fignore", true),
    stored("globasciiranges", true),
    hook_on("globskipdots", crate::expand::glob::set_globskipdots),
    hook("globstar", crate::expand::glob::set_globstar),
    stored("gnu_errfmt", false),
    stored("histappend", false),
    stored("histreedit", false),
    stored("histverify", false),
    stored("hostcomplete", true),
    stored("huponexit", false),
    stored("inherit_errexit", false),
    stored("interactive_comments", true),
    stored("lastpipe", false),
    stored("lithist", false),
    stored("localvar_inherit", false),
    stored("localvar_unset", false),
    read_only("login_shell"),
    stored("mailwarn", false),
    stored("no_empty_cmd_completion", false),
    hook("nocaseglob", crate::expand::glob::set_nocaseglob),
    hook("nocasematch", crate::expand::glob::set_nocasematch),
    stored("noexpand_translation", false),
    hook("nullglob", crate::expand::glob::set_nullglob),
    stored("patsub_replacement", true),
    stored("progcomp", true),
    stored("progcomp_alias", false),
    stored("promptvars", true),
    read_only("restricted_shell"),
    stored("shift_verbose", false),
    stored("sourcepath", true),
    stored("varredir_close", false),
    stored("xpg_echo", false),
];

/// The compatibility level a `compatNN` name selects when nothing has asked for one.
pub(super) const CURRENT_LEVEL: u32 = 53;

const fn defaults() -> u64 {
    let mut bits = 0u64;
    let mut i = 0;
    while i < OPTIONS.len() {
        if OPTIONS[i].default {
            bits |= 1 << i;
        }
        i += 1;
    }
    bits
}

/// Bit `n` is `OPTIONS[n]`, for the `Hook`, `Stored` and `ReadOnly` entries.
///
/// Process-global, like the glob settings the hooks drive: a forked subshell inherits them.
static ENABLED: AtomicU64 = AtomicU64::new(defaults());

/// The level the `compatNN` names report; [`CURRENT_LEVEL`] when none is selected.
static LEVEL: AtomicU32 = AtomicU32::new(CURRENT_LEVEL);

pub(super) fn index_of(name: &str) -> Option<usize> {
    OPTIONS.iter().position(|o| o.name == name)
}

pub(super) fn state_of(index: usize) -> bool {
    match OPTIONS[index].support {
        Support::Compat(level) => LEVEL.load(Ordering::Relaxed) == level,
        _ => ENABLED.load(Ordering::Relaxed) & (1 << index) != 0,
    }
}

pub(super) fn record(index: usize, on: bool) {
    let bit = 1u64 << index;
    if on {
        ENABLED.fetch_or(bit, Ordering::Relaxed);
    } else {
        ENABLED.fetch_and(!bit, Ordering::Relaxed);
    }
}

/// Select or clear a compatibility level, returning the level now in force.
pub(super) fn set_level(level: u32, on: bool) -> u32 {
    if on {
        LEVEL.store(level, Ordering::Relaxed);
    } else {
        let _ = LEVEL.compare_exchange(level, CURRENT_LEVEL, Ordering::Relaxed, Ordering::Relaxed);
    }
    LEVEL.load(Ordering::Relaxed)
}

/// Follow an assignment to `BASH_COMPAT`: `42` and `4.2` both select `compat42`, and anything
/// that names no `compatNN` selects the current level.
pub fn note_compat(value: &str) {
    let digits: String = value.chars().filter(|c| *c != '.').collect();
    let level = digits
        .parse::<u32>()
        .ok()
        .filter(|level| {
            OPTIONS
                .iter()
                .any(|o| matches!(o.support, Support::Compat(l) if l == *level))
        })
        .unwrap_or(CURRENT_LEVEL);
    LEVEL.store(level, Ordering::Relaxed);
}

/// Whether the option called `name` is on. False for a name bash does not have.
pub fn on(name: &str) -> bool {
    index_of(name).is_some_and(state_of)
}

/// Force a `ReadOnly` or stored option to `on`, for the shell's own startup.
pub fn force(name: &str, on: bool) {
    if let Some(index) = index_of(name) {
        if let Support::Hook(apply) = OPTIONS[index].support {
            apply(on);
        }
        record(index, on);
    }
}
