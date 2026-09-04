use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use gardr::{Store, validate_workspace};

fn main() {
    if let Err(error) = run() {
        eprintln!("gardr: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let command_line_root = option(&mut args, "--root").map(PathBuf::from);
    match take(&mut args)?.as_str() {
        "help" | "--help" => {
            reject_extra(&args)?;
            print!("{HELP}");
            Ok(())
        }
        "docs" => {
            reject_extra(&args)?;
            print!("{DOCS}");
            Ok(())
        }
        "spec" if subcommand_help(&args) => {
            print!("{SPEC_HELP}");
            Ok(())
        }
        "run" if subcommand_help(&args) => {
            print!("{RUN_HELP}");
            Ok(())
        }
        command @ ("spec" | "run") => {
            let root = root(
                command_line_root,
                env::var_os("GARDR_ROOT").map(PathBuf::from),
                env::var_os("HOME"),
            )?;
            let store = Store::open(root);
            match command {
                "spec" => spec(&store, args),
                "run" => run_command(&store, args),
                _ => unreachable!(),
            }
        }
        _ => Err(usage()),
    }
}

fn spec(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "add" => {
            let name = take(&mut args)?;
            let file =
                option(&mut args, "--file").ok_or_else(|| "--file is required".to_owned())?;
            reject_extra(&args)?;
            print_json(&store.add_spec(&name, PathBuf::from(file).as_path())?)
        }
        "list" => {
            reject_extra(&args)?;
            print_json(&store.list_specs()?)
        }
        "show" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            let (_, identity, content) = store.read_spec(&name)?;
            println!("{}", String::from_utf8_lossy(&content));
            eprintln!("sha256={}", identity.sha256);
            Ok(())
        }
        "validate" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            let (_, identity, _) = store.read_spec(&name)?;
            print_json(&identity)
        }
        _ => Err(usage()),
    }
}

fn run_command(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "start" => {
            let workspace = option(&mut args, "--workspace")
                .ok_or_else(|| "--workspace is required".to_owned())?;
            let spec =
                option(&mut args, "--spec").ok_or_else(|| "--spec is required".to_owned())?;
            reject_extra(&args)?;
            print_json(&store.start(PathBuf::from(workspace).as_path(), &spec)?)
        }
        "observe" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.observe(&id)?)
        }
        "resume" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.resume(&id)?)
        }
        "stop" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            print_json(&store.stop(&id)?)
        }
        "cleanup" => {
            let id = take(&mut args)?;
            reject_extra(&args)?;
            store.cleanup(&id)?;
            print_json(&serde_json::json!({"id": id, "cleaned": true}))
        }
        "validate-workspace" => {
            let workspace = take(&mut args)?;
            reject_extra(&args)?;
            print_json(
                &serde_json::json!({"workspace": validate_workspace(PathBuf::from(workspace).as_path())?}),
            )
        }
        _ => Err(usage()),
    }
}

fn option(args: &mut Vec<String>, flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| {
            args.remove(index);
            (index < args.len()).then(|| args.remove(index))
        })
}
fn take(args: &mut Vec<String>) -> Result<String, String> {
    if args.is_empty() {
        Err(usage())
    } else {
        Ok(args.remove(0))
    }
}
fn reject_extra(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(format!("unexpected arguments: {}", args.join(" ")))
    }
}
fn subcommand_help(args: &[String]) -> bool {
    args.is_empty() || matches!(args, [argument] if argument == "help" || argument == "--help")
}
fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|error| error.to_string())?
    );
    Ok(())
}
fn usage() -> String {
    HELP.trim_end().to_owned()
}

const HELP: &str = "Durable sandbox execution for prepared agent workspaces\n\nUsage: gardr [--root <path>] <COMMAND>\n\nCommands:\n  spec  Manage sandbox specifications\n  run   Manage workspace runs\n  docs  Print built-in guidance and examples\n  help  Print this message\n\nSpec commands:\n  add, list, show, validate\n\nRun commands:\n  start, observe, resume, stop, cleanup, validate-workspace\n\nRun `gardr spec --help` or `gardr run --help` for command details.\n\nRoot:\n  ~/.gardr by default; GARDR_ROOT or --root overrides it\n";

