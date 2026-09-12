//! `oslo.secret` through the real binary: the store from Lua, and what a plugin may reach.
#![cfg(all(feature = "secrets", feature = "plugin"))]

mod common;

use common::oslo_bin;
use std::process::Command;

/// Run `script` as Lua, against a home of its own.
fn lua(home: &std::path::Path, script: &str) -> (String, String) {
    let file = home.join("script.lua");
    std::fs::write(&file, script).expect("write");
    let out = Command::new(oslo_bin())
        .arg(&file)
        .env("XDG_DATA_HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("OSLO_SECRET_IDENTITY")
        .env_remove("OSLO_SECRET_STORE")
        .output()
        .expect("spawn oslo");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The same store the command line uses, and the same values — one store, two doors.
#[test]
fn lua_and_the_command_line_see_one_store() {
    let home = tempfile::tempdir().expect("tempdir");
    let (out, err) = lua(
        home.path(),
        r#"
        oslo.secret.set("from-lua", "a value")
        print(oslo.secret.get("from-lua"))
        print(table.concat(oslo.secret.list(), ","))
        "#,
    );
    assert_eq!(out, "a value\nfrom-lua\n", "{err}");

    let out = Command::new(oslo_bin())
        .args(["secret", "get", "from-lua"])
        .env("XDG_DATA_HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join("state"))
        .output()
        .expect("spawn oslo");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a value");
}

/// `seal` and `unseal` are the store's crypto without its filing: a plugin encrypting something it
/// keeps somewhere else of its own. Base64, because an age file is binary.
#[test]
fn seal_and_unseal_round_trip_without_storing_anything() {
    let home = tempfile::tempdir().expect("tempdir");
    let (out, err) = lua(
        home.path(),
        r#"
        local sealed = oslo.secret.seal("round trip")
        print(sealed:find("age%-encryption") == nil)
        print(oslo.secret.unseal(sealed))
        print(#oslo.secret.list())
        "#,
    );
    assert_eq!(out, "true\nround trip\n0\n", "{err}");
}

/// **A plugin store is not reachable by name**, and the check is on the name asked for rather than
/// on who is asking — so deferring the call into a hook does not get past it.
#[test]
fn lua_cannot_name_a_plugin_store() {
    let home = tempfile::tempdir().expect("tempdir");
    let (out, err) = lua(
        home.path(),
        r#"
        local store, why = oslo.secret.open("plugin.notes")
        print(store == nil, why)
        local mine, no = oslo.secret.mine()
        print(mine == nil, no)
        "#,
    );
    assert!(out.contains("oslo.secret.mine()"), "{out:?} {err}");
    assert!(
        out.contains("only a plugin has one"),
        "the config is not a plugin: {out:?}"
    );
}

/// **What the user grants is what a plugin reaches**, and the rest is refused by name.
#[test]
fn a_plugin_reaches_what_the_user_granted_and_no_more() {
    let home = tempfile::tempdir().expect("tempdir");
    for (name, value) in [("gh-token", "declared"), ("bank", "not declared")] {
        let mut child = Command::new(oslo_bin())
            .args(["secret", "set", name])
            .env("XDG_DATA_HOME", home.path())
            .env("XDG_STATE_HOME", home.path().join("state"))
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn oslo");
        std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), value.as_bytes())
            .expect("write");
        child.wait().expect("wait");
    }

    let plugin = home.path().join("oslo/site/pack/tests/start/notes");
    std::fs::create_dir_all(plugin.join("plugin")).expect("mkdir");
    std::fs::create_dir_all(home.path().join(".config/oslo")).expect("config dir");
    std::fs::write(
        home.path().join(".config/oslo/init.lua"),
        r#"oslo.plugin.secrets("notes", { "gh-token" })"#,
    )
    .expect("write config");
    std::fs::write(
        plugin.join("plugin/init.lua"),
        r#"
        print("granted:", oslo.secret.get("gh-token"))
        print("undeclared:", select(2, oslo.secret.get("bank")))
        print("listed:", table.concat(oslo.secret.open("user"):list(), ","))
        local mine = oslo.secret.mine()
        mine:set("own", "kept")
        print("own:", mine:get("own"), mine:where())
        "#,
    )
    .expect("write");

    let mut child = Command::new(oslo_bin())
        .arg("-i")
        .env("XDG_DATA_HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("HOME", home.path())
        .env_remove("TERM")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn oslo");
    std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), b"exit\n").expect("write");
    let output = child.wait_with_output().expect("wait");
    let mut said = String::from_utf8_lossy(&output.stdout).to_string();
    said.push_str(&String::from_utf8_lossy(&output.stderr));
    assert!(
        said.contains("granted:\tdeclared"),
        "the granted name is readable: {said:?}"
    );
    assert!(
        said.contains("not declared in this plugin"),
        "an undeclared name is refused: {said:?}"
    );
    // The list is what it could read, not what is there: a name is itself information.
    assert!(said.contains("listed:\tgh-token"), "{said:?}");
    assert!(
        !said.contains("bank,") && !said.contains(",bank"),
        "{said:?}"
    );
    // Its own store is somewhere else entirely, beside the database `oslo.db` would make for it.
    assert!(said.contains("own:\tkept"), "{said:?}");
    assert!(said.contains("plugins/notes.secrets"), "{said:?}");
}
