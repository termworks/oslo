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
#[cfg(feature = "crypt")]
fn a_plugin_reaches_what_the_user_granted_and_no_more() {
    let home = tempfile::tempdir().expect("tempdir");
    keep(
        home.path(),
        &[("gh-token", "declared"), ("bank", "not declared")],
    );
    let said = with_plugins(
        home.path(),
        r#"oslo.plugin.secrets("notes", { "gh-token" })"#,
        &[(
            "notes",
            "init.lua",
            r#"
            print("granted:", oslo.secret.get("gh-token"))
            print("undeclared:", select(2, oslo.secret.get("bank")))
            print("listed:", table.concat(oslo.secret.open("user"):list(), ","))
            print("bare list:", table.concat(oslo.secret.list(), ","))
            print("forget:", select(2, oslo.secret.forget("bank")))
            local mine = oslo.secret.mine()
            mine:set("own", "kept")
            print("own:", mine:get("own"), mine:where())
            "#,
        )],
    );
    assert!(
        said.contains("granted:\tdeclared"),
        "the granted name is readable: {said:?}"
    );
    assert!(
        said.contains("undeclared:\tbank: not declared in this plugin"),
        "an undeclared name is refused: {said:?}"
    );
    // The list is what it could read, not what is there: a name is itself information.
    assert!(said.contains("listed:\tgh-token\n"), "{said:?}");
    assert!(said.contains("bare list:\tgh-token\n"), "{said:?}");
    assert!(
        said.contains("forget:\tbank: not declared"),
        "the bare forget is filtered too: {said:?}"
    );
    // Its own store is somewhere else entirely, beside the database `oslo.db` would make for it.
    assert!(said.contains("own:\tkept"), "{said:?}");
    assert!(said.contains("plugins/notes.secrets"), "{said:?}");
}

/// **`$OSLO_SECRET_STORE` is no way round `open`'s rule.** The bare calls act on whichever store the
/// variable selects, and a `plugin.` one is refused there exactly as it is by name.
#[test]
fn the_store_variable_cannot_select_a_plugin_store() {
    let home = tempfile::tempdir().expect("tempdir");
    let (out, err) = lua(
        home.path(),
        r#"
        oslo.env.set("OSLO_SECRET_STORE", "plugin.victim")
        for _, verb in ipairs({ "get", "set", "forget", "list", "seal", "unseal" }) do
          local answer, why = oslo.secret[verb]("planted", "x")
          print(verb, answer == nil, why)
        end
        "#,
    );
    for verb in ["get", "set", "forget", "list", "seal", "unseal"] {
        assert!(
            out.contains(&format!(
                "{verb}\ttrue\tplugin.victim: a plugin's store is reached with oslo.secret.mine()"
            )),
            "{verb} reached the plugin store: {out:?} {err}"
        );
    }
    assert!(
        !home.path().join("oslo/plugins/victim.secrets").exists(),
        "nothing was written into it"
    );
}

/// The same door from inside a plugin's load, where it was found: one plugin pointing the variable
/// at another's store.
#[test]
#[cfg(feature = "crypt")]
fn a_plugin_cannot_reach_another_plugins_store_through_the_variable() {
    let home = tempfile::tempdir().expect("tempdir");
    let said = with_plugins(
        home.path(),
        "",
        &[
            (
                "avictim",
                "v.lua",
                r#"print("victim set:", oslo.secret.mine():set("cursor", "victim-private"))"#,
            ),
            (
                "thief",
                "t.lua",
                r#"
                oslo.env.set("OSLO_SECRET_STORE", "plugin.avictim")
                print("thief list:", oslo.secret.list())
                print("thief set:", oslo.secret.set("planted", "x"))
                print("thief get:", oslo.secret.get("cursor"))
                "#,
            ),
        ],
    );
    assert!(said.contains("victim set:\ttrue"), "{said:?}");
    for verb in ["list", "set", "get"] {
        assert!(
            said.contains(&format!(
                "thief {verb}:\tnil\tplugin.avictim: a plugin's store"
            )),
            "{verb}: {said:?}"
        );
    }
    assert!(!said.contains("victim-private"), "{said:?}");
}

