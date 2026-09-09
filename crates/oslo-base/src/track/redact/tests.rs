use super::*;
#[test]
fn a_wrapper_is_not_the_command() {
    assert_eq!(head_of("sudo cargo build --release"), "cargo build");
    assert_eq!(head_of("doas systemctl restart nginx"), "systemctl restart");
    assert_eq!(head_of("time make -j8"), "make");
    assert_eq!(head_of("nice -n 10 cargo test"), "cargo test");
    assert_eq!(head_of("sudo -u root cargo build"), "cargo build");
    assert_eq!(head_of("env FOO=bar git status"), "git status");
    // A wrapper with nothing after it is the command you ran.
    assert_eq!(head_of("time"), "time");
    assert_eq!(head_of("sudo -i"), "sudo");
}

#[test]
fn a_leading_assignment_is_not_the_command() {
    assert_eq!(head_of("RUST_LOG=debug cargo test"), "cargo test");
    assert_eq!(head_of("A=1 B=2 sudo make install"), "make");
    // Not an assignment: an `=` inside an argument, or a name that is not a name.
    assert_eq!(head_of("./x=y arg"), "./x=y");
}

/// The column exists so that "what does cargo cost me" is answerable per activity. Grouping
/// `cargo build` with `cargo test` would make it useless.
#[test]
fn a_subcommand_stays_with_its_tool() {
    assert_eq!(head_of("git commit -m 'x y'"), "git commit");
    assert_eq!(head_of("cargo run --example xyz"), "cargo run");
    // Only for tools that have subcommands, and only when it looks like one.
    assert_eq!(head_of("ls build"), "ls");
    assert_eq!(head_of("git -C /tmp status"), "git");
    assert_eq!(head_of("cargo --version"), "cargo");
    assert_eq!(head_of("cargo ./script"), "cargo");
}

/// A separator glued to the command word used to travel with it into the `head` column, so
/// `cd; pwd` was filed under `cd;` and never counted with any other `cd`. Observed in a real
/// session, not imagined.
#[test]
fn punctuation_does_not_become_part_of_the_tool_name() {
    assert_eq!(head_of("cd; pwd"), "cd");
    assert_eq!(head_of("make&"), "make");
    assert_eq!(head_of("ls|wc -l"), "ls");
    assert_eq!(head_of("cargo build && ./run"), "cargo build");
    assert_eq!(head_of("cargo test; echo done"), "cargo test");
    assert_eq!(head_of("cargo build > log.txt"), "cargo build");
    assert_eq!(head_of("grep x 2>/dev/null"), "grep");
    // An opening bracket must not leave an empty head, which would drop the row outright.
    assert_eq!(head_of("(cd build && make)"), "cd");
    // What follows the first command is somebody else's head, not part of this one.
    assert_eq!(head_of("sudo cargo build; rm -rf /"), "cargo build");
}

#[test]
fn an_empty_line_has_no_head() {
    assert_eq!(head_of(""), "");
    assert_eq!(head_of("   "), "");
    assert_eq!(head_of("\n\ncargo build"), "");
}

/// The structural mitigation: the row survives, the arguments do not.
#[test]
fn a_risky_line_keeps_its_head_and_loses_its_arguments() {
    assert_eq!(
        prepare("curl --token abcdef https://x"),
        ("curl".to_string(), "curl".to_string())
    );
    // And an ordinary line is kept whole.
    assert_eq!(
        prepare("  cargo run --example xyz  "),
        (
            "cargo run --example xyz".to_string(),
            "cargo run".to_string()
        )
    );
    // Nothing to record at all.
    assert_eq!(prepare("   "), (String::new(), String::new()));
}

#[test]
fn credentials_are_recognised_in_every_shape_they_arrive_in() {
    for line in [
        "gpg --decrypt secrets.asc",
        "gh auth login",
        "docker login -u me registry.example.com",
        "mysql -h db -p",
        "curl --password hunter2 https://x",
        "curl -H 'Authorization: Bearer x' https://x",
        "psql postgres://x",
        "export GITHUB_TOKEN=abc",
        "aws configure set x",
        "curl -u alice:hunter2 https://x",
        "mysqldump -phunter2 db",
        "echo ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6",
        "openai --api-key sk-abcdefghijklmnop",
        "aws s3 ls --profile AKIAIOSFODNN7EXAMPLE",
        "echo eyJhbGciOiJIUzI1NiJ9.x",
        "deploy --key aB3dE5fG7hJ9kL1mN3pQ5rS7",
    ] {
        assert!(is_risky(line), "{line} should not be recorded in full");
    }
}

#[test]
fn ordinary_lines_are_recorded_in_full() {
    for line in [
        "cargo run --example xyz",
        "git commit -m 'fix the parser'",
        "ls -la /var/log",
        "make --prefix=/usr install",
        "git log --author=me",
        "rg 'fn main' src/",
        "docker compose up -d",
    ] {
        assert!(!is_risky(line), "{line} is not a secret");
    }
}

/// **A name that says `KEY` beside a value that is a path.** Reported from a real session: this
/// line went missing from the history every time it was run, while the prediction model — which
/// learns from a different table — went on suggesting it, so the shell looked like it was
/// forgetting on purpose.
#[test]
fn an_assignment_pointing_at_a_key_file_is_not_a_key() {
    let line = concat!(
        r#"sudo make hardware-smoke HARDWARE_SERIALS="$SERIALS" SIGN_FILE="$SIGN" "#,
        r#"SIGN_HASH=sha512 SIGN_KEY="$MOK/MOK.priv" SIGN_CERT="$MOK/MOK.der""#
    );
    assert!(!is_risky(line), "the values are paths and variables");
    assert_eq!(prepare(line).0, line, "recorded whole");

    for kept in [
        "make SIGN_KEY=/etc/keys/MOK.priv",
        "make SIGN_KEY=~/keys/MOK.priv",
        "make SIGN_KEY=./MOK.priv",
        "deploy TOKEN=$CI_TOKEN",
        "deploy API_KEY=",
    ] {
        assert!(!is_risky(kept), "{kept} points at a secret, it is not one");
    }
}

