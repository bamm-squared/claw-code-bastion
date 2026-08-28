# Claw Code Bastion isolation verification

Claw Code Bastion is a security-hardened distribution derived from Claw Code.
The model, candidate code, MCP, hooks, plugins, validation targets, and
model-selected web URLs are treated as untrusted at their respective trust
boundaries. The v0.1.0-rc.1 campaign passed 84/84 required capabilities at its
tested commit on the tested Ubuntu/rootless-Podman environment; see
`artifacts/security-verification.json`. This evidence is environment-specific,
not a universal security guarantee and does not automatically cover later
changes on `main`.

Claw distinguishes generated policy from empirical runtime verification.
Normal Rust tests establish `UNIT-VERIFIED`. The stronger
`REAL-CONTAINER VERIFIED` status is valid only when the ignored Podman suite
has run successfully in a compliant rootless Linux environment.

## Runner contract

The security runner must provide Linux, rootless Podman, a writable
`XDG_RUNTIME_DIR` and rootless runtime state, user namespaces, cgroup v2, and
a supported rootless storage driver such as overlay or fuse-overlayfs.

The runner must not require privileged containers, host networking, host PID or
IPC namespaces, host filesystem mounts, host home mounts, engine sockets, SSH
agent forwarding, or broad added capabilities. It should be an expendable
Ubuntu 24.04-class host with no production or personal credentials.

## Provisioning and running the gate

Inspect a new runner without changing system configuration:

```bash
./scripts/setup-security-runner.sh
```

The script reports missing packages and rootless prerequisites. Install them
through the host's approved administration process, then run the complete gate:

```bash
./scripts/test-real-isolation.sh
```

That command runs host preflight, builds `Containerfile.worker`, performs a
real container policy probe, runs the ignored worker/validator tests, and
writes `artifacts/security-verification.json` only after a passing run.

The test target can also be run directly, although the helper is preferred
because it runs each capability test independently:

```bash
CLAW_REAL_PODMAN_IMAGE=claw-exec:security \
  cargo test -p runtime --test podman_isolation -- --ignored --nocapture
```

If preflight fails, the result is `NOT RUN`, never a passing isolation result.

The helper maps focused tests to individual capabilities in
`artifacts/security-verification.json`. Each capability is `pass`, `fail`, or
`not_tested`; the overall result is `pass` only when every required capability
has a real-container pass. A partial run is `incomplete`, not a passing gate.

The required real-container gate includes worker runtime, filesystem and
outside-write isolation, canonical and credential isolation, network and
socket isolation, symlink and candidate-Git handling, process/resource/output
limits and crash recovery; validator runtime, filesystem/canonical/credential/
network/socket isolation, candidate independence, timeout/descendant cleanup
and output bounds; and the candidate/canonical boundary, validation identity,
whole-change-set apply, and full hostile authoritative lifecycle.

## Runner registration

The dedicated workflow requires a manually registered self-hosted runner with
these labels:

```text
self-hosted
linux
rootless-podman
claw-security
```

Register it through repository or organization settings. No registration
token, personal access token, cloud credential, or deployment secret belongs in
this repository. The workflow grants only `contents: read` and disables
checkout credential persistence.

## Release gate

Strong isolation claims require ordinary Rust verification, a passing runner
preflight, passing real worker tests, passing real validator tests, and passing
canonical-integrity positive controls. If any real suite is unavailable, the
status remains `UNVERIFIED`.

## Strict private mode

`claw --private` is a trusted, locked profile layered on the isolated
execution backend. It requires rootless Podman, a disposable candidate,
fresh validation, review, and explicit whole-change-set Apply. Local execution,
worker networking, WebFetch, WebSearch, MCP, hooks, plugins, PowerShell, and
provider fallback are disabled. `--private --no-isolation` and unrestricted
permission mode are rejected, and disk-backed `--resume` is unavailable.

Private sessions keep conversation state in memory only. They do not persist
session JSONL or Anthropic prompt-cache data. Existing credentials are not
deleted, but provider credentials remain trusted-host-only and are never passed
to the worker or validator.

Execution privacy is separate from inference privacy. Obvious loopback
endpoints (`localhost`, `127.0.0.1`, and `::1`) are classified `LOCAL`.
Trusted user configuration may classify an endpoint `CONFIDENTIAL`, but Claw
does not independently attest that endpoint. Conventional HTTPS providers are
`REMOTE STANDARD`; unparseable endpoints are `UNKNOWN`. Private mode refuses
remote or unknown providers unless the trusted user supplies
`--allow-remote-provider`, which emits an explicit plaintext-provider warning.
Private mode cannot prevent a conventional remote provider from seeing prompts,
code, and tool results.

## Provider setup and first run

Provider selection is trusted user configuration. Run `claw provider setup` in
an interactive terminal to choose a local, confidential, standard remote, or
custom OpenAI-compatible provider. The command writes only the selected model
to the per-user `settings.json`; endpoints remain in the existing provider
environment variables and credentials remain in environment variables or the
trusted OAuth store. Secrets are never written to project settings.

Use `claw provider status` to inspect the resolved provider, redacted endpoint,
model, privacy class, and credential source. Local classification is limited
to obvious loopback endpoints. Arbitrary custom endpoints are `UNKNOWN` unless
trusted configuration marks them confidential via `CLAW_PROVIDER_PRIVACY=confidential`.
Remote standard providers are explicitly identified because prompts, code, and
tool results are sent to them. Existing complete environment configuration
continues without onboarding; non-interactive startup with missing credentials
fails rather than prompting or silently falling back.

Inspect the resolved profile with:

```bash
claw doctor --privacy
claw --private --allow-remote-provider doctor --privacy
```

