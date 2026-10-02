use super::pi::sync_pi_auth_from;
use crate::test_support::*;
use crate::*;
use std::fs;
use std::path::Path;

#[test]
fn pi_auth_is_staged_in_the_managed_directory() {
    let temp = temporary_directory();
    let source = temp.join("host-auth.json");
    fs::write(&source, b"{\"openai-codex\":{}}\n").unwrap();
    let store = Store::open(temp.join("store"));
    sync_pi_auth_from(&store, source).unwrap();
    let managed_auth = store.pi_agent_path().join("auth.json");
    assert_eq!(fs::read(&managed_auth).unwrap(), b"{\"openai-codex\":{}}\n");
    validate_pi_agent(&store).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&managed_auth).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn pi_docker_arguments_mount_managed_auth_and_select_model() {
    let mut spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--no-session']\nmodel = 'anthropic/claude-opus-4-6:high'\n").unwrap();
    spec.startup_command = vec!["env".to_owned(), "MY_VAR=1".to_owned()];
    spec.runtime_mounts = vec![RuntimeMount {
        kind: RuntimeMountType::Volume,
        source: "shared-tools".to_owned(),
        target: "/tool-cache".to_owned(),
        read_only: false,
    }];
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let arguments = docker_arguments(
        &store,
        &spec,
        &temp,
        &temp,
        "example",
        "run-test",
        "image-id",
        &["-p".to_owned(), "complete the assigned work".to_owned()],
    )
    .unwrap();
    assert!(arguments.windows(2).any(|pair| pair == ["--env", "TERM"]));
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--env", "COLORTERM"])
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "PI_CODING_AGENT_DIR=/pi-agent")
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "PI_SKIP_VERSION_CHECK=1")
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument.contains("target=/pi-agent"))
    );
    assert!(arguments.windows(2).any(|pair| pair == ["pi", "--model"]));
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--model", "anthropic/claude-opus-4-6:high"])
    );
    assert!(arguments.ends_with(&["-p".to_owned(), "complete the assigned work".to_owned()]));
    assert!(
        !arguments.iter().any(|argument| argument == "--no-session"),
        "Gardr must drop --no-session for the pi adapter so a transcript is written"
    );
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--session", "/gardr-transcript/session.jsonl"])
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "type=volume,source=shared-tools,target=/tool-cache")
    );
    assert!(
        arguments
            .windows(3)
            .any(|arguments| arguments == ["env", "MY_VAR=1", "pi"]),
        "the global startup command must wrap the harness command"
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument.contains("target=/gardr-transcript"))
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn pi_dispatch_args_reject_session_management_flags() {
    let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
    assert!(
        validate_dispatch_harness_args(&spec, &["--session".to_owned(), "x".to_owned()])
            .unwrap_err()
            .contains("session")
    );
    assert!(validate_dispatch_harness_args(&spec, &["--continue".to_owned()]).is_err());
    assert!(validate_dispatch_harness_args(&spec, &["-r".to_owned()]).is_err());
    assert!(validate_dispatch_harness_args(&spec, &["--no-session".to_owned()]).is_ok());
}

#[test]
fn pi_name_flag_does_not_conflict_with_the_injected_session_flag() {
    // --name (session display name) doesn't conflict with Gardr's injected --session, so
    // specs and dispatch args using it must keep validating (no migration required).
    let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--name', 'my-session']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
    assert!(validate_spec(&spec).is_ok());
    assert!(validate_dispatch_harness_args(&spec, &["--name".to_owned(), "x".to_owned()]).is_ok());
    assert!(validate_dispatch_harness_args(&spec, &["-n".to_owned(), "x".to_owned()]).is_ok());
}

#[test]
fn pi_command_reserves_session_management_flags() {
    let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi', '--fork', 'x']\nmodel = 'anthropic/claude-opus-4-6'\n").unwrap();
    assert!(
        validate_spec(&spec)
            .unwrap_err()
            .contains("dispatch or model-selection")
    );
}

