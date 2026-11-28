//! `{name}>file`, `{name}<file`, `{name}<>file`, `{name}>&N` and `{name}>&-`.
//!
//! The shell opens the target on the lowest free descriptor from 10 up and stores the number in
//! `name`. The descriptor is not restored when the command ends: it stays open until the script
//! closes it with `{name}>&-`, which reads the number back out of `name`.

use super::{SAVE_FD_FLOOR, bad_descriptor, moved_descriptor, open_target};
use crate::env::Environment;
use nix::fcntl::{FcntlArg, fcntl};
use oslo_base::ast::{RedirectKind, Redirection};
use oslo_base::error::{Result, ShellError};
use std::os::fd::{AsRawFd, RawFd};

pub(super) fn apply(
    env: &mut Environment,
    redir: &Redirection,
    name: &str,
    target: &str,
) -> Result<()> {
    let duplicating = matches!(redir.kind, RedirectKind::DupInput | RedirectKind::DupOutput);
    if duplicating && target == "-" {
        let fd = read(env, name).ok_or_else(|| ambiguous(name))?;
        let _ = nix::unistd::close(fd);
        return Ok(());
    }

    let fd = match open_target(env, redir, target)? {
        Some(file) => high_copy(file.as_raw_fd(), target)?,
        None => {
            let (source, moving) = match moved_descriptor(target) {
                Some(source) => (source, true),
                None => (target.parse().map_err(|_| ambiguous(target))?, false),
            };
            let fd = high_copy(source, &source.to_string())?;
            if moving {
                let _ = nix::unistd::close(source);
            }
            fd
        }
    };
    if !assign(env, name, fd) {
        let _ = nix::unistd::close(fd);
        return Err(ShellError::ExecutionError(format!(
            "{name}: cannot assign fd to variable"
        )));
    }
    Ok(())
}

/// A copy of `fd` on the lowest free descriptor from 10 up, inherited by children.
fn high_copy(fd: RawFd, named: &str) -> Result<RawFd> {
    fcntl(fd, FcntlArg::F_DUPFD(SAVE_FD_FLOOR)).map_err(|_| bad_descriptor(named))
}

fn ambiguous(name: &str) -> ShellError {
    ShellError::ExecutionError(format!("{name}: ambiguous redirect"))
}

/// `name` or `name[sub]` split into the variable and its evaluated subscript.
fn split(env: &mut Environment, name: &str) -> Result<(String, Option<i64>)> {
    match name.split_once('[') {
        Some((base, rest)) => {
            let subscript = rest.strip_suffix(']').unwrap_or(rest);
            let index = crate::expand::arithmetic::eval_arithmetic(env, subscript)?;
            Ok((base.to_string(), Some(index)))
        }
        None => Ok((name.to_string(), None)),
    }
}

/// The descriptor number stored in `name`, when it holds one.
fn read(env: &mut Environment, name: &str) -> Option<RawFd> {
    let (base, index) = split(env, name).ok()?;
    let value = match index {
        Some(index) => env.get_array(&base)?.get(index)?.to_string(),
        None => env.get_var(&base)?.to_string(),
    };
    value.trim().parse().ok().filter(|fd: &RawFd| *fd >= 0)
}

fn assign(env: &mut Environment, name: &str, fd: RawFd) -> bool {
    match split(env, name) {
        Ok((base, Some(index))) => env.set_array_element(&base, index, &fd.to_string()),
        Ok((base, None)) => env.set_var(&base, &fd.to_string(), false),
        Err(_) => false,
    }
}