The diagnostic redacts provider URLs to scheme and host and does not print
credential values. `--private` is not a claim of semantic correctness for
model-generated code; users must still review the candidate before Apply.

## Trusted WebFetch and WebSearch broker

WebFetch and WebSearch, when enabled outside strict private mode, execute as
trusted host-side operations. Worker, validator, MCP, and hook containers keep
`--network=none`; they do not receive a network delegation channel.

The broker accepts only HTTP(S), applies the configured `NetworkCapability`,
resolves hostnames before authorization, rejects loopback, private, link-local,
metadata, and other local destinations, revalidates every redirect, limits
redirects and request duration, and bounds response bodies to 2 MiB. Responses
are returned as data; downloaded content is never executed. Search credentials,
when required by a future configured provider, remain trusted-host-only.

Strict `--private` mode denies both WebFetch and WebSearch. Normal isolated
mode may permit them only through the trusted network policy. This does not
prevent an explicitly selected conventional model provider from seeing the
prompt or tool result.

## Isolated runtime images

The standard isolated runtime is the release-compatible
`ghcr.io/bamm-squared/claw-bastion-runtime:<Claw version>`. It contains the Claw
worker, `/bin/sh`, `git`, and the runtime libraries required by the worker. It
does not attempt to include every language toolchain, MCP server, plugin, or
project dependency.

Trusted users may select a custom runtime for an isolated task with
`CLAW_WORKER_IMAGE`. The same selected image is used for worker execution,
isolated stdio MCP, hooks, plugin executable tools, and validation unless
`CLAW_VALIDATOR_IMAGE` is explicitly set as a trusted validator override.
Claw never mounts host binaries or package directories and never falls back to
host execution when a command is missing.

Inspect the configured image without starting a task:

```bash
claw doctor --runtime
```

The diagnostic reports the configured reference and the local Podman image ID
when available. A tag such as `latest` is mutable; the resolved image ID is the
reproducibility identity for the current runtime environment. A task keeps its
selected reference across worker restarts and Request Changes; Claw does not
silently choose a different image per iteration.

To build a derived runtime explicitly:

```dockerfile
FROM ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0

# Add only the tools this trusted runtime needs.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ripgrep \
    && rm -rf /var/lib/apt/lists/*
# Install or copy trusted, reviewed MCP/plugin/hook executables here.
```

```bash
podman build -f Containerfile.claw-runtime -t localhost/my-claw-runtime:1 .
CLAW_WORKER_IMAGE=localhost/my-claw-runtime:1 claw doctor --runtime
CLAW_WORKER_IMAGE=localhost/my-claw-runtime:1 claw
```

Runtime image selection is trusted user configuration. Project configuration
and model output cannot replace it, trigger arbitrary image pulls, add mounts,
or weaken the Podman policy. Missing images or executables fail closed with a
message naming the selected image; the host-installed executable is not used.

## MCP under isolated execution

## Plugins under isolated execution

Plugin discovery, metadata, registration, permissions, and the existing
`/plugin` management commands remain trusted-host control-plane operations.
Plugin-defined tools and plugin hook commands are executable only through the
isolated worker when isolated execution is active. They receive the candidate
workspace at `/workspace/project` and the worker's existing networkless,
credential-free, read-only-root policy.

The isolated subset does not run plugin lifecycle `init` or `shutdown`
commands, because their current implementation directly spawns processes and
must not run on the trusted host. Plugin executables are not mounted from the
host; they must already be available in the selected worker image. Missing
executables fail closed with no local fallback. Networked and
credential-bearing plugins remain unsupported.

Private mode permits only this isolated executable subset. It does not make
in-process or host lifecycle plugin code private, and it does not claim that
plugin behavior has received real-container verification until the dedicated
hostile plugin suite passes.

Isolated mode supports a restricted MCP subset: configured stdio servers are
run as child processes inside a separate rootless Podman container using the
same networkless, read-only-root, private-namespace policy as the execution
worker. The candidate workspace is the only writable bind mount. The existing
MCP JSON-RPC manager remains responsible for discovery, tool calls, resources,
timeouts, and shutdown, but its process launcher is container-aware.

MCP servers that use remote HTTP, SSE, WebSocket, SDK, or managed-proxy
transports remain unsupported in isolated mode. Stdio servers with configured
environment variables are also rejected in isolated mode so credentials cannot
be passed through accidentally. The server executable and its dependencies
must exist in the selected worker image; host-installed MCP binaries are not
implicitly mounted or executed.

Private mode permits only this isolated, environment-free stdio subset. It
does not imply that an MCP server is trusted or that its returned data is safe;
the server remains untrusted code and is confined to the container. Networked
or credential-dependent MCP requires a future explicit broker design.
### Configuring the isolated runtime

The existing MCP configuration surface is unchanged. For isolated stdio MCP,
select the trusted worker image with `CLAW_WORKER_IMAGE` before starting Claw:

```json
{
  "mcpServers": {
    "local-tools": {
      "type": "stdio",
      "command": "my-mcp-server",
      "args": ["--project", "/workspace/project"]
    }
  }
}
```

```bash
CLAW_WORKER_IMAGE=localhost/claw-mcp-tools:latest claw
```

The image must already contain `my-mcp-server` and its dependencies. If it
does not, startup reports the server, command, and image and explains that
host executables are deliberately not mounted. There is no host-process
fallback. `CLAW_WORKER_IMAGE` is a trusted runtime setting; project
configuration cannot change it or the container security policy.

In `--private` mode this same environment-free stdio subset is allowed.
Remote transports and servers requiring configured environment variables are
rejected. HTTP, SSE, WebSocket, SDK, managed-proxy, networked, and
credential-dependent MCP support remain unavailable until a dedicated broker
is implemented.
