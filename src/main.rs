use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

use gardr::{StartRequest, Store, validate_workspace};

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
        "image" if subcommand_help(&args) => {
            print!("{IMAGE_HELP}");
            Ok(())
        }
        "run" if subcommand_help(&args) => {
            print!("{RUN_HELP}");
            Ok(())
        }
        "credential" if subcommand_help(&args) => {
            print!("{CREDENTIAL_HELP}");
            Ok(())
        }
        command @ ("spec" | "image" | "run" | "credential") => {
            let root = root(
                command_line_root,
                env::var_os("GARDR_ROOT").map(PathBuf::from),
                env::var_os("HOME"),
            )?;
            let store = Store::open(root);
            match command {
                "spec" => Err("per-spec runtime configuration has been removed; configure runtime policy in <root>/config.toml".to_owned()),
                "image" => image(&store, args),
                "run" => run_command(&store, args),
                "credential" => credential(&store, args),
                _ => unreachable!(),
            }
        }
        _ => Err(usage()),
    }
}

fn image(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "add" => {
            let name = take(&mut args)?;
            let file =
                option(&mut args, "--file").ok_or_else(|| "--file is required".to_owned())?;
            reject_extra(&args)?;
            print_json(&store.add_image(&name, PathBuf::from(file).as_path())?)
        }
        "list" => {
            reject_extra(&args)?;
            print_json(&store.list_images()?)
        }
        "show" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            let (_, identity, content) = store.read_image(&name)?;
            println!("{}", String::from_utf8_lossy(&content));
            eprintln!("sha256={}", identity.sha256);
            Ok(())
        }
        "validate" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            let (image, identity, _) = store.read_image(&name)?;
            if let Some(context) = &image.source.build_context {
                let context = store.images_path().join(context);
                if !context.join("Dockerfile").is_file() {
                    return Err(format!(
                        "image build context is missing Dockerfile: {}",
                        context.display()
                    ));
                }
            }
            print_json(&identity)
        }
        _ => Err(usage()),
    }
}

fn credential(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "set" => {
            let name = take(&mut args)?;
            let file = option(&mut args, "--file");
            let stdin = flag(&mut args, "--stdin");
            reject_extra(&args)?;
            let value = match (file, stdin) {
                (Some(_), true) => {
                    return Err("--file and --stdin are mutually exclusive".to_owned());
                }
                (Some(path), false) => fs::read(&path).map_err(|error| error.to_string())?,
                (None, true) => {
                    let mut buffer = Vec::new();
                    io::stdin()
                        .read_to_end(&mut buffer)
                        .map_err(|error| error.to_string())?;
                    buffer
                }
                (None, false) => return Err("one of --file or --stdin is required".to_owned()),
            };
            store.set_credential(&name, &value)?;
            print_json(&serde_json::json!({"name": name, "registered": true}))
        }
        "list" => {
            reject_extra(&args)?;
            print_json(&store.list_credentials()?)
        }
        "rm" => {
            let name = take(&mut args)?;
            reject_extra(&args)?;
            store.remove_credential(&name)?;
            print_json(&serde_json::json!({"name": name, "removed": true}))
        }
        _ => Err(usage()),
    }
}

