//! One table, both builtins.
//!
//! The failures R5.1–R5.3 describe were all "plausible answer, wrong answer, status 0", which a
//! test asserting `assert!(result.is_ok())` cannot see. So every case here pins the *exit status*
//! — 0 true, 1 false, 2 syntax error — and the shared table runs each expression through `[` and
//! `[[` so the two forms cannot drift apart again.
//!
//! Every expected status was taken from `bash --posix -c`.

use super::{builtin_extended_test, builtin_test};
use crate::env::scope::Environment;

/// Run `[ words… ]` and return its exit status.
fn bracket(words: &[&str]) -> i32 {
    let mut env = Environment::new();
    let mut argv = vec!["[".to_string()];
    argv.extend(words.iter().map(|w| w.to_string()));
    argv.push("]".to_string());
    builtin_test(&mut env, &argv).expect("test builtin must not unwind")
}

/// Run `[[ words… ]]` and return its exit status.
fn double_bracket(words: &[&str]) -> i32 {
    let mut env = Environment::new();
    let mut argv = vec!["[[".to_string()];
    argv.extend(words.iter().map(|w| w.to_string()));
    argv.push("]]".to_string());
    builtin_extended_test(&mut env, &argv).expect("extended test builtin must not unwind")
}

/// Expressions that mean the same thing in both forms. `(words, status)`.
const SHARED: &[(&[&str], i32)] = &[
    // Bare strings.
    (&[], 1),
    (&["x"], 0),
    (&[""], 1),
    // String length.
    (&["-z", ""], 0),
    (&["-z", "x"], 1),
    (&["-n", "x"], 0),
    (&["-n", ""], 1),
    // String comparison.
    (&["a", "=", "a"], 0),
    (&["a", "=", "b"], 1),
    (&["a", "!=", "b"], 0),
    (&["a", "!=", "a"], 1),
    (&["a", "<", "b"], 0),
    (&["b", "<", "a"], 1),
    (&["a", ">", "b"], 1),
    (&["b", ">", "a"], 0),
    // Arithmetic comparison.
    (&["3", "-eq", "3"], 0),
    (&["3", "-ne", "3"], 1),
    (&["3", "-gt", "2"], 0),
    (&["2", "-ge", "3"], 1),
    (&["2", "-lt", "3"], 0),
    (&["3", "-le", "3"], 0),
    (&["-1", "-lt", "0"], 0),
    // Blanks and a leading `+` are accepted.
    (&[" 3 ", "-eq", "3"], 0),
    (&["+3", "-eq", "3"], 0),
    // R5.1/R5.3: an operator that does not exist is a syntax error, not `false`.
    (&["-q", "x"], 2),
    (&["a", "-qq", "b"], 2),
    (&["a", "-lte", "b"], 2),
    // File predicates that need no fixture.
    (&["-d", "/"], 0),
    (&["-e", "/"], 0),
    (&["-f", "/"], 1),
    (&["-e", "/nonexistent-oslo-xyz"], 1),
    (&["-f", "/nonexistent-oslo-xyz"], 1),
    (&["-d", "/nonexistent-oslo-xyz"], 1),
    (&["-x", "/nonexistent-oslo-xyz"], 1),
    (&["-s", "/nonexistent-oslo-xyz"], 1),
    (&["-L", "/nonexistent-oslo-xyz"], 1),
    (&["-p", "/nonexistent-oslo-xyz"], 1),
    (&["-t", "not-a-number"], 2),
    (&["-t", "99"], 1),
    // `-o` on a name no shell option has is false, not an error: that is what bash answers, and
    // it is what lets `[ -o pipefail ] || ...` run on a shell without the option.
    (&["-o", "no-such-option"], 1),
    (&["-o", ""], 1),
    // Nothing here is a nameref, so `-R` is false for a set variable too.
    (&["-R", "PATH"], 1),
];

