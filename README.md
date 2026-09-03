# Gardr

Gardr runs a prepared, sealed Heimr workspace under one named sandbox specification. It owns
the spec store, container lifecycle, and durable execution records. It does not construct a
workspace, provide agent context, read `HANDOFF.json`, or decide whether an agent succeeded.

`gardr` requires `GARDR_ROOT` (or `--root`). Its store contains `specs/`, `runs/`, optional
approved mount directories under `mounts/`, and optional Docker build contexts under `images/`.
Those named directories are the only host paths a spec can request besides the supplied workspace.

```toml
# build.toml
version = 1

[image]
reference = "ghcr.io/example/agent:latest"
# build_context = "agent-image" # resolves only to $GARDR_ROOT/images/agent-image

[sandbox]
network = "none" # "bridge" is the only other initial policy

[harness]
adapter = "claude-code"
command = ["claude", "-p", "complete the assigned work"]

[[mounts]]
name = "tools"                  # resolves only to $GARDR_ROOT/mounts/tools
target = "/tools"
read_only = true

[credentials]
environment = ["GH_TOKEN"]     # references only; values are never stored
```

```sh
gardr --root /var/lib/gardr spec add build --file build.toml
gardr --root /var/lib/gardr spec validate build
gardr --root /var/lib/gardr spec list
gardr --root /var/lib/gardr run start --workspace /workspaces/task --spec build
gardr --root /var/lib/gardr run observe run-…
gardr --root /var/lib/gardr run stop run-…
gardr --root /var/lib/gardr run cleanup run-…
```

All command results except `spec show` are single JSON documents for orchestration. `run observe`
does not inspect a workspace, stream logs, attach a terminal, or interpret handoff content. Each
run writes immutable `spec.toml` and `resolved.json`, then mutable `state.json` and `runner.log`
under `$GARDR_ROOT/runs/<run-id>/`. Resume validates the sealed workspace and uses the frozen spec;
it never silently replaces state. Cleanup is idempotent and refuses a running run.

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
