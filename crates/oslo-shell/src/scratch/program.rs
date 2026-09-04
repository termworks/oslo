//! Exact programs hosted by the existing Scratch keeper.

use super::keeper::{self, Role};
use crate::watch::{Policy, WatchSpec};
use nix::unistd::Pid;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;

pub const WORKER_ENV: &str = "OSLO_WATCH_WORKER";

pub fn start_watch(name: &str, spec: &WatchSpec) -> io::Result<Pid> {
    let token = token()?;
    let executable = CString::new("/proc/self/exe").expect("static path");
    let arguments = arguments(spec, &token)?;
    let environment = environment(&token)?;
    let argument_pointers = pointers(&arguments);
    let environment_pointers = pointers(&environment);
    match keeper::spawn(name, oslo_ui::settings::current().scratch.log_bytes)? {
        Role::Caller(keeper) => Ok(keeper),
        Role::Inside => {
            // SAFETY: every pointer names a CString prepared before the fork and lives through
            // this call. The child exits directly if execve fails.
            unsafe {
                nix::libc::execve(
                    executable.as_ptr(),
                    argument_pointers.as_ptr(),
                    environment_pointers.as_ptr(),
                );
                nix::libc::_exit(127);
            }
        }
    }
}

pub fn generated_name(spec: &WatchSpec, session: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(spec.root.as_os_str().as_bytes());
    for pattern in &spec.patterns {
        hash.update([0]);
        hash.update(pattern.as_bytes());
    }
    hash.update([u8::from(spec.policy == Policy::Restart)]);
    for arg in &spec.argv {
        hash.update([0]);
        hash.update(arg.as_bytes());
    }
    if let Some(session) = session {
        hash.update([0]);
        hash.update(session.as_bytes());
    }
    let digest = format!("{:x}", hash.finalize());
    let mut logical: String = spec
        .name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    logical = logical.trim_matches('-').to_string();
    if logical.is_empty() {
        logical.push_str("command");
    }
    logical.truncate(49);
    format!("watch-{logical}-{}", &digest[..8])
}

fn arguments(spec: &WatchSpec, token: &str) -> io::Result<Vec<CString>> {
    let mut args = vec![
        "/proc/self/exe".to_string(),
        "watch".to_string(),
        format!("--__worker={token}"),
        "--foreground".to_string(),
        "--name".to_string(),
        spec.name.clone(),
        if spec.initial {
            "--initial".to_string()
        } else {
            "--postpone".to_string()
        },
        format!("--debounce={}", spec.debounce.as_millis()),
        format!("--grace={}", spec.grace.as_millis()),
        match spec.policy {
            Policy::Coalesce => "--coalesce".to_string(),
            Policy::Restart => "--restart".to_string(),
        },
    ];
    args.extend(spec.patterns.iter().cloned());
    args.push("--".to_string());
    args.extend(spec.argv.iter().cloned());
    args.into_iter()
        .map(|arg| CString::new(arg).map_err(|_| io::Error::other("watch argument contains NUL")))
        .collect()
}

fn environment(token: &str) -> io::Result<Vec<CString>> {
    let mut out = Vec::new();
    for (name, value) in std::env::vars_os() {
        if name == WORKER_ENV {
            continue;
        }
        let mut pair = name.as_os_str().as_bytes().to_vec();
        pair.push(b'=');
        pair.extend_from_slice(value.as_os_str().as_bytes());
        out.push(
            CString::new(pair).map_err(|_| io::Error::other("environment value contains NUL"))?,
        );
    }
    out.push(CString::new(format!("{WORKER_ENV}={token}")).expect("hex token"));
    Ok(out)
}

fn pointers(values: &[CString]) -> Vec<*const nix::libc::c_char> {
    values
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect()
}

fn token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut out = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn spec(name: &str, pattern: &str) -> WatchSpec {
        WatchSpec {
            name: name.to_string(),
            root: PathBuf::from("/work"),
            patterns: vec![pattern.to_string()],
            argv: vec!["cargo".to_string(), "check".to_string()],
            initial: false,
            debounce: Duration::from_millis(100),
            policy: Policy::Coalesce,
            grace: Duration::from_secs(1),
        }
    }

    #[test]
    fn generated_names_are_stable_readable_and_distinct() {
        let first = generated_name(&spec("API server", "src/**"), None);
        assert_eq!(first, generated_name(&spec("API server", "src/**"), None));
        assert_ne!(first, generated_name(&spec("API server", "tests/**"), None));
        assert!(super::super::name::valid(&first), "{first}");
        assert!(first.starts_with("watch-API-server-"), "{first}");
    }

    #[test]
    fn session_changes_directory_service_names() {
        let item = spec("check", "src/**");
        assert_ne!(
            generated_name(&item, Some("one")),
            generated_name(&item, Some("two"))
        );
    }
}