const SPEC_HELP: &str = "Manage sandbox specifications\n\nUsage: gardr [--root <path>] spec <COMMAND>\n\nCommands:\n  add       Validate and store a spec: gardr spec add <name> --file <path>\n  list      Print stored spec names as JSON\n  show      Print a stored spec; writes its SHA-256 to stderr\n  validate  Print a stored spec's identity as JSON\n\nUse `gardr docs` for the specification format.\n";

const RUN_HELP: &str = "Manage prepared workspace runs\n\nUsage: gardr [--root <path>] run <COMMAND>\n\nCommands:\n  start               Start a sealed workspace: --workspace <path> --spec <name>\n  observe             Reconcile and print a run: <run-id>\n  resume              Restart a stopped or failed run: <run-id>\n  stop                Stop a running run: <run-id>\n  cleanup             Remove a non-running container: <run-id>\n  validate-workspace  Validate a prepared, sealed workspace: <path>\n\nRun commands return one JSON document. Use `gardr docs` for lifecycle details.\n";

const DOCS: &str = r#"# gardr — durable sandbox execution

Gardr runs a prepared, sealed workspace under one named sandbox specification. It owns the
specification store, Docker container lifecycle, and durable run records. It does not create a
workspace, prepare an agent dispatch, or determine whether the agent completed its assignment.

## Root and store

Gardr uses `~/.gardr` by default. Set `GARDR_ROOT` or pass `--root <path>` to select another root;
`--root` wins. The root contains `specs/`, `runs/`, and optional approved `mounts/` and `images/`
directories. Gardr creates `specs/` and `runs/` when they are first needed.

## Commands

```text
gardr spec add <name> --file <path>      # validate and store an immutable spec
gardr spec list                          # JSON list of stored names
gardr spec show <name>                   # print the TOML and its SHA-256 to stderr
gardr spec validate <name>               # JSON identity for one stored spec

gardr run validate-workspace <path>      # validate a prepared, sealed workspace
gardr run start --workspace <path> --spec <name>
gardr run observe <run-id>               # reconcile and return current run state
gardr run resume <run-id>                # restart a stopped or failed run
gardr run stop <run-id>                  # stop a running container
gardr run cleanup <run-id>               # remove a non-running container; idempotent
```

All command results except `spec show` are one JSON document, intended for an orchestrator to read.

## A sandbox specification

```toml
version = 1

[image]
reference = "ghcr.io/example/agent:latest"
# build_context = "agent-image" # optional: <root>/images/agent-image/Dockerfile

[sandbox]
network = "none" # or "bridge"

[harness]
adapter = "claude-code"
command = ["claude", "-p", "complete the assigned work"]

[[mounts]]
name = "tools"                  # resolves only to <root>/mounts/tools
target = "/tools"
read_only = true

[credentials]
environment = ["GH_TOKEN"]     # names only; values are never stored
```

Spec names, mount names, and build-context names select direct children of the approved root
directories. A mount cannot target `/workspace`, and Gardr rejects unknown fields, duplicate mount
names or targets, invalid container paths, and credential values.

## Workspace and lifecycle

`run start` accepts only a prepared workspace containing one or more sealed entries under
`dispatches/`. Gardr mounts that workspace at `/workspace`, freezes the selected spec, records the
workspace seal and approved mounts, then starts Docker. It never falls back to host execution.

Each run has a directory at `<root>/runs/<run-id>/` containing the frozen `spec.toml`, resolved
configuration, run state, mount lock, runner log, and artifacts directory. `resume` revalidates the
workspace seal, frozen spec, and approved mounts before launching again. `cleanup` is terminal and
refuses a running run; stop it first.
"#;

fn root(
    command_line_root: Option<PathBuf>,
    environment_root: Option<PathBuf>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    command_line_root
        .or(environment_root)
        .or_else(|| home.map(|home| PathBuf::from(home).join(".gardr")))
        .ok_or_else(|| "unable to resolve the default root: HOME is not set".to_owned())
}

#[cfg(test)]
mod tests {
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
        assert!(HELP.contains("docs  Print built-in guidance"));
    }

    #[test]
    fn nested_help_is_available_without_a_store() {
        assert!(subcommand_help(&[]));
        assert!(subcommand_help(&["--help".to_owned()]));
        assert!(!subcommand_help(&["list".to_owned()]));
        assert!(SPEC_HELP.contains("add"));
        assert!(RUN_HELP.contains("validate-workspace"));
    }
}
