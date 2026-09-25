use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
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
                "spec" => spec(&store, args),
                "image" => image(&store, args),
                "run" => run_command(&store, args),
                "credential" => credential(&store, args),
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
            let (spec, identity, _) = store.read_spec(&name)?;
            store.validate_runtime_spec(&spec)?;
            print_json(&identity)
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
        "rebuild" => {
            let all = flag(&mut args, "--all");
            if all {
                reject_extra(&args)?;
                print_json(&store.rebuild_images(&store.list_images()?)?)
            } else {
                let name = take(&mut args)?;
                reject_extra(&args)?;
                print_json(&store.rebuild_image(&name)?)
            }
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
            let overrides = gardr::RuntimeOverrides {
                workspace: option(&mut args, "--workspace"),
                image: option(&mut args, "--image"),
                harness: option(&mut args, "--harness"),
                model: option(&mut args, "--model"),
                network_none,
            };
            let spec = option(&mut args, "--spec");
            let harness_args = options(&mut args, "--harness-arg");
            reject_extra(&args)?;
            print_json(&store.start(overrides, spec.as_deref(), harness_args)?)
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

const HELP: &str = "Durable sandbox execution for prepared agent workspaces\n\nUsage: gardr [--root <path>] <COMMAND>\n\nCommands:\n  image      Manage named image profiles\n  spec       Manage sandbox specifications\n  run        Manage workspace runs\n  credential Manage the registered credential store\n  docs       Print built-in guidance and examples\n  help       Print this message\n\nImage and spec commands:\n  add, list, show, validate\n\nRun commands:\n  start, observe, resume, stop, cleanup, validate-workspace\n\nCredential commands:\n  set, list, rm\n\nRun `gardr spec --help`, `gardr run --help`, or `gardr credential --help` for command details.\n\nRoot:\n  ~/.gardr by default; GARDR_ROOT or --root overrides it\n\nAn optional <root>/config.toml supplies global defaults for workspace/image/harness/model/spec;\nsee `gardr run --help` and `gardr docs` for the layering precedence.\n";

const SPEC_HELP: &str = "Manage sandbox specifications\n\nUsage: gardr [--root <path>] spec <COMMAND>\n\nCommands:\n  add       Validate and store a spec: gardr spec add <name> --file <path>\n  list      Print stored spec names as JSON\n  show      Print a stored spec; writes its SHA-256 to stderr\n  validate  Print a stored spec's identity as JSON\n\nNote: harness.adapter = \"claude-code\" is not supported today (no credential bootstrap); `spec add`\nrejects it. Use \"pi\" with an Anthropic model instead.\n\nFirewall policy is global-only: a spec cannot declare [firewall], and `spec add` rejects one that\ndoes. A spec's [sandbox] network may only narrow the global config's network default to \"none\".\n\nA spec's [credentials] environment entries resolve from the `gardr credential` store, not from\ngardr's own process environment; see `gardr credential --help` and `gardr docs`.\n\nUse `gardr docs` for the specification format.\n";

const IMAGE_HELP: &str = "Manage named image profiles\n\nUsage: gardr [--root <path>] image <COMMAND>\n\nCommands:\n  add       Validate and store an immutable image profile: gardr image add <name> --file <path>\n  list      Print stored image profile names as JSON\n  show      Print a stored image profile; writes its SHA-256 to stderr\n  validate  Print a stored image profile's identity as JSON\n  rebuild   Refresh the image behind a profile: gardr image rebuild <name> | --all\n\nNote: a harnesses list containing \"claude-code\" is not supported today (no credential bootstrap);\n`image add` rejects it. Use \"pi\" with an Anthropic model instead.\n\n`rebuild` rebuilds a build-context profile from scratch (no layer cache, base image re-pulled) or\nre-pulls a reference profile; a run started afterward resolves the freshly refreshed image without\nfurther action. `--all` rebuilds every stored profile in one invocation. A rebuild failure reports\nthe docker error and leaves the previously working image and tag in place.\n\nUse `gardr docs` for the image profile format.\n";

const RUN_HELP: &str = "Manage prepared workspace runs\n\nUsage: gardr [--root <path>] run <COMMAND>\n\nCommands:\n  start               Start a run: [--workspace <path>] [--spec <name>] [--image <name>]\n                      [--harness <adapter>] [--model <name>] [--network none] [--harness-arg <arg>]...\n  observe             Reconcile and print a run: <run-id>\n  resume              Restart a stopped or failed run: <run-id>\n  stop                Stop a running run: <run-id>\n  cleanup             Remove a non-running container: <run-id>\n  validate-workspace  Validate a prepared, sealed workspace: <path>\n\n`run start` resolves workspace, image, harness, and model from three layers, per key:\n  --workspace/--image/--harness/--model (CLI) > the stored spec named by --spec (or the global\n  config's default spec) > <root>/config.toml (global defaults). `--spec` is optional: a run may\n  start with no spec at all when the global config and CLI resolve every required key. A key left\n  unset by every layer fails `run start` explicitly, naming the key and the layers consulted.\n  `harness.command` layers the same way between the spec and global config (no CLI override),\n  falling back to a built-in per-adapter default (`[\"pi\"]` for pi).\n\nNetwork mode (`none`/`bridge`) and the egress firewall allowlist are global-only, at\n<root>/config.toml. `--network none` (or a spec's `[sandbox] network = \"none\"`) may only narrow\nthe global default to `none`; neither can force `bridge`. A bridge run whose global config has no\nnon-empty `[firewall] allow` fails explicitly at `run start`; there is no hardcoded minimum or\nseparate installer allowlist. See `gardr docs`.\n\nRun commands return one JSON document, including the effective value and source layer\n(`cli`/`spec`/`global`/`default`) gardr used for each of the four layered keys, plus the resolved\nnetwork mode and frozen firewall allowlist. Use `gardr docs` for lifecycle details and the\nlayering precedence.\n";

const CREDENTIAL_HELP: &str = "Manage the registered credential store\n\nUsage: gardr [--root <path>] credential <COMMAND>\n\nCommands:\n  set   Register (upsert) a credential: gardr credential set <name> --file <path> | --stdin\n  list  Print registered credential names as JSON (values are never included)\n  rm    Remove a registered credential: <name>\n\nCredential values are never printed back by any command. Use `gardr docs` for how a spec's\n[credentials] environment entries resolve a value through this store.\n";

const DOCS: &str = r#"# gardr — durable sandbox execution

Gardr runs a prepared, sealed workspace under one named sandbox specification. It owns the
specification store, Docker container lifecycle, and durable run records. It does not create a
workspace, prepare an agent dispatch, or determine whether the agent completed its assignment.

## Root and store

Gardr uses `~/.gardr` by default. Set `GARDR_ROOT` or pass `--root <path>` to select another root;
`--root` wins. The root contains `specs/`, `runs/`, optional approved `mounts/`, and named image
profiles plus optional build contexts under `images/`. Gardr creates managed directories when first needed.

## Commands

```text
gardr image add <name> --file <path>     # validate and store an immutable image profile
gardr image list                         # JSON list of stored image profiles
gardr image show <name>                  # print the TOML and its SHA-256 to stderr
gardr image validate <name>              # JSON identity for one image profile
gardr image rebuild <name>               # refresh the image behind one profile
gardr image rebuild --all                # refresh the image behind every stored profile

gardr spec add <name> --file <path>      # validate and store an immutable spec
gardr spec list                          # JSON list of stored names
gardr spec show <name>                   # print the TOML and its SHA-256 to stderr
gardr spec validate <name>               # JSON identity for one stored spec

gardr run validate-workspace <path>      # validate a prepared, sealed workspace
gardr run start [--workspace <path>] [--spec <name>] [--image <name>] [--harness <adapter>]
                [--model <name>] [--network none] [--harness-arg <arg>]...
gardr run observe <run-id>               # reconcile and return current run state; reports the
                                          # effective value and source layer for each layered key
gardr run resume <run-id>                # restart a stopped or failed run
gardr run stop <run-id>                  # stop a running container
gardr run cleanup <run-id>               # remove a non-running container; idempotent

gardr credential set <name> --file <path>   # register (upsert) a credential from a file
gardr credential set <name> --stdin         # or from stdin; the value is never printed back
gardr credential list                       # JSON list of registered credential names (no values)
gardr credential rm <name>                  # remove a registered credential
```

All command results except `spec show` are one JSON document, intended for an orchestrator to read.

## Layered runtime configuration

`workspace`, `image`, `harness` (the adapter name), and `model` resolve from three layers, per key:
the `run start` command line (`--workspace`/`--image`/`--harness`/`--model`) beats a stored spec,
which beats the optional global config at `<root>/config.toml`. A missing `config.toml` is not an
error. `harness.command` layers the same way between a spec and the global config (there is no CLI
override for it), falling back to a built-in per-adapter default (`["pi"]` for the
only supported adapter, `pi`) when neither layer sets it. The global config may also name a default
`spec`; `--spec` is optional on `run start` and overrides it — a run may start with no spec at all
when the global config and CLI resolve every required key between them:

```toml
# <root>/config.toml (all keys optional)
workspace = "/workspaces/default"
spec = "build"
image = "claude-agent"
network = "bridge" # or "none"; the sandbox network default for every run

[harness]
adapter = "pi"
command = ["pi"]
model = "anthropic/claude-opus-4-6:high"

[firewall]
# The single egress allowlist applied to every bridge run. There is no per-spec [firewall], no
# hardcoded minimum, and no separate installer-only domain set; a bridge run whose resolved
# allowlist is empty fails explicitly at `run start`.
allow = ["api.github.com", "github.com", "crates.io", "packages.example.com"]
```

## Network and firewall are global-only

`sandbox.network` (`none`/`bridge`) and the egress allowlist are configured only in
`<root>/config.toml`; a spec cannot declare `[firewall]` and `spec add` rejects one that does, and
Gardr no longer ships a hardcoded minimum allowlist or a separate installer-domain set — every
tool install runs against the same single `[firewall] allow` list as the harness. A spec's own
`[sandbox] network`, and `run start --network`, may only *narrow* the global default to `"none"`;
neither can force `bridge`, which is the global config's decision alone. Left unset everywhere, a
run resolves to `none`. A bridge run whose global config has no (or an empty) `[firewall] allow`
fails explicitly at `run start` rather than starting with open or empty egress. The resolved
network mode and firewall allowlist are frozen into the run record at `run start`, exactly like the
four layered runtime keys; `run resume` reuses them verbatim and never re-reads the global config.
A stored spec that still declares `[firewall]` or `[[tools.install]].allow` fails to load with a
clear error naming the removed key, not a silent ignore.

Pi and OpenAI-Codex provider API domains implied by the effective `model` (`api.anthropic.com`, or
`api.openai.com`/`chatgpt.com`) are added to bridge egress automatically, in addition to the global
allowlist — an operator never has to list them by hand.

A spec no longer has to set `image` or `harness.model` (or, for that matter, `workspace` or
`harness.adapter`/`harness.command`): whatever a layer leaves unset, the next layer down supplies.
A required key left unset by every layer fails `run start` explicitly, naming the key and the
layers consulted (`cli`, `spec`, `global`). Validations that used to run only at `spec add` —
supported adapter, the image profile provides the harness, a provider-qualified model,
`tools.required` satisfied by the image — now run at `run start`/`run resume` against the *merged*
result instead, since any of those values may come from a different layer than the others.
`spec add` still rejects a spec whose own values (whichever it sets) are invalid. The merged
effective value and its source layer, for every layered key, are frozen into the run record at
`run start` and reported by both `run start` and `run observe`; `run resume` uses those frozen
values verbatim and never re-merges the layers, so a later edit to the global config or a
re-registered spec can never change a run already in flight.

## A sandbox specification

```toml
version = 1

# workspace, [image] name, and [harness] adapter/command/model are all optional here: whichever
# of them the global config or `run start` flags supply, this spec doesn't need to repeat.

[sandbox]
# Optional: may only narrow the global config's network default to "none". Omit this to use
# whatever the global config (or `run start --network none`) resolves.
network = "none"

[harness]
adapter = "pi"          # the only supported adapter today; see note below
command = ["pi"] # reusable harness arguments; dispatch arguments
                  # such as `-p` belong to `run start --harness-arg`
model = "anthropic/claude-opus-4-6:high"

[[mounts]]
name = "tools"                  # resolves only to <root>/mounts/tools
target = "/tools"
read_only = true

[credentials]
# Plain strings stay valid: the env-var name doubles as the credential-store key.
# A table form maps the in-container `name` to a different registry key `from`
# (defaulting to `name`), so one registered secret can be reused under several names.
environment = ["GH_TOKEN", { name = "GH_TOKEN_RO", from = "github-read-only-pat" }]

# Firewall policy lives only in the global config now; a spec cannot declare [firewall].
[[tools.install]]
name = "go"
check = "go"
install = [["asdf", "plugin", "add", "golang"], ["asdf", "install", "golang", "latest"], ["asdf", "set", "-u", "golang", "latest"]]
```

`gardr image rebuild` refreshes the Docker image behind a named profile without touching the
immutable profile itself: a build-context profile is rebuilt with no layer cache and its base image
re-pulled, and a reference profile is re-pulled. `--all` refreshes every stored profile in one
invocation. The next `run start` using that profile resolves the freshly refreshed image with no
further action. Docker tags Gardr previously created for a rebuilt build-context profile that no
longer correspond to its current context directory are removed; images Gardr did not create are
never touched. A rebuild failure reports the docker error and leaves the previously working image
and tag in place. Rebuilding is always explicit — there is no automatic staleness detection or
scheduled refresh.

An image profile is host-owned policy and declares a single source, its supported harnesses, and the
preinstalled tools it provides:

```toml
version = 1
harnesses = ["pi"]
tools = ["git", "node"]
[source]
build_context = "claude-agent" # <root>/images/claude-agent/Dockerfile
```

Specs select profiles by name. `tools.required` must be provided by that profile; `tools.install`
remains for ephemeral missing-tool installation and never selects an image. Profile, spec, mount, and
build-context names select approved children of the Gardr root. A mount cannot target `/workspace`,
and Gardr rejects unknown fields, duplicate mount names or targets, invalid container paths, and
duplicate credential environment names.

Gardr owns a private credential registry under `<root>/credentials/`, independent of
`mounts`/`specs`/`images`: `gardr credential set <name> --file <path>` (or `--stdin`) registers or
rotates a value without ever printing it back, `gardr credential list` prints registered names as
JSON, and `gardr credential rm <name>` removes one. A spec's `[credentials] environment` entries stay
valid as plain strings, which resolve from the store entry of the same name; an entry may instead be
a table with `name` (the in-container env var) and `from` (the store key, defaulting to `name`), so
one registered secret can be injected under different names across specs or entries. `run start`
resolves each entry's value from this store — never from `gardr`'s own process environment — and
fails with a clear error naming the missing store key if a spec references an unregistered
credential. Two entries that resolve to the same in-container `name` are rejected at `spec add` time.

`claude-code` is a recognized `harness.adapter` value but is not a supported harness today: it has
no credential-bootstrap mechanism, so `gardr spec add` and `gardr image add` both reject it at store
time. Use `pi` with an Anthropic model instead.

Pi requires a provider-qualified `harness.model`; Gardr injects it as `--model`. It bootstraps the
managed `<root>/pi/agent/` directory from only host `~/.pi/agent/auth.json`, mounts that at
`/pi-agent`, and sets `PI_CODING_AGENT_DIR`; it never mounts the host Pi directory. The initial
supported providers are `anthropic` and `openai-codex`, whose API domains are added to bridge
egress automatically alongside the global `[firewall] allow` list (see "Network and firewall are
global-only" above).

Bridge runs apply exactly the global config's `[firewall] allow` list plus those implied provider
domains: there is no hardcoded minimum, no separate maximum, and no separate installer-only domain
set. Gardr resolves and pins each allowed address, then reapplies the same policy before starting
the harness (a missing tool's install commands run before the firewall locks down for the harness
itself). The image must provide the Gardr agent runtime contract: `iptables`, `ipset`, `dig`,
`sudo`, and an entrypoint that fails closed when `/usr/local/bin/init-firewall.sh` fails.

## Workspace and lifecycle

`run start` accepts only a prepared workspace containing one or more sealed entries under
`dispatches/`. Gardr mounts that workspace at `/workspace`, freezes the merged effective runtime
configuration (and the selected spec, if any), records the workspace seal and approved mounts, then
starts Docker. It never falls back to host execution.

Each run has a directory at `<root>/runs/<run-id>/` containing the frozen spec (`spec.toml`, absent
when the run started with no `--spec`), resolved configuration, run state, mount lock, runner log,
and artifacts directory. `resume` revalidates the workspace seal, frozen spec (if any), and approved
mounts before launching again, using the frozen effective values verbatim; it never re-merges the
global config, spec, or CLI layers. `cleanup` is terminal and refuses a running run; stop it first.

For the pi adapter, Gardr always writes pi's session transcript to
`<root>/runs/<run-id>/transcript/session.jsonl`, overriding any `--no-session` left in a spec's
reusable `harness.command` so existing specs keep working unmodified. Regardless of adapter,
`cleanup` captures the container's raw stdout/stderr to `<root>/runs/<run-id>/stdout.log` and
`stderr.log` before `docker rm`, whatever the harness's exit status. `run observe`'s JSON `usage`
field reports token counts and total cost read live from the pi transcript, once one exists; there
is no separate cost command. A `usage_error` field distinguishes a transcript that couldn't be read
or parsed cleanly from `usage: null` (no spend yet).
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
        assert!(SPEC_HELP.contains("add"));
        assert!(RUN_HELP.contains("validate-workspace"));
        assert!(CREDENTIAL_HELP.contains("set"));
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