#[test]
fn shared_expressions_agree_in_both_forms() {
    for (words, expected) in SHARED {
        assert_eq!(
            bracket(words),
            *expected,
            "[ {} ] should exit {}",
            words.join(" "),
            expected
        );
        assert_eq!(
            double_bracket(words),
            *expected,
            "[[ {} ]] should exit {}",
            words.join(" "),
            expected
        );
    }
}

/// The integer comparisons, where the two forms part: `test` reads decimal integers and calls
/// anything else an error, `[[ ]]` evaluates each operand as an arithmetic expression.
/// `(words, test status, [[ status)`.
const INTEGER_OPERANDS: &[(&[&str], i32, i32)] = &[
    (&["abc", "-eq", "0"], 2, 0),
    (&["1", "-eq", "abc"], 2, 1),
    (&["", "-eq", "0"], 2, 0),
    (&["0x10", "-eq", "16"], 2, 0),
    (&["abc", "-gt", "0"], 2, 1),
    (&["1+1", "-eq", "2"], 2, 0),
    (&["2#101", "-eq", "5"], 2, 0),
    (&["010", "-eq", "8"], 1, 0),
    (&["08", "-eq", "8"], 0, 1),
    (&["1 2", "-eq", "1"], 2, 1),
];

#[test]
fn integer_operands_are_decimal_in_test_and_arithmetic_in_double_brackets() {
    for (words, posix, extended) in INTEGER_OPERANDS {
        assert_eq!(bracket(words), *posix, "[ {} ]", words.join(" "));
        assert_eq!(
            double_bracket(words),
            *extended,
            "[[ {} ]]",
            words.join(" ")
        );
    }
}

/// R5.1: the connectives, the parentheses, and the garbage that must not be true.
///
/// These are `[`-only because `[[ ]]`'s connectives never reach the builtin — the parser lowers
/// them to shell `&&`/`||`/`!`.
const POSIX_GRAMMAR: &[(&[&str], i32)] = &[
    // R5.1: four-or-more operands used to be unconditionally true.
    (&["-f", "/nope-a", "-a", "-f", "/nope-b"], 1),
    (&["-z", "", "-o", "-z", "x"], 0),
    (&["-n", "x", "-a", "-n", "y"], 0),
    (&["-n", "", "-o", "-n", ""], 1),
    (&["x", "-a", "y"], 0),
    (&["", "-a", "x"], 1),
    (&["", "-o", "x"], 0),
    (&["x", "-a", "y", "-a", "z"], 0),
    (&["", "-o", "", "-o", "x"], 0),
    // `-a` binds tighter than `-o`: false AND false OR true.
    (&["x", "-a", "", "-o", "y"], 0),
    // R5.2: `!` negates at every arity.
    (&["!", "-f", "/nonexistent-oslo-xyz"], 0),
    (&["!", "-d", "/"], 1),
    (&["!", "a", "=", "a"], 1),
    (&["!", "a", "=", "b"], 0),
    (&["!", "!", "a", "=", "a"], 0),
    (&["!", "-z", ""], 1),
    (&["!", ""], 0),
    (&["!", "x"], 1),
    (&["!"], 0),
    // `!` in an operand slot is data, not an operator.
    (&["-f", "-a"], 1),
    (&["!", "-a"], 1),
    // Three words with `-a` or `-o` in the middle join two strings, whatever they spell.
    (&["!", "-a", "/"], 0),
    (&["-z", "-a", "-z"], 0),
    (&["", "-o", "-n"], 0),
    // Parentheses.
    (&["(", "a", "=", "a", ")"], 0),
    (&["(", "a", "=", "b", ")"], 1),
    (&["(", "a", ")"], 0),
    (&["(", "", ")"], 1),
    (&["(", "a", "=", "b", ")", "-o", "(", "c", "=", "c", ")"], 0),
    (&["(", "a", "=", "b", ")", "-a", "(", "c", "=", "c", ")"], 1),
    (&["!", "(", "a", "=", "b", ")"], 0),
    // `test` is decimal-only, so `010` is ten.
    (&["010", "-eq", "10"], 0),
    (&["010", "-eq", "8"], 1),
    // Single words that look like operators but sit alone.
    (&["="], 0),
    (&["-z"], 0),
    (&["-a"], 0),
    // R5.1: unparseable input exits 2 rather than reporting a truth value.
    (&["a", "b"], 2),
    (&["a", "b", "c"], 2),
    (&["a", "b", "c", "d", "e"], 2),
    (&["x", "-a"], 2),
    (&["a", "="], 2),
    (&["!", "x", "="], 2),
    (&["(", ")"], 2),
    (&["(", "a", "=", "a"], 2),
    (&["a", "=", "a", ")"], 2),
    (&["1", "-eq", "1", "-a"], 2),
    // `-a`/`-o` do not short-circuit: the far side's error still surfaces.
    (&["1", "-eq", "1", "-o", "abc", "-eq", "1"], 2),
    (&["0", "-eq", "1", "-a", "abc", "-eq", "1"], 2),
];