#[test]
fn transcript_directory_is_created_for_pi_runs_and_recorded_in_state() {
    let temp = temporary_directory();
    let source = temp.join("spec.toml");
    fs::write(&source, spec()).unwrap();
    let store = Store::open(temp.join("store"));
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    store.add_spec("build", &source).unwrap();
    let workspace = temp.join("workspace");
    fs::create_dir_all(workspace.join("dispatches")).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
    let (record, _spec) = store
        .create_run(overrides_for(&workspace), Some("build"), vec![])
        .unwrap();
    let transcript_path = record.transcript_path.clone().unwrap();
    assert!(transcript_path.contains(&record.id));
    assert!(Path::new(&transcript_path).parent().unwrap().is_dir());
    assert!(record.stdout_path.ends_with("stdout.log"));
    assert!(record.stderr_path.ends_with("stderr.log"));
    let reloaded = store.read_run(&record.id).unwrap();
    assert_eq!(reloaded.transcript_path, record.transcript_path);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn transcript_usage_is_summed_from_session_jsonl() {
    let temp = temporary_directory();
    let transcript = temp.join("session.jsonl");
    fs::write(
            &transcript,
            concat!(
                "{\"type\":\"session\",\"version\":3,\"id\":\"s\"}\n",
                "{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":1,\"cacheWrite\":2,\"totalTokens\":18,\"cost\":{\"input\":0.01,\"output\":0.02,\"cacheRead\":0.0,\"cacheWrite\":0.0,\"total\":0.03}}}}\n",
                "{\"type\":\"compaction\",\"id\":\"b\",\"parentId\":\"a\",\"summary\":\"s\",\"tokensBefore\":1,\"usage\":{\"input\":1,\"output\":1,\"cacheRead\":0,\"cacheWrite\":0,\"cost\":{\"total\":0.001}}}\n",
            ),
        )
        .unwrap();
    let (usage, skipped) = read_pi_transcript_usage(&transcript).unwrap();
    let usage = usage.unwrap();
    assert_eq!(usage.input_tokens, 11);
    assert_eq!(usage.output_tokens, 6);
    assert!((usage.total_cost - 0.031).abs() < 1e-9);
    assert_eq!(skipped, 0);
    let (missing_usage, missing_skipped) =
        read_pi_transcript_usage(&temp.join("missing.jsonl")).unwrap();
    assert!(missing_usage.is_none());
    assert_eq!(missing_skipped, 0);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn transcript_usage_tolerates_a_torn_trailing_line_but_flags_other_unparsable_lines() {
    let temp = temporary_directory();
    let transcript = temp.join("session.jsonl");
    fs::write(
            &transcript,
            concat!(
                "not json at all\n",
                "{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input\":10,\"output\":5,\"cost\":{\"total\":0.05}}}}\n",
                "{\"type\":\"message\",\"id\":\"b\",\"parentId\":\"a\",\"message\":{\"role\":\"assistant\",\"con", // torn trailing write
            ),
        )
        .unwrap();
    let (usage, skipped) = read_pi_transcript_usage(&transcript).unwrap();
    let usage = usage.expect("usage collected despite bad lines");
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(
        skipped, 1,
        "the leading unparsable line is flagged; the torn trailing line is tolerated"
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn transcript_usage_surfaces_io_errors_distinct_from_no_transcript_yet() {
    let temp = temporary_directory();
    // A directory in place of the transcript file yields a real IO error ("Is a directory")
    // distinct from `NotFound`; this test runs as root, so permission bits alone wouldn't
    // reliably block a read.
    let transcript = temp.join("session.jsonl");
    fs::create_dir(&transcript).unwrap();
    let result = read_pi_transcript_usage(&transcript);
    assert!(
        result.is_err(),
        "an unreadable transcript must surface an error, not be conflated with 'no spend yet'"
    );
    let (missing_usage, missing_skipped) =
        read_pi_transcript_usage(&temp.join("missing.jsonl")).unwrap();
    assert!(
        missing_usage.is_none(),
        "a missing transcript is 'no spend yet', not an error"
    );
    assert_eq!(missing_skipped, 0);
    fs::remove_dir_all(temp).unwrap();
}
