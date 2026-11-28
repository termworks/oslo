//! `declare`, and the attributes this shell refuses rather than pretends to have.
//!
//! The unit tests beside the builtin already cover the decision. These go through the real binary
//! because the failure that matters is what a *script* sees: the status it can branch on and the
//! diagnostic a person reads, neither of which a direct call to the builtin function exercises.

mod common;

use common::oslo_bin;
use std::process::Command;

struct Ran {
    stdout: String,
    stderr: String,
    status: i32,
}

fn oslo(script: &str) -> Ran {
    let out = Command::new(oslo_bin())
        .arg("-c")
        .arg(script)
        .output()
        .expect("spawn oslo");
    Ran {
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        status: out.status.code().unwrap_or(-1),
    }
}

/// `declare -A` makes an associative array: the subscript is a key, so two keys are two entries,
/// where an indexed array would evaluate both as arithmetic and land them on element 0.
#[test]
fn an_associative_array_keeps_its_keys_apart() {
    let ran = oslo("declare -A m; m[a]=1; m[b]=2; echo \"${m[a]}|${m[b]}|${#m[@]}\"");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert_eq!(ran.stdout.trim(), "1|2|2");
    let ran = oslo("typeset -A t=([k]=v); echo \"${t[k]}\"");
    assert_eq!(ran.stdout.trim(), "v", "typeset is the same builtin");
}

/// The case and nameref attributes are real attributes, and a letter with no meaning is refused.
#[test]
fn the_case_and_nameref_attributes_work() {
    assert_eq!(oslo("declare -l v=ABC; echo $v").stdout.trim(), "abc");
    assert_eq!(oslo("declare -u v=abc; echo $v").stdout.trim(), "ABC");
    assert_eq!(oslo("x=1; declare -n v=x; v=2; echo $x").stdout.trim(), "2");
    let ran = oslo("declare -Q v");
    assert_eq!(ran.status, 2);
    assert!(!ran.stderr.is_empty());
}

/// The ones oslo *does* have keep working, so the refusals above are a short list rather than a
/// general shrug at `declare`.
#[test]
fn the_attributes_this_shell_has_still_work() {
    assert_eq!(
        oslo("declare -a a; a[2]=x; echo ${#a[@]}").stdout.trim(),
        "1"
    );
    assert_eq!(oslo("declare -r r=1; echo $r").stdout.trim(), "1");
    assert_eq!(
        oslo("declare -x e=1; sh -c 'echo $e'").stdout.trim(),
        "1",
        "-x did not export"
    );
    assert_eq!(
        oslo("f() { declare -g g=out; }; f; echo $g").stdout.trim(),
        "out"
    );
}

/// **A read-only mark from `local -r` leaves with the function.**
///
/// The mark went into the process-wide set and nothing ever took it out, so
/// `f() { local -r x=1; }; f` left `x` frozen for the rest of the session — a name that could never
/// be assigned again and had no value under it. Every spelling below was checked against bash:
/// `local` and `declare` are local declarations and their marks are scoped; the `readonly` builtin
/// is not local and its mark is global even inside a function.
#[test]
fn a_scoped_readonly_leaves_with_its_scope() {
    let ran = oslo("f() { local -r x=1; }; f; x=2; echo \"x=$x\"");
    assert_eq!(ran.stdout.trim_end(), "x=2", "{}", ran.stderr);
    assert!(ran.stderr.is_empty(), "and said nothing: {}", ran.stderr);

    // `declare -r` inside a function is a local declaration too.
    let ran = oslo("f() { declare -r y=1; }; f; y=2; echo \"y=$y\"");
    assert_eq!(ran.stdout.trim_end(), "y=2", "{}", ran.stderr);

    // But it is still read-only *while* the function runs. `declare` reports the refusal as a
    // status, where a bare `a=9` would abandon the whole call.
    let ran = oslo("f() { local -r a=1; declare a=9; echo \"a=$a\"; }; f");
    assert_eq!(ran.stdout.trim_end(), "a=1", "frozen inside the function");
    assert!(!ran.stderr.is_empty(), "and said so");
}

/// The `readonly` builtin is not a local declaration, wherever it is written.
#[test]
fn the_readonly_builtin_marks_globally_even_in_a_function() {
    let ran = oslo("f() { readonly w=1; }; f\nw=2\necho \"w=$w\"");
    assert_eq!(ran.stdout.trim_end(), "w=1", "still frozen after the call");
    assert!(!ran.stderr.is_empty(), "and refused the assignment");

    let ran = oslo("readonly z=1\nz=2\necho \"z=$z\"");
    assert_eq!(ran.stdout.trim_end(), "z=1");
}

/// A name that was *already* read-only cannot be declared local over, so it keeps its mark.
#[test]
fn an_outer_readonly_survives_an_inner_declaration() {
    let ran = oslo("readonly g=1; f() { local -r g=2; }; f 2>/dev/null\ng=3\necho \"g=$g\"");
    assert_eq!(ran.stdout.trim_end(), "g=1", "{}", ran.stderr);
}

/// **One function, one definition, whichever builtin is asked.**
///
/// There were two complete AST-to-source printers in the binary and they disagreed: `set` rendered
/// `if true; then echo hi; fi` on one line where `type` — and bash — put it on three. So the same
/// function had two definitions depending on which builtin you asked, and `eval "$(…)"` round
/// trips through one of them.
#[test]
fn every_builtin_prints_a_function_the_same_way() {
    let body = "f() { if true; then echo hi; fi; }";
    let of = |builtin: &str| {
        let ran = oslo(&format!("{body}; {builtin}"));
        ran.stdout
            .lines()
            .skip_while(|line| !line.starts_with("f ()"))
            .take(6)
            .collect::<Vec<_>>()
            .join("\n")
    };

    let from_type = of("type f");
    assert!(!from_type.is_empty(), "type printed a definition");
    assert_eq!(of("set"), from_type, "`set` agrees with `type`");
    assert_eq!(of("declare -f f"), from_type, "and so does `declare -f`");

    // The shape itself, so a change to the shared printer is a deliberate one. This is bash's,
    // trailing spaces and all — checked against it rather than written from memory.
    assert_eq!(
        from_type,
        "f () \n{ \n    if true; then\n        echo hi;\n    fi\n}"
    );
}

/// `-p` prints what was declared, attributes and all.
///
/// `declare -i n=2+3` is the one that made this matter: the attribute is what evaluated `2+3`, so
/// a `-p` line that says `declare -- n="5"` cannot be read back as the declaration it describes.
/// Letter order is bash's — `i`, then `r`, then `x`.
#[test]
fn the_integer_attribute_survives_into_declare_p() {
    let ran = oslo("declare -i n=2+3; declare -p n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert_eq!(ran.stdout.trim(), r#"declare -i n="5""#);

    let ran = oslo("declare -irx n=7; declare -p n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert_eq!(ran.stdout.trim(), r#"declare -irx n="7""#);

    // And `+i` takes it back off, so the line goes back to saying nothing about it.
    let ran = oslo("declare -i n=1; declare +i n; declare -p n");
    assert_eq!(ran.status, 0, "{}", ran.stderr);
    assert_eq!(ran.stdout.trim(), r#"declare -- n="1""#);
}
