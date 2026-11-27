//! The conditions the shell fires itself rather than the kernel: DEBUG, ERR and RETURN.
//!
//! Each runs its handler through [`super::handlers::run_handler`], so `$?` survives it, and each
//! has a guard so the handler's own commands do not fire it again.

use super::handlers::{
    Disposition, TRAPSIG_DEBUG, TRAPSIG_ERR, TRAPSIG_RETURN, disposition, run_handler,
};
use crate::env::Environment;
use crate::env::origin_now;
use oslo_base::error::Result;
use std::cell::Cell;

thread_local! {
    /// How many trap handlers are running, outermost included.
    static TRAP_DEPTH: Cell<u32> = const { Cell::new(0) };
    static IN_DEBUG: Cell<bool> = const { Cell::new(false) };
    static IN_ERR: Cell<bool> = const { Cell::new(false) };
    static IN_RETURN: Cell<bool> = const { Cell::new(false) };
    /// Set in a pipeline stage whose DEBUG trap the parent already ran before forking it.
    static SKIP_DEBUG: Cell<bool> = const { Cell::new(false) };
}

/// A trap handler running; dropping it ends the extent [`in_trap`] reports.
pub(super) struct TrapRunning;

impl TrapRunning {
    pub(super) fn enter() -> Self {
        TRAP_DEPTH.with(|d| d.set(d.get() + 1));
        TrapRunning
    }
}

impl Drop for TrapRunning {
    fn drop(&mut self) {
        TRAP_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Whether a trap handler is running, which freezes `$BASH_COMMAND`.
pub fn in_trap() -> bool {
    TRAP_DEPTH.with(|d| d.get() > 0)
}

/// Whether a DEBUG handler is running.
pub fn in_debug_trap() -> bool {
    IN_DEBUG.with(Cell::get)
}

/// Skip the next DEBUG trap in this process: the parent already ran it for this stage.
pub fn skip_next_debug_trap() {
    SKIP_DEBUG.with(|s| s.set(true));
}

/// Run the DEBUG trap, before the command whose text is in `$BASH_COMMAND`.
///
/// It cannot change `$?`, it does not fire for its own commands, and a failing handler is
/// reported rather than failing the command.
pub fn run_debug_trap(env: &mut Environment) {
    if SKIP_DEBUG.with(|s| s.replace(false)) || in_debug_trap() {
        return;
    }
    let Disposition::Run(text) = disposition(env, "DEBUG") else {
        return;
    };
    let action = text.to_string();
    IN_DEBUG.with(|d| d.set(true));
    let outcome = run_handler(env, &action, TRAPSIG_DEBUG);
    IN_DEBUG.with(|d| d.set(false));
    if let Err(e) = outcome {
        eprintln!("{}trap: DEBUG: {e}", origin_now());
    }
}

/// Run the ERR trap, for a command that has just failed. An `exit` inside it propagates.
pub fn run_err_trap(env: &mut Environment) -> Result<()> {
    if IN_ERR.with(Cell::get) {
        return Ok(());
    }
    let Disposition::Run(text) = disposition(env, "ERR") else {
        return Ok(());
    };
    let action = text.to_string();
    IN_ERR.with(|d| d.set(true));
    let outcome = run_handler(env, &action, TRAPSIG_ERR);
    IN_ERR.with(|d| d.set(false));
    outcome.map(|_| ())
}

/// Run the RETURN trap as a function or sourced file finishes. `$?` inside it is the status of
/// the last command the body ran, which a `return N` does not change.
pub fn run_return_trap(env: &mut Environment) -> Result<()> {
    if IN_RETURN.with(Cell::get) {
        return Ok(());
    }
    let Disposition::Run(text) = disposition(env, "RETURN") else {
        return Ok(());
    };
    let action = text.to_string();
    IN_RETURN.with(|d| d.set(true));
    let outcome = run_handler(env, &action, TRAPSIG_RETURN);
    IN_RETURN.with(|d| d.set(false));
    outcome.map(|_| ())
}
