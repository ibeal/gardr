pub(super) fn usage() -> String {
    HELP.trim_end().to_owned()
}

pub(super) const HELP: &str = "One-shot sandbox execution for agent repositories\n\nUsage: gardr [--root <path>] <COMMAND>\n\nCommands:\n  image      Manage named image profiles\n  run        Manage workspace runs\n  credential Manage the registered credential store\n  docs       Print built-in guidance and examples\n  help       Print this message\n\nThe removed `spec` command returns a migration error. Runtime policy belongs in config.toml.\n\nRoot:\n  ~/.gardr by default; GARDR_ROOT or --root overrides it\n";

pub(super) const SPEC_HELP: &str = "Per-spec runtime configuration has been removed. Move image, harness, model, credentials, network, and firewall policy to <root>/config.toml.\n";

pub(super) const IMAGE_HELP: &str = "Manage named image profiles\n\nUsage: gardr [--root <path>] image <COMMAND>\n\nCommands:\n  add       Validate and store an immutable image profile: gardr image add <name> --file <path>\n  list      Print stored image profile names as JSON\n  show      Print a stored image profile; writes its SHA-256 to stderr\n  validate  Print a stored image profile's identity as JSON\n\nNote: a harnesses list containing \"claude-code\" is not supported today (no credential bootstrap);\n`image add` rejects it. Use \"pi\" with an Anthropic model instead.\n\nUse `gardr docs` for the image profile format.\n";

pub(super) const RUN_HELP: &str = "Manage one-shot agent runs\n\nUsage: gardr [--root <path>] run <COMMAND>\n\nCommands:\n  start               [--repo <path> | --url <git-url>] [--thread <name>] [--ask-file <path>]\n                      [--image <name>] [--harness <adapter>] [--model <name>]\n                      [--network none] [--harness-arg <arg>]...\n  observe             <run-id>\n  resume              <interrupted-run-id>\n  stop                <run-id>\n  cleanup             <run-id>\n  validate-workspace  <path>\n\nWith no source option, start uses the current directory (or an existing thread's source). --url\ncreates a persistent checkout beneath the Gardr root. With no --ask-file or --harness-arg, start\nattaches the terminal to the harness; detach with Ctrl-P, Ctrl-Q. Autonomous runs remain detached.\nInterrupted runs resume their frozen source, original ask, and Pi transcript; successful runs are\nterminal. Runtime policy remains global.\n";

pub(super) const CREDENTIAL_HELP: &str = "Manage the registered credential store\n\nUsage: gardr [--root <path>] credential <COMMAND>\n\nCommands:\n  set   gardr credential set <name> --file <path> | --stdin\n  list  Print registered names as JSON\n  rm    Remove a registered value: <name>\n\nValues are never printed. Global config [credentials] entries select registry names and aliases.\n";

pub(super) const DOCS: &str = r#"# gardr — one-shot sandbox execution

`run start` selects the current directory by default, `--repo <path>` selects an existing checkout,
and `--url <git-url>` creates a persistent clone under `<root>/workspaces`. Every source is mounted
writable at `/repo`. With no `--ask-file` or `--harness-arg`, `run start` attaches the terminal to
an interactive harness session and forwards `TERM` and `COLORTERM`; use Docker's default Ctrl-P,
Ctrl-Q sequence to detach without stopping it. Supplying either kind of autonomous input leaves the
run detached and prints its JSON
record. `--ask-file <path>` copies an autonomous ask into the immutable run record.

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

Gardr mounts stable base instructions at `/gardr-context/AGENTS.md`. Additional volumes and bind
mounts are user-owned runtime policy configured through `[[mounts]]`. Source selection never changes
image, model, credentials, mounts, network policy, or provider-domain inference. Image profiles
remain under `<root>/images`.
"#;
