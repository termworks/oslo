//! Managed command watchers exposed through `oslo.watch.start`.

#[cfg(feature = "direnv")]
use super::util::native;
use super::util::{ok, put};
use oslo_base::value::{LuaError, Table, Value};
use oslo_shell::watch::{Policy, WatchSpec, normalize};
use std::cell::RefCell;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

thread_local! {
    static SCOPED: RefCell<Vec<Arc<Mutex<Service>>>> = const { RefCell::new(Vec::new()) };
}

enum Target {
    Process(Child),
    #[cfg(feature = "scratch")]
    Scratch(String),
}

struct Service {
    name: String,
    target: Option<Target>,
}

#[derive(Debug)]
struct Parsed {
    spec: WatchSpec,
    scratch: ScratchRequest,
    persist: bool,
}

#[derive(Debug)]
enum ScratchRequest {
    Auto,
    Never,
    Required(Option<String>),
}

pub fn build() -> Value {
    let mut watch = Table::new();
    put(&mut watch, "start", |_, args| {
        let value = args
            .first()
            .ok_or_else(|| LuaError::new("oslo.watch.start: expected a table".to_string()))?;
        let root = std::env::current_dir()
            .map_err(|error| LuaError::new(format!("oslo.watch.start: {error}")))?;
        let started = start(value, &root, true, None)?;
        ok(started.handle)
    });
    Value::table(watch)
}

pub(crate) struct Started {
    pub handle: Value,
    #[cfg(feature = "direnv")]
    pub cleanup: Option<Value>,
}

#[cfg(feature = "direnv")]
pub(crate) fn start_directory(value: &Value, root: &Path) -> Result<Started, LuaError> {
    let session = std::env::var("OSLO_SESSION").ok();
    start(value, root, false, session.as_deref())
}

fn start(
    value: &Value,
    root: &Path,
    initial_default: bool,
    session: Option<&str>,
) -> Result<Started, LuaError> {
    let parsed = parse(value, root, initial_default)?;
    parsed
        .spec
        .validate()
        .map_err(|error| LuaError::new(format!("oslo.watch.start: {error}")))?;
    let target = launch(&parsed, session)
        .map_err(|error| LuaError::new(format!("oslo.watch.start: {error}")))?;
    let service = Arc::new(Mutex::new(Service {
        name: parsed.spec.name.clone(),
        target: Some(target),
    }));
    if !parsed.persist {
        SCOPED.with(|slot| slot.borrow_mut().push(Arc::clone(&service)));
    }
    #[cfg(feature = "direnv")]
    let cleanup = (!parsed.persist).then(|| cleanup(Arc::clone(&service)));
    Ok(Started {
        handle: handle(Arc::clone(&service)),
        #[cfg(feature = "direnv")]
        cleanup,
    })
}

