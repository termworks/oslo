//! Offering `sudo` for what permission kept `rm` from removing.
//!
//! Only at a prompt with a terminal, only after the walk, and only once per `rm`: the operands
//! that held a "Permission denied" are named, and a yes runs the system's `rm` on them under
//! `sudo`, which asks for its password on the same terminal. The answer starts on no, because what
//! it does cannot be undone and no trash is involved.

use super::{Mode, Options};
use std::process::Command;

/// Ask, and run `sudo rm` on `operands` if the answer is yes. `None` when nothing was run.
pub(super) fn offer(
    operands: &[impl AsRef<str>],
    options: &Options,
    mode: &Mode,
    origin: &str,
) -> Option<i32> {
    let named: Vec<String> = operands
        .iter()
        .map(|operand| format!("'{}'", operand.as_ref()))
        .collect();
    let question = format!(
        "rm: permission denied under {}. Remove with sudo? It cannot be undone.",
        named.join(", ")
    );
    if super::walk::widget(&question, "Use sudo", "Leave them") != Some(true) {
        return None;
    }

    let mut sudo = Command::new("sudo");
    sudo.arg("rm").arg("-f");
    if options.recursive || (mode.loose && !options.dir) {
        sudo.arg("-r");
    }
    sudo.arg("--");
    for operand in operands {
        sudo.arg(oslo_base::lossless::to_os(operand.as_ref()));
    }
    match sudo.status() {
        Ok(status) => Some(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("{origin}rm: sudo: {}", oslo_base::error::reason(&e));
            Some(1)
        }
    }
}
