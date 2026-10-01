# Gardr

Gardr runs prepared workspaces in Docker with durable run records, Pi transcripts, registered
credentials, and fail-closed egress. It does not prepare workspaces, choose a toolchain, or mandate
how an image supplies its tools.

## Global runtime policy

Runtime policy belongs in `~/.gardr/config.toml` (or `$GARDR_ROOT/config.toml`):

```toml
workspace = "/workspaces/default"
image = "my-agent"
network = "bridge"

# Optional command prefix placed before the harness command.
# For example: ["nix", "develop", "--command"] or ["toolbox", "run", "--"]
startup_command = []

[[mounts]]
type = "volume"
source = "shared-tool-cache"
target = "/tool-cache"

[[mounts]]
type = "bind"
source = "/absolute/host/path"
target = "/extra-input"
read_only = true

[harness]
adapter = "pi"
command = ["pi"]
model = "anthropic/claude-opus-4-6:high"

[credentials]
environment = ["GH_TOKEN", { name = "GH_TOKEN_RO", from = "github-read-only-pat" }]

[firewall]
allow = ["api.github.com", "github.com"]
```

`startup_command` and `mounts` are generic container configuration. Gardr passes named Docker
volumes and absolute-path bind mounts through as configured; it does not inject `/nix`, create a
Nix-specific cache policy, or require Nix. Docker creates a missing named volume on first use.
Mount targets used internally by Gardr are reserved.

`run start` may deliberately override workspace, image, harness, or model. `--network none` may
narrow global bridge access. Per-spec runtime policy and `--spec` are removed and return migration
errors instead of being silently ignored.

Credential values live in Gardr's private registry, not the config or process environment:

```sh
gardr credential set github-read-only-pat --file ./pat.txt
gardr credential list
gardr credential rm github-read-only-pat
```

## Images

Images are user-owned profiles under the Gardr root, normally `~/.gardr/images`:

```toml
version = 1
harnesses = ["pi"]
tools = ["git", "node", "pi"]

[source]
build_context = "my-agent"
# Or use: reference = "registry.example.com/my-agent@sha256:..."
```

A build context is resolved relative to `~/.gardr/images`. Gardr validates/builds or pulls the
selected image, but makes no assumptions about its package manager or toolchain. Images may contain
all required tools directly, use Nix, or use any other startup mechanism compatible with the global
`startup_command`.

## Commands

```sh
gardr image add <name> --file <profile.toml>
gardr image list
gardr image show <name>
gardr image validate <name>

gardr run validate-workspace <path>
gardr run start [--workspace <path>] [--image <name>] [--harness pi] [--model <provider/model>] \
  [--network none] [--harness-arg <arg>]...
gardr run observe <run-id>
gardr run resume <run-id>
gardr run stop <run-id>
gardr run cleanup <run-id>
```

Runs freeze their effective policy and image identity. Resume uses that frozen state rather than
re-reading global configuration. Pi transcripts and usage survive container cleanup; cleanup also
captures stdout/stderr before removing the container.