fn parse(value: &Value, root: &Path, initial_default: bool) -> Result<Parsed, LuaError> {
    let Value::Table(spec) = value else {
        return Err(LuaError::new(
            "oslo.watch.start: expected a table".to_string(),
        ));
    };
    let spec = spec.borrow();
    let paths = strings(&spec.get_str("paths"), "paths")?;
    let argv = strings(&spec.get_str("run"), "run")?;
    let name = optional_string(&spec.get_str("name"), "name")?.unwrap_or_else(|| {
        argv.first()
            .and_then(|program| Path::new(program).file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("command")
            .to_string()
    });
    let initial = optional_bool(&spec.get_str("initial"), "initial")?.unwrap_or(initial_default);
    let persist = optional_bool(&spec.get_str("persist"), "persist")?.unwrap_or(false);
    let debounce = milliseconds(&spec.get_str("debounce_ms"), "debounce_ms", 100)?;
    let grace = milliseconds(&spec.get_str("grace_ms"), "grace_ms", 1000)?;
    let policy = match optional_string(&spec.get_str("policy"), "policy")?.as_deref() {
        None | Some("coalesce") => Policy::Coalesce,
        Some("restart") => Policy::Restart,
        Some(other) => {
            return Err(LuaError::new(format!(
                "oslo.watch.start: policy must be \"coalesce\" or \"restart\", got {other:?}"
            )));
        }
    };
    let scratch = match spec.get_str("scratch") {
        Value::Nil => ScratchRequest::Auto,
        Value::Bool(false) => ScratchRequest::Never,
        Value::Bool(true) => ScratchRequest::Required(None),
        Value::Str(name) if name.as_ref() == "auto" => ScratchRequest::Auto,
        Value::Str(name) => ScratchRequest::Required(Some(name.to_string())),
        other => {
            return Err(LuaError::new(format!(
                "oslo.watch.start: scratch must be \"auto\", false, true, or a name, got {}",
                other.type_name()
            )));
        }
    };
    let patterns = paths
        .iter()
        .map(|path| normalize(root, path).display().to_string())
        .collect();
    Ok(Parsed {
        spec: WatchSpec {
            name,
            root: root.to_path_buf(),
            patterns,
            argv,
            initial,
            debounce,
            policy,
            grace,
        },
        scratch,
        persist,
    })
}

fn strings(value: &Value, field: &str) -> Result<Vec<String>, LuaError> {
    let Value::Table(items) = value else {
        return Err(LuaError::new(format!(
            "oslo.watch.start: {field} must be a list of strings"
        )));
    };
    items
        .borrow()
        .sequence()
        .iter()
        .enumerate()
        .map(|(index, value)| match value {
            Value::Str(text) => Ok(text.to_string()),
            other => Err(LuaError::new(format!(
                "oslo.watch.start: {field}[{}] must be a string, got {}",
                index + 1,
                other.type_name()
            ))),
        })
        .collect()
}

fn optional_string(value: &Value, field: &str) -> Result<Option<String>, LuaError> {
    match value {
        Value::Nil => Ok(None),
        Value::Str(text) => Ok(Some(text.to_string())),
        other => Err(LuaError::new(format!(
            "oslo.watch.start: {field} must be a string, got {}",
            other.type_name()
        ))),
    }
}

fn optional_bool(value: &Value, field: &str) -> Result<Option<bool>, LuaError> {
    match value {
        Value::Nil => Ok(None),
        Value::Bool(value) => Ok(Some(*value)),
        other => Err(LuaError::new(format!(
            "oslo.watch.start: {field} must be a boolean, got {}",
            other.type_name()
        ))),
    }
}

fn milliseconds(value: &Value, field: &str, default: u64) -> Result<Duration, LuaError> {
    match value {
        Value::Nil => Ok(Duration::from_millis(default)),
        value => value
            .as_number()
            .and_then(|number| number.as_int())
            .filter(|number| *number >= 0)
            .map(|number| Duration::from_millis(number as u64))
            .ok_or_else(|| {
                LuaError::new(format!(
                    "oslo.watch.start: {field} must be a non-negative integer"
                ))
            }),
    }
}

fn launch(parsed: &Parsed, _session: Option<&str>) -> std::io::Result<Target> {
    match &parsed.scratch {
        ScratchRequest::Never => process(&parsed.spec),
        #[cfg(feature = "scratch")]
        ScratchRequest::Auto | ScratchRequest::Required(_) => {
            let name = match &parsed.scratch {
                ScratchRequest::Required(Some(name)) => {
                    if !oslo_shell::scratch::name::valid(name) {
                        return Err(std::io::Error::other(format!(
                            "{name:?} is not a usable scratch name"
                        )));
                    }
                    name.clone()
                }
                _ => oslo_shell::scratch::program::generated_name(&parsed.spec, _session),
            };
            oslo_shell::scratch::program::start_watch(&name, &parsed.spec)?;
            Ok(Target::Scratch(name))
        }
        #[cfg(not(feature = "scratch"))]
        ScratchRequest::Auto => process(&parsed.spec),
        #[cfg(not(feature = "scratch"))]
        ScratchRequest::Required(name) => {
            let _ = name;
            Err(std::io::Error::other(
                "this build does not support Scratch-backed watch services",
            ))
        }
    }
}

fn process(spec: &WatchSpec) -> std::io::Result<Target> {
    worker_command(spec, "--foreground")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map(Target::Process)
}

fn worker_command(spec: &WatchSpec, launch: &str) -> Command {
    let mut command = Command::new("/proc/self/exe");
    command
        .arg("watch")
        .arg(launch)
        .arg("--name")
        .arg(&spec.name)
        .arg(if spec.initial {
            "--initial"
        } else {
            "--postpone"
        })
        .arg(format!("--debounce={}", spec.debounce.as_millis()))
        .arg(format!("--grace={}", spec.grace.as_millis()))
        .arg(match spec.policy {
            Policy::Coalesce => "--coalesce",
            Policy::Restart => "--restart",
        })
        .args(&spec.patterns)
        .arg("--")
        .args(&spec.argv)
        .current_dir(&spec.root);
    command
}

fn handle(service: Arc<Mutex<Service>>) -> Value {
    let mut handle = super::handle::Handle::new("oslo.watch.service");
    let named = Arc::clone(&service);
    handle.verb("name", move |_, _| {
        ok(Value::str(
            named
                .lock()
                .map(|held| held.name.clone())
                .unwrap_or_default(),
        ))
    });
    let mode = Arc::clone(&service);
    handle.verb("mode", move |_, _| {
        let name = mode
            .lock()
            .ok()
            .and_then(|held| match held.target.as_ref() {
                Some(Target::Process(_)) => Some("process"),
                #[cfg(feature = "scratch")]
                Some(Target::Scratch(_)) => Some("scratch"),
                None => None,
            })
            .unwrap_or("stopped");
        ok(Value::str(name))
    });
    let stopped = Arc::clone(&service);
    handle.verb("stop", move |_, _| {
        ok(Value::Bool(stop(&stopped).unwrap_or(false)))
    });
    handle.build()
}

#[cfg(feature = "direnv")]
fn cleanup(service: Arc<Mutex<Service>>) -> Value {
    native("oslo.watch.service.stop", move |_, _| {
        let _ = stop(&service);
        ok(Value::Nil)
    })
}

fn stop(service: &Arc<Mutex<Service>>) -> std::io::Result<bool> {
    let target = service
        .lock()
        .map_err(|_| std::io::Error::other("watch service lock is poisoned"))?
        .target
        .take();
    let Some(target) = target else {
        return Ok(false);
    };
    match target {
        Target::Process(mut child) => {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(child.id() as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if child.try_wait()?.is_some() {
                    return Ok(true);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        #[cfg(feature = "scratch")]
        Target::Scratch(name) => oslo_shell::scratch::store::kill(&name)?,
    }
    Ok(true)
}

pub(crate) fn stop_all() {
    let services = SCOPED.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
    for service in services.into_iter().rev() {
        let _ = stop(&service);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Value {
        let mut table = Table::new();
        for (index, item) in items.iter().enumerate() {
            table.set(Value::int(index as i64 + 1), Value::str(*item));
        }
        Value::table(table)
    }

    #[test]
    fn parsing_normalizes_paths_and_preserves_argv() {
        let mut table = Table::new();
        table.set_str("paths", list(&["src/**/*.rs", "../Cargo.toml"]));
        table.set_str("run", list(&["sh", "-c", "echo $PWD"]));
        table.set_str("initial", Value::Bool(false));
        let parsed = parse(&Value::table(table), Path::new("/work/app"), true).expect("parse");
        assert_eq!(
            parsed.spec.patterns,
            ["/work/app/src/**/*.rs", "/work/Cargo.toml"]
        );
        assert_eq!(parsed.spec.argv, ["sh", "-c", "echo $PWD"]);
        assert!(!parsed.spec.initial);
    }

    #[test]
    fn invalid_policy_is_refused_before_launch() {
        let mut table = Table::new();
        table.set_str("paths", list(&["src"]));
        table.set_str("run", list(&["true"]));
        table.set_str("policy", Value::str("replace"));
        let error = parse(&Value::table(table), Path::new("/work"), true).expect_err("policy");
        assert!(error.to_string().contains("coalesce"), "{error}");
    }
}
