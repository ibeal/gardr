# Gardr

Gardr runs fresh Pi sessions against a selected repository in Docker. Runtime policy, the shared
`/nix` cache, stable agent context, credentials, and fail-closed egress are Gardr-owned. Every run
has an immutable record; optional named threads preserve only bounded continuity between runs.

## Global runtime policy

Runtime policy belongs in `~/.gardr/config.toml` (or `$GARDR_ROOT/config.toml`):

```toml
image = "my-agent"
network = "bridge"
startup_command = []

[[mounts]]
type = "volume"
source = "shared-tool-cache"
target = "/tool-cache"

[harness]
adapter = "pi"
command = ["pi"]
model = "anthropic/claude-opus-4-6:high"

[credentials]
environment = ["GH_TOKEN", { name = "GH_TOKEN_RO", from = "github-read-only-pat" }]

[firewall]
allow = ["api.github.com", "github.com"]
```

The selected repository is mounted writable at `/repo`. Gardr also mounts its global `gardr-nix`
volume at `/nix`, stable base instructions at `/gardr-context/AGENTS.md`, and per-run Pi state.
Source selection never changes image, model, credentials, mounts, or network policy.

Credential values live in Gardr's private registry:

```sh
gardr credential set github-read-only-pat --file ./pat.txt
gardr credential list
gardr credential rm github-read-only-pat
```

## Sources, asks, and threads

```sh
# Current directory
gardr run start

# Existing checkout
gardr run start --repo ../project

# Persistent clone under the Gardr root
gardr run start --url https://github.com/example/project.git

# Autonomous one-shot input (copied into the immutable run record)
gardr run start --repo ../project --ask-file ./task.md

# Bounded continuity: first call binds the source; later calls reuse it
gardr run start --repo ../project --thread refactor
gardr run start --thread refactor --ask-file ./follow-up.md
```

A URL run reports `source.workspace_id` and `source.workspace_path`; its `source.path` can also be
passed to a later `--repo` run. A thread stores only `CONTINUITY.md`, which the worker keeps limited
to decisions, current state, verification, blockers, and next work. Prior Pi transcripts are never
injected into a new run.

Fresh runs never inherit another run's transcript. An interrupted or unsuccessful run can be
resumed with `gardr run resume <run-id>`; it reuses that run's frozen source, policy, original ask,
and Pi transcript and accepts no replacement input. Successfully completed runs are terminal.
`run observe`, `run stop`, and `run cleanup` manage the current container and retained per-attempt
stdout/stderr, transcript, input, and metadata.

## Images

Images are user-owned profiles under the Gardr root:

```toml
version = 1
harnesses = ["pi"]
tools = ["git", "node", "pi"]

[source]
build_context = "my-agent"
# Or: reference = "registry.example.com/my-agent@sha256:..."
```

## Commands

```sh
gardr image add <name> --file <profile.toml>
gardr image list
gardr image show <name>
gardr image validate <name>

gardr run start [--repo <path> | --url <git-url>] [--thread <name>] [--ask-file <path>] \
  [--image <name>] [--harness pi] [--model <provider/model>] [--network none]
gardr run observe <run-id>
gardr run stop <run-id>
gardr run cleanup <run-id>
```
