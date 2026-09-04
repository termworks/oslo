//! Exact programs hosted by the existing Scratch keeper.

use super::keeper::{self, Role};
use crate::watch::{Policy, WatchSpec};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;

pub const WORKER_ENV: &str = "OSLO_WATCH_WORKER";
pub const BOOTSTRAP_ENV: &str = "OSLO_WATCH_SCRATCH_BOOTSTRAP";

pub fn start_watch(name: &str, spec: &WatchSpec) -> io::Result<()> {
    let token = token()?;
    let status = std::process::Command::new("/proc/self/exe")
        .arg(format!("--__watch-scratch={token}"))
        .arg(name)
        .args(launcher_arguments(spec))
        .env(BOOTSTRAP_ENV, &token)
        .env_remove(WORKER_ENV)
        .current_dir(&spec.root)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "scratch bootstrap exited {}",
            status.code().unwrap_or(1)
        )))
    }
}

pub fn host_watch(name: &str, spec: &WatchSpec) -> io::Result<()> {
    let token = token()?;
    let executable = CString::new("/proc/self/exe").expect("static path");
    let arguments = arguments(spec, &token)?;
    // SAFETY: the bootstrap calls this before Oslo starts any threads.
    unsafe { std::env::set_var(WORKER_ENV, &token) };
    close_inherited_descriptors();
    match keeper::spawn(name, oslo_ui::settings::current().scratch.log_bytes)? {
        Role::Caller(_) => wait_ready(name),
        Role::Inside => {
            close_inherited_descriptors();
            let _ = nix::unistd::execv(&executable, &arguments);
            // SAFETY: the worker child must not return to the forked process.
            unsafe { nix::libc::_exit(127) }
        }
    }
}

fn close_inherited_descriptors() {
    // SAFETY: the Scratch worker needs only its standard descriptors after `keeper::spawn`.
    let closed = unsafe { nix::libc::syscall(nix::libc::SYS_close_range, 3u32, u32::MAX, 0u32) };
    if closed == -1 {
        for fd in 3..1024 {
            let _ = nix::unistd::close(fd);
        }
    }
}

pub fn verify_bootstrap(token: &str) -> bool {
    token.len() == 32 && std::env::var(BOOTSTRAP_ENV).as_deref() == Ok(token)
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
    match session {
        None => {
            logical.truncate(49);
            format!("watch-{logical}-{}", &digest[..8])
        }
        Some(session) => {
            logical.truncate(40);
            let session: String = session
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(8)
                .collect();
            let session = if session.is_empty() {
                "session".to_string()
            } else {
                session
            };
            format!("watch-{logical}-{}-{session}", &digest[..8])
        }
    }
}

fn arguments(spec: &WatchSpec, token: &str) -> io::Result<Vec<CString>> {
    let mut args = vec![
        "/proc/self/exe".to_string(),
        "watch".to_string(),
        format!("--__worker={token}"),
    ];
    args.extend(launcher_arguments(spec));
    args.into_iter()
        .map(|arg| CString::new(arg).map_err(|_| io::Error::other("watch argument contains NUL")))
        .collect()
}

fn launcher_arguments(spec: &WatchSpec) -> Vec<String> {
    let mut args = vec![
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
    args
}

fn wait_ready(name: &str) -> io::Result<()> {
    let paths = super::store::Paths::new(name);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if paths.sock().exists() && super::store::alive(name) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Err(io::Error::other(format!(
        "{name}: scratch keeper did not become ready"
    )))
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

    #[test]
    fn generated_names_fit_the_scratch_limit() {
        let item = spec(&"service".repeat(20), "src/**");
        assert!(generated_name(&item, None).len() <= 64);
        assert!(generated_name(&item, Some("0123456789abcdef")).len() <= 64);
    }
}
