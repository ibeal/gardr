use crate::test_support::*;
use crate::*;
use std::fs;
use std::path::PathBuf;

#[test]
fn credentials_can_be_registered_listed_and_removed_without_exposing_values() {
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    store.set_credential("gh-pat", b"super-secret").unwrap();
    assert_eq!(store.list_credentials().unwrap(), ["gh-pat".to_owned()]);
    assert_eq!(
        fs::read(store.credential_path("gh-pat").unwrap()).unwrap(),
        b"super-secret"
    );
    // Re-registering (upsert) rotates the value rather than failing like spec/image `add`.
    store.set_credential("gh-pat", b"rotated-secret").unwrap();
    assert_eq!(
        fs::read(store.credential_path("gh-pat").unwrap()).unwrap(),
        b"rotated-secret"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(store.credentials_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(store.credential_path("gh-pat").unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    store.remove_credential("gh-pat").unwrap();
    assert_eq!(store.list_credentials().unwrap(), Vec::<String>::new());
    assert!(store.remove_credential("gh-pat").is_err());
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn credential_environment_entries_support_plain_string_and_mapped_table_forms() {
    let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = ['GH_TOKEN', {name = 'GH_TOKEN2', from = 'gh-pat'}, {name = 'GH_TOKEN3'}]\n").unwrap();
    validate_spec(&spec).unwrap();
    let refs = &spec.credentials.environment;
    assert_eq!(refs[0].env_name(), "GH_TOKEN");
    assert_eq!(refs[0].registry_key(), "GH_TOKEN");
    assert_eq!(refs[1].env_name(), "GH_TOKEN2");
    assert_eq!(refs[1].registry_key(), "gh-pat");
    assert_eq!(refs[2].env_name(), "GH_TOKEN3");
    assert_eq!(refs[2].registry_key(), "GH_TOKEN3");
}

#[test]
fn docker_arguments_resolves_credentials_from_the_store_via_from_and_env_file() {
    let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = [{name = 'GH_TOKEN', from = 'gh-pat'}]\n").unwrap();
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    store.set_credential("gh-pat", b"super-secret\n").unwrap();
    let arguments = docker_arguments(
        &store,
        &spec,
        &temp,
        &temp,
        "example",
        "run-test",
        "image-id",
        &[],
    )
    .unwrap();
    let env_file_index = arguments
        .iter()
        .position(|argument| argument == "--env-file")
        .expect("--env-file must be present");
    let env_file = PathBuf::from(&arguments[env_file_index + 1]);
    let contents = fs::read_to_string(&env_file).unwrap();
    assert_eq!(contents, "GH_TOKEN=super-secret\n");
    assert!(
        !arguments
            .iter()
            .any(|argument| argument.contains("super-secret")),
        "the secret value must not appear directly on the docker argv: {arguments:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&env_file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn docker_arguments_fails_clearly_for_an_unregistered_credential() {
    let spec = parse_spec(b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = ['GH_TOKEN']\n").unwrap();
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let error = docker_arguments(
        &store,
        &spec,
        &temp,
        &temp,
        "example",
        "run-test",
        "image-id",
        &[],
    )
    .unwrap_err();
    assert!(
        error.contains("required credential is not registered: GH_TOKEN"),
        "{error}"
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn credential_set_rejects_multi_line_and_comment_looking_values() {
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let error = store
        .set_credential(
            "ssh-key",
            b"-----BEGIN KEY-----\nsecret-bytes\n-----END KEY-----\n",
        )
        .unwrap_err();
    assert!(error.contains("single line"), "{error}");
    assert!(!store.credential_path("ssh-key").unwrap().exists());

    let error = store
        .set_credential("comment", b"#not-a-comment")
        .unwrap_err();
    assert!(error.contains('#'), "{error}");

    // A single trailing newline (a common artifact of files written by editors) is tolerated.
    store.set_credential("token", b"plain-value\n").unwrap();
    assert_eq!(
        fs::read(store.credential_path("token").unwrap()).unwrap(),
        b"plain-value\n"
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn atomic_write_does_not_clobber_a_sibling_credential_whose_name_is_a_prefix() {
    // `validate_name` allows `.` in a credential name; registering `a.b` must not go through a
    // temp path (like the old `<name>.new`) that collides with a distinct, already-registered
    // credential `a`.
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    store.set_credential("a", b"a-secret").unwrap();
    store.set_credential("a.b", b"a-b-secret").unwrap();
    assert_eq!(
        fs::read(store.credential_path("a").unwrap()).unwrap(),
        b"a-secret",
        "registering a.b must not clobber the unrelated credential a"
    );
    assert_eq!(
        fs::read(store.credential_path("a.b").unwrap()).unwrap(),
        b"a-b-secret"
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn credentials_env_does_not_survive_cleanup() {
    let temp = temporary_directory();
    let script = "#!/bin/sh\nset -eu\ncase \"$1\" in\n  inspect) echo 'false 0' ;;\n  logs) exit 0 ;;\n  rm) exit 0 ;;\n  *) echo \"unexpected docker command: $1\" >&2; exit 1 ;;\nesac\n";
    let _docker = FakeDocker::install(script, &temp.join("bin"));

    let store = Store::open(temp.join("store"));
    let source = temp.join("spec.toml");
    fs::write(&source, spec()).unwrap();
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    store.add_spec("build", &source).unwrap();
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
    let (mut record, _) = store
        .create_run(overrides_for(&workspace), Some("build"), vec![])
        .unwrap();
    let directory = store.run_path(&record.id).unwrap();
    // Simulate a run that resolved credentials into the plaintext env-file, as `launch` would
    // before invoking `docker run` (independent of exercising the credential-resolution path).
    let credentials_env = directory.join("credentials.env");
    atomic_write(&credentials_env, b"GH_TOKEN=super-secret\n").unwrap();
    assert!(credentials_env.exists());
    record.state = RunState::Stopped;
    record.container = Some("fake-container".to_owned());
    save_record(&directory, &record).unwrap();

    store.cleanup(&record.id).unwrap();

    assert!(
        !credentials_env.exists(),
        "cleanup must remove the plaintext resolved-credentials env-file"
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn launch_removes_credentials_env_after_docker_run_returns() {
    let temp = temporary_directory();
    let script = "#!/bin/sh\nset -eu\nif [ \"$1\" = 'image' ] && [ \"$2\" = 'inspect' ]; then\n  if [ \"$3\" = '--format' ]; then\n    echo 'sha256:fakeid'\n  fi\n  exit 0\nfi\nif [ \"$1\" = 'volume' ]; then\n  echo \"volume-created\"\n  exit 0\nfi\nif [ \"$1\" = 'run' ]; then\n  echo 'fake-container'\n  exit 0\nfi\necho \"unexpected docker command: $*\" >&2\nexit 1\n";
    let _docker = FakeDocker::install(script, &temp.join("bin"));

    let store = Store::open(temp.join("store"));
    let source = temp.join("spec.toml");
    fs::write(
            &source,
            b"version = 1\n[image]\nname = 'example'\n[sandbox]\nnetwork = 'none'\n[harness]\nadapter = 'pi'\ncommand = ['pi']\nmodel = 'anthropic/claude-opus-4-6'\n[credentials]\nenvironment = ['GH_TOKEN']\n",
        )
        .unwrap();
    let image_source = temp.join("example.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'example:latest'\n",
    )
    .unwrap();
    store.add_image("example", &image_source).unwrap();
    store.add_spec("build", &source).unwrap();
    store.set_credential("GH_TOKEN", b"super-secret").unwrap();
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();

    let record = store
        .start(overrides_for(&workspace), Some("build"), vec![])
        .unwrap();
    assert!(matches!(record.state, RunState::Running));
    let directory = store.run_path(&record.id).unwrap();
    assert!(
        !directory.join("credentials.env").exists(),
        "credentials.env must not survive past docker run returning"
    );
    fs::remove_dir_all(temp).unwrap();
}