/// And the case the rule is actually for still trips: a literal value beside a telling name.
#[test]
fn a_literal_beside_a_secret_name_is_still_dropped() {
    for line in [
        "deploy API_KEY=abc123",
        "run PASSWORD=hunter2",
        "make SIGN_KEY=s3cr3t",
    ] {
        assert!(is_risky(line), "{line} carries the value itself");
    }
}

/// **An assignment in front of a command is not a secret by itself.** It used to be: any line
/// starting `FOO=bar` lost its arguments, so `WGPU_BACKEND=vulkan rerun` and
/// `RUST_LOG=debug cargo test` were both stored as bare command names. The credential case the
/// rule existed for is caught by the value, wherever in the line it sits.
#[test]
fn a_leading_assignment_is_read_like_any_other_word() {
    assert!(!is_risky("WGPU_BACKEND=vulkan rerun"));
    assert!(!is_risky("RUST_LOG=debug cargo test"));
    assert!(!is_risky("A=1 B=2 make install"));

    assert!(is_risky(
        "GITHUB_TOKEN=ghp_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6 gh pr list"
    ));
    assert!(is_risky("GITHUB_TOKEN=hunter2 gh pr list"));
    assert!(is_risky("OPENAI_API_KEY=sk-abcdefghijklmnop chat"));
}

/// The glued `-p<value>` rule cannot tell `-phunter2` from `-print`, and it is not supposed to
/// try: a filter that has to be perfect is a bad filter. This pins what the false positive
/// costs — one line's arguments, never the row — so that nobody later "fixes" it by narrowing
/// the rule until a real password gets through.
#[test]
fn a_false_positive_costs_arguments_and_nothing_else() {
    assert!(is_risky("find . -print"));
    assert_eq!(
        prepare("find . -print"),
        ("find".to_string(), "find".to_string())
    );
}

/// A paste is not a habit, and a continued line is not a suggestion.
#[test]
fn a_paste_and_a_heredoc_body_are_never_recorded_whole() {
    assert!(is_risky(&format!("echo {}", "x".repeat(MAX_ARGV))));
    assert!(is_risky("cat <<EOF\nsome body\nEOF"));
    assert_eq!(prepare("cat <<EOF\nbody\nEOF").0, "cat");
}

#[test]
fn whole_subtrees_stay_out_of_the_store() {
    assert!(is_excluded("/w/p/node_modules/react/lib"));
    assert!(is_excluded("/w/p/.git/refs"));
    assert!(!is_excluded("/w/p/src"));
    assert!(
        !is_excluded("/w/p/gitignore"),
        "a component is matched whole, not as a prefix"
    );
}

/// `$HOME` is deliberately *not* on the exclusion list, though the design puts it there.
///
/// Excluding it at write time throws away every command run at home, which kills the
/// directory-aware suggestion in the one directory many people spend the day in. Keeping it out
/// of *jump candidates* is the part that was actually wanted, and that is enforced in the
/// directory queries — see `query::tests::home_is_remembered_but_never_jumped_to`.
#[test]
fn home_itself_is_still_written_down() {
    assert!(!is_excluded("/home/u"));
    assert!(!is_excluded("/home/u/src"));
}

/// `/tmp` is on the design's exclusion list as a *directory*, next to two entries that say
/// "anything under". Nobody works in the lobby, and plenty of people work in a build tree
/// under it — excluding those too would kill the feature for them with no diagnostic at all,
/// which is the failure this store can least afford, since its only symptom is a `cd` that
/// quietly never learns anything.
#[test]
fn a_build_tree_under_tmp_is_ordinary_work() {
    assert!(is_excluded("/tmp"));
    assert!(is_excluded("/tmp/"), "with or without the slash");
    assert!(!is_excluded("/tmp/build-xyz"));
    assert!(!is_excluded("/tmp/scratch/src"));
    assert!(!is_excluded("/tmpfoo"), "a prefix is not a component");
    // And the component rules still reach inside it.
    assert!(is_excluded("/tmp/p/node_modules/react"));
}

/// **An ordinary assignment is not a key.** `WAYLAND_DISPLAY=wayland-0` is 25 characters with upper
/// case, lower case and a digit in it — every test `looks_like_a_key` applies — but only because
/// the name is glued to the value. Judging `NAME=value` as one word reduced the line to `export`,
/// so a variable somebody exported never appeared in their history again.
#[test]
fn an_assignment_is_judged_on_its_value() {
    for line in [
        "export WAYLAND_DISPLAY=wayland-0",
        "export XDG_RUNTIME_DIR=/run/user/1000",
        "export EDITOR=nvim",
        "export LC_ALL=en_US.UTF-8",
        "env SOME_LONG_VARIABLE_NAME=1 ls",
    ] {
        assert!(!is_risky(line), "{line} was reduced to its head");
        assert_eq!(prepare(line).0, line, "{line} did not survive whole");
    }
}

/// The value still decides, so a key assigned to an innocent name is still caught.
#[test]
fn a_key_in_an_assignment_is_still_a_key() {
    for line in [
        "export GITHUB_TOKEN=ghp_AbCd0123456789AbCd0123",
        "export SOMENAME=AbCdEf0123456789AbCdEf01",
    ] {
        assert!(is_risky(line), "{line} was written down whole");
        assert_eq!(prepare(line).0, "export");
    }
}