/// **`unseal` opens anything sealed to your key**, so it is refused to a plugin's load, on the bare
/// call and on a handle it made — while the config keeps it, and `mine()` seals the plugin's own.
#[test]
#[cfg(feature = "crypt")]
fn seal_and_unseal_are_refused_while_a_plugin_loads() {
    let home = tempfile::tempdir().expect("tempdir");
    let said = with_plugins(
        home.path(),
        r#"
        SEALED = oslo.secret.seal("from-the-config")
        print("config unseal:", oslo.secret.unseal(SEALED))
        "#,
        &[(
            "sneaky",
            "init.lua",
            r#"
            print("bare unseal:", oslo.secret.unseal(SEALED))
            print("bare seal:", oslo.secret.seal("x"))
            local user = oslo.secret.open("user")
            print("handle unseal:", user:unseal(SEALED))
            local mine = oslo.secret.mine()
            print("own:", mine:unseal(mine:seal("own-blob")))
            "#,
        )],
    );
    assert!(said.contains("config unseal:\tfrom-the-config"), "{said:?}");
    for door in ["bare unseal", "bare seal", "handle unseal"] {
        assert!(
            said.contains(&format!("{door}:\tnil\t")) && said.contains("not while a plugin loads"),
            "{door} was not refused: {said:?}"
        );
    }
    assert_eq!(said.matches("from-the-config").count(), 1, "{said:?}");
    assert!(said.contains("own:\town-blob"), "{said:?}");
}

/// **Only the config grants.** A plugin granting itself is refused, so its second file reads
/// nothing either.
#[test]
#[cfg(feature = "crypt")]
fn a_plugin_cannot_grant_itself_a_secret() {
    let home = tempfile::tempdir().expect("tempdir");
    keep(home.path(), &[("gh-token", "the-token")]);
    let said = with_plugins(
        home.path(),
        "",
        &[
            (
                "sneaky",
                "a.lua",
                r#"print("grant:", pcall(oslo.plugin.secrets, "sneaky", { "gh-token" }))"#,
            ),
            (
                "sneaky",
                "b.lua",
                r#"print("second file:", oslo.secret.get("gh-token"))"#,
            ),
        ],
    );
    assert!(
        said.contains("grant:\tfalse") && said.contains("a plugin may not grant"),
        "{said:?}"
    );
    assert!(
        said.contains("second file:\tnil\tgh-token: not declared"),
        "{said:?}"
    );
    assert!(!said.contains("the-token"), "{said:?}");
}

/// A store whose storage Lua defined is held to the same grant as a directory of files.
#[test]
#[cfg(feature = "crypt")]
fn a_defined_store_is_filtered_for_a_plugin_too() {
    let home = tempfile::tempdir().expect("tempdir");
    let said = with_plugins(
        home.path(),
        r#"
        local kept = {}
        oslo.secret.define("vault", {
          get = function(name) return kept[name] end,
          set = function(name, sealed) kept[name] = sealed return true end,
          list = function()
            local names = {}
            for name in pairs(kept) do names[#names + 1] = name end
            table.sort(names)
            return names
          end,
        })
        oslo.secret.open("vault"):set("deploy", "vault-value")
        oslo.secret.open("vault"):set("shared", "granted-value")
        oslo.plugin.secrets("reader", { "shared" })
        "#,
        &[(
            "reader",
            "init.lua",
            r#"
            local vault = oslo.secret.open("vault")
            print("get:", vault:get("deploy"))
            print("granted:", vault:get("shared"))
            print("list:", table.concat(vault:list(), ","))
            "#,
        )],
    );
    assert!(said.contains("get:\tnil\tdeploy: not declared"), "{said:?}");
    assert!(said.contains("granted:\tgranted-value"), "{said:?}");
    assert!(said.contains("list:\tshared\n"), "{said:?}");
    assert!(!said.contains("vault-value"), "{said:?}");
}

/// Keep each `(name, value)` in the `user` store, through the command line.
#[cfg(feature = "crypt")]
fn keep(home: &std::path::Path, secrets: &[(&str, &str)]) {
    for (name, value) in secrets {
        let mut child = Command::new(oslo_bin())
            .args(["secret", "set", name])
            .env("XDG_DATA_HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env_remove("OSLO_SECRET_IDENTITY")
            .env_remove("OSLO_SECRET_STORE")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("spawn oslo");
        std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), value.as_bytes())
            .expect("write");
        assert!(child.wait().expect("wait").success(), "set {name}");
    }
}

/// Start an interactive shell whose `init.lua` is `config`, with each `(plugin, file, body)`
/// installed, and answer everything it printed.
#[cfg(feature = "crypt")]
fn with_plugins(home: &std::path::Path, config: &str, plugins: &[(&str, &str, &str)]) -> String {
    for (plugin, file, body) in plugins {
        let root = home.join("oslo/site/pack/tests/start").join(plugin);
        std::fs::create_dir_all(root.join("plugin")).expect("mkdir");
        std::fs::write(root.join("plugin").join(file), body).expect("write plugin");
    }
    std::fs::create_dir_all(home.join(".config/oslo")).expect("config dir");
    std::fs::write(home.join(".config/oslo/init.lua"), config).expect("write config");

    let mut child = Command::new(oslo_bin())
        .arg("-i")
        .env("XDG_DATA_HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("OSLO_SECRET_IDENTITY")
        .env_remove("OSLO_SECRET_STORE")
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
    said
}
