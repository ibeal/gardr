# Gardr

Gardr runs a prepared, sealed Heimr workspace under one named sandbox specification. It owns
the spec store, container lifecycle, and durable execution records. It does not construct a
workspace, provide agent context, read `HANDOFF.json`, or decide whether an agent succeeded.

`gardr` stores its data in `~/.gardr` by default. Set `GARDR_ROOT` or pass `--root` to override
that location; `--root` takes precedence. Its store contains `specs/`, `runs/`, optional approved
mount directories under `mounts/`, and named image profiles and optional Docker build contexts under `images/`.
Those named directories are the only host paths a spec can request besides the supplied workspace.

```toml
# build.toml
version = 1

[image]
name = "pi-agent" # resolves only to the configured root's images/pi-agent.toml

[sandbox]
network = "none" # use "bridge" for Gardr's allowlisted egress firewall

[harness]
adapter = "pi"
# `command` contains only reusable harness arguments; the dispatch supplies
# prompt/print arguments through `run start --harness-arg`.
command = ["pi", "--no-session"]
model = "anthropic/claude-opus-4-6:high"

# The claude-code adapter has no credential bootstrap yet and is rejected at
# `spec add`/`image add` store time; use pi instead.

[[mounts]]
name = "tools"                  # resolves only to the configured root's mounts/tools
target = "/tools"
read_only = true

[credentials]
environment = ["GH_TOKEN"]     # references only; values are never stored

# Bridge runs start with Gardr's minimum egress policy plus these domains.
[firewall]
allow = ["packages.example.com"]

# Each tool is checked first. Missing tools run their declared commands while
# the listed domains are temporarily added, then Gardr reapplies runtime egress.
[[tools.install]]
name = "go"
check = "go"
install = [["asdf", "plugin", "add", "golang"], ["asdf", "install", "golang", "latest"], ["asdf", "set", "-u", "golang", "latest"]]
allow = ["go.dev", "storage.googleapis.com"]
```

```sh
gardr image add pi-agent --file pi-agent.toml
gardr image validate pi-agent
gardr spec add build --file build.toml
gardr spec validate build
gardr spec list
gardr run start --workspace /workspaces/task --spec build --harness-arg -p --harness-arg "complete the assigned work"
gardr run observe run-…
gardr run stop run-…
gardr run cleanup run-…
```

All command results except `spec show` are single JSON documents for orchestration. `run observe`
does not inspect a workspace, stream logs, attach a terminal, or interpret handoff content. Each
run writes immutable `spec.toml` and `resolved.json`, then mutable `state.json` and `runner.log`
under the configured root's `runs/<run-id>/`. Resume validates the sealed workspace and uses the frozen spec;
it never silently replaces state. Cleanup is idempotent and refuses a running run.

For `adapter = "pi"`, Gardr always persists pi's session transcript under
`runs/<run-id>/transcript/session.jsonl`, even if a spec's reusable `harness.command` still
contains `--no-session`: Gardr drops that flag and injects `--session` itself so the transcript
survives cleanup. Regardless of adapter, `run cleanup` captures the container's raw stdout/stderr to
`runs/<run-id>/stdout.log` and `runs/<run-id>/stderr.log` before `docker rm`, whatever the harness's
exit status. `run observe`'s JSON includes a `usage` field with token counts and total cost read live
from the pi transcript once one exists, so a caller never needs to locate or parse the session file
itself; there is no separate cost-reporting command. If the transcript exists but couldn't be read
or parsed cleanly, `usage_error` explains why, distinct from a plain `usage: null` (no spend yet).

Image profiles are immutable, host-owned runtime policy. A profile chooses exactly one source: an image reference or an approved build context at `<root>/images/<context>/Dockerfile`; it lists the harness adapters and preinstalled tools it provides. Specs select profiles by name and may declare `tools.required` capabilities that the profile must provide. Gardr resolves the profile and records its identity and Docker image ID in the run metadata. It does not select an image from imperative `tools.install` commands or use a remote image registry.

```toml
# pi-agent.toml
version = 1
harnesses = ["pi"]
tools = ["git", "node"]
[source]
reference = "ghcr.io/example/pi-agent@sha256:..."
```

The initial backend is Docker. Gardr runs the workspace at `/workspace`, selected approved mounts
at their declared targets, the declared Docker network policy, and only referenced credential
environment variables. Docker Desktop provides the macOS path; native Docker is supported on Linux.
No native macOS process sandbox or non-Docker Linux backend is implemented in this initial release.

| Host | Backend | Supported | Notes |
| --- | --- | --- | --- |
| macOS | Docker Desktop | Yes | Containers run in Docker's Linux VM. |
| Linux | Docker Engine | Yes | Native Docker container execution. |
| macOS or Linux | No Docker backend | No | Gardr fails explicitly; it never falls back to host execution. |
| Other hosts | Any | No | Unsupported in the initial release. |

`run start` returns `running` only after Docker returns a container identifier. `run observe` is
read-only and reconciles the container's current status and known exit code for its JSON response.
`run stop`, `run resume`, and `run cleanup` persist their lifecycle transitions; cleanup removes a
non-running container and is idempotent. An unavailable container or incomplete resolved state is
reported as an explicit runner failure rather than treated as agent-workflow success.

For `adapter = "pi"`, Gardr bootstraps its managed `<root>/pi/agent/` directory from only the
host `~/.pi/agent/auth.json`, mounts that directory at `/pi-agent`, and sets
`PI_CODING_AGENT_DIR`. Pi's `--model` is injected from the required provider-qualified
`harness.model`. The initial supported providers are `anthropic` and `openai-codex`; their
runtime API domains are added to bridge egress. Gardr never mounts the host Pi directory.

For `network = "bridge"`, Gardr follows the containerized-agent firewall model: it resolves each
allowed domain at startup, adds the resolved IPs to an ipset, pins the selected address in
`/etc/hosts`, and drops other egress. The specification is trusted and named, so there is no
separate firewall maximum. Gardr records both normal runtime egress and the temporary installer
egress in `resolved.json`. The selected image must provide the same Debian-style runtime contract as
the Gardr agent image: `iptables`, `ipset`, `dig`, `sudo`, and an entrypoint that starts its harness
only after `/usr/local/bin/init-firewall.sh` succeeds.
