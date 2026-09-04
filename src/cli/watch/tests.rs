use super::*;

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn parses_exact_command_after_mandatory_separator() {
    let request = parse(
        &words(&[
            "--postpone",
            "--restart",
            "--debounce",
            "25",
            "src/**/*.rs",
            "Cargo.toml",
            "--",
            "sh",
            "-c",
            "echo $PWD",
        ]),
        PathBuf::from("/work"),
    )
    .expect("request");
    assert_eq!(request.spec.argv, words(&["sh", "-c", "echo $PWD"]));
    assert_eq!(request.spec.patterns.len(), 2);
    assert!(!request.spec.initial);
    assert_eq!(request.spec.policy, Policy::Restart);
    assert_eq!(request.spec.debounce, Duration::from_millis(25));
}

#[test]
fn separator_and_paths_are_required() {
    assert!(parse(&words(&["src", "echo"]), PathBuf::from("/work")).is_err());
    assert!(parse(&words(&["--", "echo"]), PathBuf::from("/work")).is_err());
}

#[test]
fn attach_selects_scratch() {
    let request = parse(
        &words(&["--attach", "src", "--", "echo"]),
        PathBuf::from("/work"),
    )
    .expect("request");
    assert_eq!(request.scratch, ScratchChoice::Required(None));
    assert!(request.attach);
}

#[test]
fn private_worker_requires_its_inherited_marker() {
    let token = "0123456789abcdef0123456789abcdef";
    let error =
        private_args(&words(&[&format!("--__worker={token}")])).expect_err("public private flag");
    assert!(error.contains("refused"), "{error}");
}