#[test]
fn posix_grammar_matches_bash() {
    for (words, expected) in POSIX_GRAMMAR {
        assert_eq!(
            bracket(words),
            *expected,
            "[ {} ] should exit {}",
            words.join(" "),
            expected
        );
    }
}

/// The one operator whose meaning depends on the form: `==` globs only inside `[[ ]]`.
#[test]
fn double_equals_is_a_pattern_only_in_the_extended_form() {
    assert_eq!(bracket(&["abc", "==", "a*"]), 1);
    assert_eq!(double_bracket(&["abc", "==", "a*"]), 0);

    assert_eq!(bracket(&["abc", "==", "abc"]), 0);
    assert_eq!(double_bracket(&["abc", "==", "abc"]), 0);

    // `=` is literal in both.
    assert_eq!(bracket(&["abc", "=", "a*"]), 1);
    assert_eq!(double_bracket(&["abc", "=", "a*"]), 1);
}

#[test]
fn extended_form_rejects_more_than_three_operands() {
    assert_eq!(double_bracket(&["a", "=", "a", "-a", "b", "=", "b"]), 2);
}

#[test]
fn bracket_without_a_closing_bracket_is_an_error() {
    let mut env = Environment::new();
    let argv = vec!["[".to_string(), "a".to_string()];
    assert_eq!(builtin_test(&mut env, &argv).unwrap(), 2);
}

#[test]
fn test_name_needs_no_closing_bracket() {
    let mut env = Environment::new();
    let argv = vec![
        "test".to_string(),
        "a".to_string(),
        "=".to_string(),
        "a".to_string(),
    ];
    assert_eq!(builtin_test(&mut env, &argv).unwrap(), 0);
}

#[test]
fn extended_form_knows_about_shell_variables() {
    let mut env = Environment::new();
    env.set_var("OSLO_TEST_V", "set", false);
    let argv = vec![
        "[[".to_string(),
        "-v".to_string(),
        "OSLO_TEST_V".to_string(),
        "]]".to_string(),
    ];
    assert_eq!(builtin_extended_test(&mut env, &argv).unwrap(), 0);

    let argv = vec![
        "[[".to_string(),
        "-v".to_string(),
        "OSLO_TEST_UNSET".to_string(),
        "]]".to_string(),
    ];
    assert_eq!(builtin_extended_test(&mut env, &argv).unwrap(), 1);
}