fn run_command(store: &Store, mut args: Vec<String>) -> Result<(), String> {
    match take(&mut args)?.as_str() {
        "start" => {
            let network = option(&mut args, "--network");
            let network_none = match network.as_deref() {
                None => false,
                Some("none") => true,
                Some(other) => {
                    return Err(format!(
                        "--network only accepts 'none' (bridge is decided by the global config): {other}"
                    ));
                }
            };
            if args.iter().any(|argument| argument == "--workspace") {
                return Err(
                    "--workspace has been removed; use --repo <path> (or --url <git-url>)"
                        .to_owned(),
                );
            }
            let request = StartRequest {
                repo: checked_option(&mut args, "--repo")?.map(PathBuf::from),
                url: checked_option(&mut args, "--url")?,
                thread: checked_option(&mut args, "--thread")?,
                ask_file: checked_option(&mut args, "--ask-file")?.map(PathBuf::from),
                current_dir: env::current_dir().map_err(|error| error.to_string())?,
            };
            let overrides = gardr::RuntimeOverrides {
                image: option(&mut args, "--image"),
                harness: option(&mut args, "--harness"),
                model: option(&mut args, "--model"),
                network_none,
                ..Default::default()
            };
            if option(&mut args, "--spec").is_some() {
                return Err("--spec has been removed; configure runtime policy in <root>/config.toml and use deliberate CLI overrides only".to_owned());
            }
            let harness_args = options(&mut args, "--harness-arg");
            reject_extra(&args)?;
            print_json(&store.start_one_shot(request, overrides, harness_args)?)
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
fn checked_option(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let Some(index) = args.iter().position(|value| value == flag) else {
        return Ok(None);
    };
    if index + 1 >= args.len() || args[index + 1].starts_with("--") {
        return Err(format!("{flag} requires a value"));
    }
    args.remove(index);
    Ok(Some(args.remove(index)))
}
fn options(args: &mut Vec<String>, flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    while let Some(value) = option(args, flag) {
        values.push(value);
    }
    values
}
fn flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|value| value == name) {
        args.remove(index);
        true
    } else {
        false
    }
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

const HELP: &str = "One-shot sandbox execution for agent repositories\n\nUsage: gardr [--root <path>] <COMMAND>\n\nCommands:\n  image      Manage named image profiles\n  run        Manage workspace runs\n  credential Manage the registered credential store\n  docs       Print built-in guidance and examples\n  help       Print this message\n\nThe removed `spec` command returns a migration error. Runtime policy belongs in config.toml.\n\nRoot:\n  ~/.gardr by default; GARDR_ROOT or --root overrides it\n";

const SPEC_HELP: &str = "Per-spec runtime configuration has been removed. Move image, harness, model, credentials, network, and firewall policy to <root>/config.toml.\n";

const IMAGE_HELP: &str = "Manage named image profiles\n\nUsage: gardr [--root <path>] image <COMMAND>\n\nCommands:\n  add       Validate and store an immutable image profile: gardr image add <name> --file <path>\n  list      Print stored image profile names as JSON\n  show      Print a stored image profile; writes its SHA-256 to stderr\n  validate  Print a stored image profile's identity as JSON\n\nNote: a harnesses list containing \"claude-code\" is not supported today (no credential bootstrap);\n`image add` rejects it. Use \"pi\" with an Anthropic model instead.\n\nUse `gardr docs` for the image profile format.\n";

const RUN_HELP: &str = "Manage one-shot agent runs\n\nUsage: gardr [--root <path>] run <COMMAND>\n\nCommands:\n  start               [--repo <path> | --url <git-url>] [--thread <name>] [--ask-file <path>]\n                      [--image <name>] [--harness <adapter>] [--model <name>]\n                      [--network none] [--harness-arg <arg>]...\n  observe             <run-id>\n  resume              <interrupted-run-id>\n  stop                <run-id>\n  cleanup             <run-id>\n  validate-workspace  <path>\n\nWith no source option, start uses the current directory (or an existing thread's source). --url\ncreates a persistent checkout beneath the Gardr root. Interrupted runs resume their frozen source,\noriginal ask, and Pi transcript; successful runs are terminal. Runtime policy remains global.\n";

const CREDENTIAL_HELP: &str = "Manage the registered credential store\n\nUsage: gardr [--root <path>] credential <COMMAND>\n\nCommands:\n  set   gardr credential set <name> --file <path> | --stdin\n  list  Print registered names as JSON\n  rm    Remove a registered value: <name>\n\nValues are never printed. Global config [credentials] entries select registry names and aliases.\n";

const DOCS: &str = r#"# gardr — one-shot sandbox execution

`run start` selects the current directory by default, `--repo <path>` selects an existing checkout,
and `--url <git-url>` creates a persistent clone under `<root>/workspaces`. Every source is mounted
writable at `/repo`. `--ask-file <path>` copies an autonomous ask into the immutable run record.

A named `--thread` binds a source to `<root>/threads/<name>/CONTINUITY.md`. Later fresh runs reuse
that source and continuity file, but always get a new Pi transcript. An interrupted or unsuccessful
run may resume its frozen source, original ask, and existing transcript; successful runs are
terminal. Resume accepts no replacement source or ask.

Runtime policy remains global in `<root>/config.toml`:

```toml
image = "my-agent"
network = "bridge"
startup_command = ["toolbox", "run", "--"]

[harness]
adapter = "pi"
command = ["pi"]
model = "anthropic/claude-opus-4-6:high"

[credentials]
environment = ["GH_TOKEN", { name = "GH_TOKEN_RO", from = "github-read-only-pat" }]

[firewall]
allow = ["api.github.com", "github.com"]
```

Gardr supplies a shared `gardr-nix` volume at `/nix` and stable base instructions at
`/gardr-context/AGENTS.md`. Source selection never changes image, model, credentials, mounts,
network policy, or provider-domain inference. Image profiles remain under `<root>/images`.
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
}
