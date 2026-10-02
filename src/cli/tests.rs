use super::*;

#[test]
fn root_prefers_command_line_then_environment_then_home_default() {
    let home = OsString::from("/home/tester");
    assert_eq!(
        root(
            Some(PathBuf::from("/command")),
            Some(PathBuf::from("/environment")),
            Some(home.clone())
        ),
        Ok(PathBuf::from("/command"))
    );
    assert_eq!(
        root(
            None,
            Some(PathBuf::from("/environment")),
            Some(home.clone())
        ),
        Ok(PathBuf::from("/environment"))
    );
    assert_eq!(
        root(None, None, Some(home)),
        Ok(PathBuf::from("/home/tester/.gardr"))
    );
}

#[test]
fn help_documents_the_default_root_and_docs_command() {
    assert!(HELP.contains("~/.gardr by default"));
    assert!(HELP.contains("docs       Print built-in guidance"));
}

#[test]
fn source_options_require_values() {
    let mut args = vec![
        "--repo".to_owned(),
        "--url".to_owned(),
        "example".to_owned(),
    ];
    assert_eq!(
        checked_option(&mut args, "--repo").unwrap_err(),
        "--repo requires a value"
    );
}

#[test]
fn repeated_harness_arguments_preserve_dispatch_order() {
    let mut args = vec![
        "--harness-arg".to_owned(),
        "-p".to_owned(),
        "--harness-arg".to_owned(),
        "complete the assigned work".to_owned(),
    ];
    assert_eq!(
        options(&mut args, "--harness-arg"),
        ["-p", "complete the assigned work"]
    );
    assert!(args.is_empty());
}

#[test]
fn start_is_interactive_only_without_autonomous_input() {
    let request = StartRequest {
        repo: None,
        url: None,
        thread: None,
        ask_file: None,
        current_dir: PathBuf::from("/repo"),
    };
    assert!(is_interactive(&request, &[]));
    assert!(!is_interactive(
        &request,
        &["-p".to_owned(), "do the work".to_owned()]
    ));

    let request_with_file = StartRequest {
        ask_file: Some(PathBuf::from("task.md")),
        ..request
    };
    assert!(!is_interactive(&request_with_file, &[]));
}

#[test]
fn herdr_agent_hint_is_set_only_when_gardr_runs_under_herdr() {
    assert_eq!(
        herdr_agent_hint(Some(OsString::from("1"))),
        Some(("HERDR_AGENT", "pi"))
    );
    assert_eq!(herdr_agent_hint(None), None);
    assert_eq!(herdr_agent_hint(Some(OsString::from("0"))), None);
    assert_eq!(herdr_agent_hint(Some(OsString::from(""))), None);
}

#[test]
fn nested_help_is_available_without_a_store() {
    assert!(subcommand_help(&[]));
    assert!(subcommand_help(&["--help".to_owned()]));
    assert!(!subcommand_help(&["list".to_owned()]));
    assert!(SPEC_HELP.contains("removed"));
    assert!(RUN_HELP.contains("validate-workspace"));
    assert!(CREDENTIAL_HELP.contains("set"));
}

#[test]
fn removed_spec_flag_returns_a_migration_error() {
    let store = gardr::Store::open("unused");
    let error = run_command(
        &store,
        vec!["start".to_owned(), "--spec".to_owned(), "old".to_owned()],
    )
    .unwrap_err();
    assert!(
        error.contains("--spec") && error.contains("removed"),
        "{error}"
    );
}

#[test]
fn flag_removes_a_boolean_switch_when_present() {
    let mut args = vec!["--stdin".to_owned(), "extra".to_owned()];
    assert!(flag(&mut args, "--stdin"));
    assert_eq!(args, ["extra"]);
    assert!(!flag(&mut args, "--stdin"));
}

#[test]
fn credential_set_registers_from_a_file_without_printing_the_value() {
    let temp = std::env::temp_dir().join(format!(
        "gardr-main-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let store = gardr::Store::open(temp.join("store"));
    let source = temp.join("secret.txt");
    std::fs::write(&source, b"super-secret").unwrap();
    credential(
        &store,
        vec![
            "set".to_owned(),
            "gh-pat".to_owned(),
            "--file".to_owned(),
            source.display().to_string(),
        ],
    )
    .unwrap();
    assert_eq!(
        std::fs::read(store.credential_path("gh-pat").unwrap()).unwrap(),
        b"super-secret"
    );
    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn credential_set_rejects_file_and_stdin_together() {
    let temp = std::env::temp_dir().join(format!(
        "gardr-main-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            + 1
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let store = gardr::Store::open(temp.join("store"));
    let error = credential(
        &store,
        vec![
            "set".to_owned(),
            "gh-pat".to_owned(),
            "--file".to_owned(),
            "whatever".to_owned(),
            "--stdin".to_owned(),
        ],
    )
    .unwrap_err();
    assert!(error.contains("mutually exclusive"));
    std::fs::remove_dir_all(temp).unwrap();
}
