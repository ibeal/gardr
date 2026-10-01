//! Integration coverage for the Heimr-mounted launch: one mounted-directory workspace and one
//! cloned workspace, each exercised through an interactive-style start, stop, and resume so the
//! Pi session survives a container that stopped.
//!
//! Requires Docker and a real `~/.pi/agent/auth.json` (Gardr's `sync_pi_auth` copies it into the
//! managed store on first use, matching every other adapter-`pi` run in this repository). Not run
//! by default:
//!
//! ```sh
//! cargo test --test heimr_environment -- --ignored
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gardr::{RuntimeOverrides, Store};

const TEST_IMAGE: &str = "gardr-heimr-environment-test:latest";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn temporary_directory() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "gardr-heimr-environment-test-{}-{}",
        now_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn run(command: &mut Command) {
    let output = command.output().expect("command failed to launch");
    assert!(
        output.status.success(),
        "{:?} failed: stdout={} stderr={}",
        command,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Builds (once per test binary invocation) a tiny image whose `pi` is a shell script standing in
/// for the real harness: it records its working directory and argv into `/agent-state`, which is
/// host-visible through Gardr's `/agent-state` bind mount, then sleeps so the container stays
/// `Running` long enough to inspect and to be stopped/resumed deliberately.
fn build_test_image() {
    let context = temporary_directory();
    fs::write(
        context.join("Dockerfile"),
        "FROM busybox:latest\nCOPY pi /usr/local/bin/pi\nRUN chmod +x /usr/local/bin/pi\n",
    )
    .unwrap();
    fs::write(
        context.join("pi"),
        "#!/bin/sh\nset -eu\ncount_file=/agent-state/invocation-count\ncount=0\nif [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi\ncount=$((count + 1))\necho \"$count\" > \"$count_file\"\n{\n  echo \"cwd=$(pwd)\"\n  echo \"argv=$*\"\n} > \"/agent-state/invocation-$count.log\"\nexec sleep 600\n",
    )
    .unwrap();
    run(Command::new("docker")
        .args(["build", "-t", TEST_IMAGE, "."])
        .current_dir(&context));
    fs::remove_dir_all(context).unwrap();
}

fn new_store(root: &Path) -> Store {
    let store = Store::open(root.join("store"));
    let image_source = root.join("image.toml");
    fs::write(
        &image_source,
        format!("version = 1\nharnesses = ['pi']\n[source]\nreference = '{TEST_IMAGE}'\n"),
    )
    .unwrap();
    store.add_image("test-agent", &image_source).unwrap();
    fs::write(
        store.config_path(),
        "image = 'test-agent'\n[harness]\nadapter = 'pi'\nmodel = 'anthropic/claude-opus-4-6'\n",
    )
    .unwrap();
    store
}

fn mounted_directory_workspace(root: &Path, name: &str) -> PathBuf {
    let heimr_root = root.join("heimr");
    let source = root.join(format!("{name}-source"));
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("README.md"), b"mounted directory fixture\n").unwrap();
    let workspace = heimr::Workspace::open(&heimr_root, name).unwrap();
    workspace.create().unwrap();
    populate_environment(&workspace);
    workspace.set_mounted_source(&source).unwrap();
    workspace.root
}

fn cloned_workspace(root: &Path, name: &str) -> PathBuf {
    let heimr_root = root.join("heimr");
    let source = root.join(format!("{name}-source"));
    fs::create_dir_all(&source).unwrap();
    for (program, args) in [
        ("git", vec!["init", "--quiet", "."]),
        ("git", vec!["config", "user.email", "test@example.com"]),
        ("git", vec!["config", "user.name", "Gardr Test"]),
    ] {
        run(Command::new(program).args(args).current_dir(&source));
    }
    fs::write(source.join("README.md"), b"cloned workspace fixture\n").unwrap();
    run(Command::new("git").args(["add", "."]).current_dir(&source));
    run(Command::new("git")
        .args(["commit", "--quiet", "-m", "initial"])
        .current_dir(&source));
    let workspace = heimr::Workspace::open(&heimr_root, name).unwrap();
    workspace.create().unwrap();
    populate_environment(&workspace);
    workspace.prepare_repository(&source).unwrap();
    workspace.root
}

fn populate_environment(workspace: &heimr::Workspace) {
    workspace
        .put_environment_file(Path::new("AGENTS.md"), b"test fixture AGENTS.md\n")
        .unwrap();
    workspace
        .put_environment_file(Path::new("task.md"), b"test fixture task\n")
        .unwrap();
    workspace
        .put_environment_file(Path::new("prompt.md"), b"test fixture prompt\n")
        .unwrap();
    workspace
        .put_environment_file(Path::new("context/INDEX.md"), b"test fixture index\n")
        .unwrap();
}

fn state_path(workspace: &Path) -> PathBuf {
    workspace.join("state")
}

fn exercise_interactive_lifecycle(root: &Path, workspace: &Path) {
    let store = new_store(root);
    let overrides = RuntimeOverrides {
        workspace: Some(workspace.display().to_string()),
        ..Default::default()
    };
    let record = store.start(overrides, None, vec![]).unwrap();
    assert_eq!(record.state.to_string(), "running");
    std::thread::sleep(Duration::from_millis(500));

    let first_invocation = fs::read_to_string(state_path(workspace).join("invocation-1.log"))
        .expect("the fake pi harness must have written its first invocation log to /agent-state");
    assert!(
        first_invocation.contains("cwd=/repo"),
        "the agent must start in /repo: {first_invocation}"
    );
    assert!(
        first_invocation.contains("--model")
            && first_invocation.contains("anthropic/claude-opus-4-6"),
        "the model must be injected: {first_invocation}"
    );
    assert!(
        first_invocation.contains("--session")
            && first_invocation.contains("/gardr-transcript/session.jsonl"),
        "the Pi session path must be the durable per-run transcript location: {first_invocation}"
    );
    assert!(
        first_invocation.contains("/agent/AGENTS.md")
            && first_invocation.contains("/agent/task.md")
            && first_invocation.contains("/agent/prompt.md")
            && first_invocation.contains("/agent/context/INDEX.md"),
        "the invariant startup instruction must point at every curated environment file: {first_invocation}"
    );

    // Stop and resume: an interactive session must be attachable/resumable after its container
    // stops, reusing the same frozen mount plan and the same durable transcript path rather than
    // a fresh prompt-only continuation.
    let stopped = store.stop(&record.id).unwrap();
    assert_eq!(stopped.state.to_string(), "stopped");
    let resumed = store.resume(&record.id).unwrap();
    assert_eq!(resumed.state.to_string(), "running");
    assert_eq!(resumed.transcript_path, record.transcript_path);
    std::thread::sleep(Duration::from_millis(500));

    let second_invocation = fs::read_to_string(state_path(workspace).join("invocation-2.log"))
        .expect("resume must relaunch the harness against the same /agent-state");
    assert!(second_invocation.contains("cwd=/repo"));
    assert!(second_invocation.contains("/gardr-transcript/session.jsonl"));

    store.stop(&record.id).unwrap();
    store.cleanup(&record.id).unwrap();
}

#[test]
#[ignore = "requires Docker and ~/.pi/agent/auth.json"]
fn mounted_directory_workspace_is_resumable_after_its_container_stops() {
    build_test_image();
    let root = temporary_directory();
    let workspace = mounted_directory_workspace(&root, "mounted");
    exercise_interactive_lifecycle(&root, &workspace);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires Docker and ~/.pi/agent/auth.json"]
fn cloned_workspace_is_resumable_after_its_container_stops() {
    build_test_image();
    let root = temporary_directory();
    let workspace = cloned_workspace(&root, "cloned");
    exercise_interactive_lifecycle(&root, &workspace);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires Docker and ~/.pi/agent/auth.json"]
fn autonomous_launch_reuses_the_same_preparation_as_interactive_launch() {
    build_test_image();
    let root = temporary_directory();
    let workspace = mounted_directory_workspace(&root, "autonomous");
    let store = new_store(&root);
    let overrides = RuntimeOverrides {
        workspace: Some(workspace.display().to_string()),
        ..Default::default()
    };
    let record = store.start(overrides, None, vec![]).unwrap();
    assert_eq!(record.state.to_string(), "running");
    std::thread::sleep(Duration::from_millis(500));

    // Non-interactive consumers read the same stdout/stderr/transcript/HANDOFF.json evidence
    // retained for every run; `observe` must keep reporting live state without any separate
    // autonomous code path.
    let observed = store.observe(&record.id).unwrap();
    assert_eq!(observed.state.to_string(), "running");
    assert!(Path::new(&observed.stdout_path).parent().unwrap().is_dir());
    assert!(
        observed
            .transcript_path
            .as_ref()
            .is_some_and(|path| Path::new(path).parent().unwrap().is_dir())
    );

    store.stop(&record.id).unwrap();
    store.cleanup(&record.id).unwrap();
    assert!(Path::new(&observed.stdout_path).is_file());
    assert!(Path::new(&observed.stderr_path).is_file());
    fs::remove_dir_all(root).unwrap();
}
