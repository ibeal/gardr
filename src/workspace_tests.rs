use crate::test_support::*;
use crate::*;
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn workspace_validation_accepts_arbitrary_directory_contents() {
    let temp = temporary_directory();
    let workspace = temp.join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("arbitrary-input"), b"content").unwrap();
    assert!(validate_workspace(&workspace).unwrap().is_dir());
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn workspace_validation_rejects_non_directories() {
    let temp = temporary_directory();
    let file = temp.join("workspace");
    fs::write(&file, b"not a directory").unwrap();
    assert!(validate_workspace(&file).unwrap_err().contains("directory"));
    assert!(validate_workspace(&temp.join("missing")).is_err());
    fs::remove_dir_all(temp).unwrap();
}
#[test]
fn one_shot_sources_use_current_or_explicit_directories_and_threads_reuse_them() {
    let temp = temporary_directory();
    let store = Store::open(temp.join("store"));
    let image_source = temp.join("agent.toml");
    fs::write(
        &image_source,
        b"version = 1\nharnesses = ['pi']\n[source]\nreference = 'agent:latest'\n",
    )
    .unwrap();
    store.add_image("agent", &image_source).unwrap();
    fs::create_dir_all(store.pi_agent_path()).unwrap();
    fs::write(store.pi_agent_path().join("auth.json"), b"{}").unwrap();
    set_private_directory(&store.pi_agent_path()).unwrap();
    set_private_file(&store.pi_agent_path().join("auth.json")).unwrap();
    fs::create_dir_all(store.config_path().parent().unwrap()).unwrap();
    fs::write(
        store.config_path(),
        b"image = 'agent'\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n",
    )
    .unwrap();
    let repo = temp.join("repo");
    fs::create_dir(&repo).unwrap();
    let ask = temp.join("ask.md");
    fs::write(&ask, b"do the work").unwrap();
    let bin = temp.join("bin");
    let _docker = FakeDocker::install(
        "#!/bin/sh\nif [ \"$1\" = image ]; then echo sha256:agent; exit 0; fi\nif [ \"$1\" = run ]; then echo container-id; exit 0; fi\nexit 1\n",
        &bin,
    );

    let mut first = store
        .start_one_shot(
            StartRequest {
                repo: None,
                url: None,
                thread: Some("work".to_owned()),
                ask_file: Some(ask),
                current_dir: repo.clone(),
            },
            RuntimeOverrides::default(),
            vec![],
        )
        .unwrap();
    assert_eq!(
        first.source.as_ref().unwrap().path,
        repo.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(
        fs::read(first.input_path.as_ref().unwrap()).unwrap(),
        b"do the work"
    );
    assert!(store.threads_path().join("work/CONTINUITY.md").is_file());
    assert!(
        store
            .run_path(&first.id)
            .unwrap()
            .join("metadata.json")
            .is_file()
    );
    assert_eq!(first.effective.image.value, "agent");

    let second = store
        .start_one_shot(
            StartRequest {
                repo: None,
                url: None,
                thread: Some("work".to_owned()),
                ask_file: None,
                current_dir: temp.clone(),
            },
            RuntimeOverrides::default(),
            vec![],
        )
        .unwrap();
    assert_eq!(second.source.as_ref().unwrap().path, first.workspace);
    assert_eq!(second.effective.image.value, first.effective.image.value);
    assert_eq!(second.effective.model.value, first.effective.model.value);
    assert_ne!(second.transcript_path, first.transcript_path);

    let original_input = first.input_path.clone();
    let original_transcript = first.transcript_path.clone();
    first.state = RunState::Failed;
    first.container = None;
    first.failure = Some("agent crashed".to_owned());
    save_record(&store.run_path(&first.id).unwrap(), &first).unwrap();
    let mut resumed = store.resume(&first.id).unwrap();
    assert_eq!(resumed.attempt, 2);
    assert_eq!(resumed.input_path, original_input);
    assert_eq!(resumed.transcript_path, original_transcript);
    assert_eq!(resumed.source.as_ref().unwrap().path, first.workspace);

    resumed.state = RunState::Stopped;
    resumed.exit_status = Some(0);
    resumed.container = None;
    save_record(&store.run_path(&resumed.id).unwrap(), &resumed).unwrap();
    assert!(
        store
            .resume(&resumed.id)
            .unwrap_err()
            .contains("completed successfully")
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn url_sources_clone_into_unique_persistent_workspaces() {
    let temp = temporary_directory();
    let origin = temp.join("origin");
    let init = Command::new("git")
        .args(["init", "--quiet"])
        .arg(&origin)
        .status()
        .unwrap();
    assert!(init.success());
    let store = Store::open(temp.join("store"));
    let first = store.clone_source(origin.to_str().unwrap()).unwrap();
    let second = store.clone_source(origin.to_str().unwrap()).unwrap();
    assert!(Path::new(&first.path).is_dir());
    assert_ne!(first.workspace_id, second.workspace_id);
    assert_ne!(first.workspace_path, second.workspace_path);
    assert_eq!(first.url.as_deref(), origin.to_str());
    fs::remove_dir_all(temp).unwrap();
}
