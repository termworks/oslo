//! Sensitive variables through the real binary: a secret's value expands and exports as itself,
//! and `set -x`, `set`, `declare -p`, `typeset -p` and `export -p` show `<secret>` instead.

mod common;

use common::oslo_bin;
use std::path::Path;
use std::process::{Command, Stdio};

/// What a run printed, each stream on its own.
struct Said {
    out: String,
    err: String,
}

/// Run `script` as a Lua file against a home of its own.
fn lua(home: &Path, script: &str) -> Said {
    let file = home.join("case.lua");
    std::fs::write(&file, script).expect("write script");
    let output = Command::new(oslo_bin())
        .arg(&file)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("OSLO_SECRET_IDENTITY")
        .env_remove("OSLO_SECRET_STORE")
        .stdin(Stdio::null())
        .output()
        .expect("spawn oslo");
    Said {
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// **`{ secret = true }` is the explicit spelling**, and it changes what is printed and nothing
/// else: the word expands, the child is handed the value, and a plain variable is untouched.
#[test]
fn an_explicit_mark_hides_the_value_from_every_listing() {
    let home = tempfile::tempdir().expect("tempdir");
    let said = lua(
        home.path(),
        r#"
        oslo.env.set("TOK", "lua-marked-value", { secret = true })
        oslo.env.set("PLAIN", "plain-value")
        oslo.proc.exec([[
          echo "word:$TOK"
          printenv TOK
          declare -p TOK PLAIN
          typeset -p TOK
          export -p | grep -e ' TOK=' -e ' PLAIN='
          set | grep -e '^TOK=' -e '^PLAIN='
          set -x
          : "$TOK"
          TOK=another-marked-value
          export TOK=third-marked-value
          set +x
        ]])
        "#,
    );
    let out = &said.out;
    assert!(out.contains("word:lua-marked-value\n"), "{out}");
    assert!(
        out.contains("\nlua-marked-value\n"),
        "the child gets it: {out}"
    );
    assert!(
        out.contains("declare -x TOK=<secret>\n"),
        "declare -p: {out}"
    );
    assert_eq!(out.matches("declare -x TOK=<secret>").count(), 3, "{out}");
    assert!(out.contains("TOK=<secret>\n"), "set: {out}");
    assert!(out.contains(r#"declare -x PLAIN="plain-value""#), "{out}");
    assert!(out.contains("PLAIN=plain-value\n"), "{out}");
    let err = &said.err;
    assert!(err.contains("+ : <secret>\n"), "xtrace of a word: {err}");
    assert!(err.contains("+ TOK=<secret>"), "of an assignment: {err}");
    assert!(err.contains("+ export TOK=<secret>"), "of export: {err}");
    assert!(!err.contains("marked-value"), "{err}");
}

/// `secret = false` takes the mark off again.
#[test]
fn the_mark_can_be_taken_off() {
    let home = tempfile::tempdir().expect("tempdir");
    let said = lua(
        home.path(),
        r#"
        oslo.env.set("TOK", "first", { secret = true })
        oslo.env.set("TOK", "second", { secret = false })
        oslo.proc.exec("declare -p TOK")
        "#,
    );
    assert!(
        said.out.contains(r#"declare -x TOK="second""#),
        "{}",
        said.out
    );
}

/// **A value `oslo.secret.*` handed to Lua is marked when Lua assigns it**, through `oslo.env.set`
/// or a plain global, without being asked.
#[test]
#[cfg(feature = "crypt")]
fn a_value_from_the_store_is_marked_when_lua_assigns_it() {
    let home = tempfile::tempdir().expect("tempdir");
    let said = lua(
        home.path(),
        r#"
        oslo.secret.set("gh-token", "store-handed-value")
        oslo.env.set("VIA_SET", oslo.secret.get("gh-token"))
        VIA_GLOBAL = oslo.secret.get("gh-token")
        oslo.env.set("UNRELATED", "store-handed-value-not")
        oslo.proc.exec("declare -p VIA_SET VIA_GLOBAL UNRELATED; printenv VIA_SET")
        "#,
    );
    let out = &said.out;
    assert!(out.contains("declare -x VIA_SET=<secret>"), "{out}");
    assert!(out.contains("declare -x VIA_GLOBAL=<secret>"), "{out}");
    assert!(
        out.contains(r#"declare -x UNRELATED="store-handed-value-not""#),
        "{out}"
    );
    assert!(out.ends_with("store-handed-value\n"), "{out}");
}

/// **A plain script is untouched.** Nothing marks a variable except oslo, so bash's own listing and
/// trace are what a script that never mentions a secret sees.
#[test]
fn a_variable_nobody_marked_prints_as_bash_prints_it() {
    let run = common::run("TOK=abc; set -x; : \"$TOK\"; set +x; declare -p TOK");
    assert_eq!(run.out(), r#"declare -- TOK="abc""#);
    assert!(run.err().contains("+ : abc"), "{}", run.err());
}

/// **The recipe of a stored variable that reads the store marks it before it runs**, so even the
/// trace of its own materialisation says `<secret>`.
#[test]
#[cfg(feature = "crypt")]
fn a_lazy_secret_is_hidden_from_its_own_materialisation() {
    use std::io::Write;
    let home = tempfile::tempdir().expect("tempdir");
    let env = |command: &mut Command| {
        command
            .env("HOME", home.path())
            .env("XDG_DATA_HOME", home.path())
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("OSLO_MACROS_BIN", home.path().join("bin"))
            .env_remove("OSLO_SECRET_IDENTITY")
            .env_remove("OSLO_SECRET_STORE")
            .env_remove("GITHUB_TOKEN")
            .env_remove("TERM");
    };

    let mut set = Command::new(oslo_bin());
    env(set
        .args(["secret", "set", "gh-token"])
        .stdin(Stdio::piped()));
    let mut child = set.spawn().expect("spawn oslo");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"ghp_lazy_value")
        .expect("write");
    assert!(child.wait().expect("wait").success());

    let recipe = format!(
        "GITHUB_TOKEN=$({} secret get gh-token)",
        oslo_bin().display()
    );
    let mut add = Command::new(oslo_bin());
    env(add
        .args(["macros", "add", "--var", &recipe])
        .stdin(Stdio::null())
        .stdout(Stdio::null()));
    assert!(add.status().expect("macros add").success());

    let mut shell = Command::new(oslo_bin());
    env(shell
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped()));
    let mut child = shell.spawn().expect("spawn oslo");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(
            b"set -x\necho \"[$GITHUB_TOKEN]\"\nset +x\ndeclare -p GITHUB_TOKEN\n\
              printenv GITHUB_TOKEN\nexit\n",
        )
        .expect("write");
    let output = child.wait_with_output().expect("wait");
    let out = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(out.contains("[ghp_lazy_value]"), "it expands: {out}");
    assert!(out.contains("declare -x GITHUB_TOKEN=<secret>"), "{out}");
    assert!(
        out.contains("\nghp_lazy_value\n"),
        "the child gets it: {out}"
    );
    assert!(
        err.contains("export GITHUB_TOKEN=<secret>"),
        "the materialisation is traced, hidden: {err}"
    );
    assert!(err.contains("echo '[<secret>]'"), "{err}");
    assert!(!err.contains("ghp_lazy_value"), "{err}");
}
