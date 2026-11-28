//! Each expectation is what bash 5.3 prints for the same definition, byte for byte.

use super::function_text;

fn printed(source: &str) -> String {
    let name = source.split(['(', ' ']).next().unwrap_or_default();
    function_text(source, name, true).expect("a function")
}

#[test]
fn statements_end_in_a_semicolon_except_the_last() {
    assert_eq!(
        printed("f() { echo a; echo b; }"),
        "f () \n{ \n    echo a;\n    echo b\n}"
    );
}

#[test]
fn loops_and_conditionals_take_bashs_layout() {
    assert_eq!(
        printed("f() { for i in 1 2; do echo $i; done; while a; do b; done; }"),
        "f () \n{ \n    for i in 1 2;\n    do\n        echo $i;\n    done;\n    while a; do\n        b;\n    done\n}"
    );
    assert_eq!(
        printed("f() { if a; then b; elif c; then d; fi; }"),
        "f () \n{ \n    if a; then\n        b;\n    else\n        if c; then\n            d;\n        fi;\n    fi\n}"
    );
}

#[test]
fn a_here_document_keeps_its_body() {
    assert_eq!(
        printed("f() { cat <<EOF\nhello $x\nEOF\necho after\n}"),
        "f () \n{ \n    cat <<EOF\nhello $x\nEOF\n\n    echo after\n}"
    );
}

#[test]
fn quoting_globs_and_braces_survive_as_written() {
    assert_eq!(
        printed(r#"f() { echo "\$y" "a\"b" {a,b} ${!x}; [[ abc == a* ]]; }"#),
        "f () \n{ \n    echo \"\\$y\" \"a\\\"b\" {a,b} ${!x};\n    [[ abc == a* ]]\n}"
    );
}

#[test]
fn pipes_with_stderr_and_time_are_kept() {
    assert_eq!(
        printed("f() { ls |& cat; time sleep 0; }"),
        "f () \n{ \n    ls 2>&1 | cat;\n    time sleep 0\n}"
    );
}

#[test]
fn a_substitution_is_printed_again() {
    assert_eq!(
        printed("f() { echo $(ls   -l;  echo  x); }"),
        "f () \n{ \n    echo $(ls -l; echo x)\n}"
    );
}

#[test]
fn redirections_after_the_body_follow_the_brace() {
    assert_eq!(
        printed("f() { echo hi; } > out 2>&1"),
        "f () \n{ \n    echo hi\n} > out 2>&1"
    );
}

#[test]
fn an_exported_body_is_on_few_lines() {
    assert_eq!(
        function_text("f() { echo a; if true; then echo b; fi; }", "", false).as_deref(),
        Some("() {  echo a;\n if true; then\n echo b;\n fi\n}")
    );
}
