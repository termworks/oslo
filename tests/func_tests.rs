//! `--func`: a pure Lua script, run from its file the way a user runs one.

mod common;

use std::process::Command;

/// Write `files` into a fresh directory and run the first one with `args`.
fn run_pure(files: &[(&str, &str)], args: &[&str]) -> (String, String, i32) {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).expect("write");
    }
    let out = Command::new(common::oslo_bin())
        .arg(dir.path().join(files[0].0))
        .args(args)
        .output()
        .expect("spawn oslo");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn a_pure_script_prints_what_it_returns() {
    let (out, err, status) = run_pure(
        &[
            (
                "config",
                "#!/usr/bin/env -S oslo --func\n\
                 local name = ...\n\
                 return { name = name, region = import('shared').region, ports = { 80, 443 } }\n",
            ),
            ("shared.lua", "return { region = 'eu' }\n"),
        ],
        &["web"],
    );
    assert_eq!(status, 0, "stderr: {err}");
    assert_eq!(
        out,
        "{\"name\":\"web\",\"ports\":[80,443],\"region\":\"eu\"}\n"
    );
}

#[test]
fn an_impure_script_does_not_run() {
    let (out, err, status) = run_pure(
        &[(
            "counter",
            "#!/usr/bin/env -S oslo --func\n\
             local n = 0\n\
             local function bump() n = n + 1 end\n\
             return n\n",
        )],
        &[],
    );
    assert_eq!(status, 1);
    assert!(out.is_empty(), "it printed: {out}");
    assert!(
        err.contains("captured variable"),
        "the wrong refusal: {err}"
    );
}

#[test]
fn a_pure_script_has_no_oslo_api() {
    let (out, _, status) = run_pure(
        &[(
            "probe",
            "#!/usr/bin/env -S oslo --func\nreturn oslo == nil and io == nil\n",
        )],
        &[],
    );
    assert_eq!((out.as_str(), status), ("true\n", 0));
}