/// R8.8: `-o` reads the live option table, so it cannot disagree with `set -o`.
///
/// The old implementation answered `false` for every name, which made `[[ -o errexit ]]` a
/// plausible-looking "errexit is off" no matter what `set -e` had done.
#[test]
fn shell_option_predicate_tracks_the_option_table() {
    use crate::env::options::ShellOption;

    let mut env = Environment::new();
    let ask = |env: &mut Environment, name: &str| {
        let argv: Vec<String> = ["[[", "-o", name, "]]"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        builtin_extended_test(env, &argv).expect("[[ must not unwind")
    };

    assert_eq!(ask(&mut env, "errexit"), 1);
    env.set_option(ShellOption::ErrExit, true);
    assert_eq!(ask(&mut env, "errexit"), 0);
    // Only the named option moved.
    assert_eq!(ask(&mut env, "nounset"), 1);

    // `test`/`[` reads the same table.
    env.set_option(ShellOption::NoUnset, true);
    let argv: Vec<String> = ["[", "-o", "nounset", "]"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(builtin_test(&mut env, &argv).unwrap(), 0);
}

/// R8.8: `-N` is "modified since last read": the modification time is strictly later.
#[test]
fn modified_since_last_read_needs_a_later_modification() {
    let dir = scratch_dir("newer");
    let file = dir.join("fresh");
    std::fs::write(&file, b"x").expect("write fixture");
    let path = file.to_string_lossy().to_string();
    let stamp = |accessed: std::time::SystemTime, modified: std::time::SystemTime| {
        let times = std::fs::FileTimes::new()
            .set_accessed(accessed)
            .set_modified(modified);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .and_then(|f| f.set_times(times))
            .expect("set times");
    };
    let now = std::time::SystemTime::now();
    let earlier = now - std::time::Duration::from_secs(60);

    stamp(earlier, now);
    assert_eq!(bracket(&["-N", &path]), 0);
    assert_eq!(double_bracket(&["-N", &path]), 0);
    // What `touch` leaves: both stamps alike.
    stamp(now, now);
    assert_eq!(bracket(&["-N", &path]), 1);

    assert_eq!(bracket(&["-N", "/nonexistent-oslo-xyz"]), 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// R5.3: `-r` asks the kernel. `stat()` succeeds on a mode-000 file the caller cannot open, so
/// the old implementation called it readable.
#[test]
fn readability_uses_access_not_stat() {
    if nix::unistd::geteuid().is_root() {
        return; // root can read anything; the distinction is invisible.
    }

    let dir = scratch_dir("access");
    let unreadable = dir.join("locked");
    std::fs::write(&unreadable, b"secret").unwrap();
    set_mode(&unreadable, 0o000);

    let path = unreadable.to_str().unwrap();
    assert_eq!(bracket(&["-r", path]), 1, "mode-000 file is not readable");
    assert_eq!(double_bracket(&["-r", path]), 1);
    // …but it still exists, which is what `stat` was actually answering.
    assert_eq!(bracket(&["-e", path]), 0);

    set_mode(&unreadable, 0o644);
    assert_eq!(bracket(&["-r", path]), 0);

    std::fs::remove_dir_all(&dir).ok();
}

/// The file predicates `test` used to answer `false` to unconditionally.
#[test]
fn file_predicates_have_real_answers_in_both_forms() {
    let dir = scratch_dir("files");
    let empty = dir.join("empty");
    let full = dir.join("full");
    std::fs::write(&empty, b"").unwrap();
    std::fs::write(&full, b"content\n").unwrap();
    set_mode(&full, 0o755);

    let cases: &[(&[&str], i32)] = &[
        (&["-s", full.to_str().unwrap()], 0),
        (&["-s", empty.to_str().unwrap()], 1),
        (&["-x", full.to_str().unwrap()], 0),
        (&["-x", empty.to_str().unwrap()], 1),
        (&["-w", full.to_str().unwrap()], 0),
        (&["-d", dir.to_str().unwrap()], 0),
        (&["-O", full.to_str().unwrap()], 0),
        (&[full.to_str().unwrap(), "-ef", full.to_str().unwrap()], 0),
        (&[full.to_str().unwrap(), "-ef", empty.to_str().unwrap()], 1),
    ];

    for (words, expected) in cases {
        assert_eq!(bracket(words), *expected, "[ {} ]", words.join(" "));
        assert_eq!(
            double_bracket(words),
            *expected,
            "[[ {} ]]",
            words.join(" ")
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("oslo-cond-{}-{}", tag, std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}
