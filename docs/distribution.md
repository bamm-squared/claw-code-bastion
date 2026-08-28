# Claw Code Bastion distribution guide

Claw Code Bastion is a security-hardened coding-agent CLI derived from Claw
Code and maintained independently. The command is `claw`; compatibility
interfaces such as `~/.claw` and `CLAW_*` remain unchanged.

## Support matrix

| Environment | CLI | Isolated execution | Verification |
| --- | --- | --- | --- |
| Ubuntu/Linux x86_64 | Supported | Supported with rootless Podman | RC1 real-container campaign: 84/84 |
| Other Linux architectures | Build may work | Not covered by RC1 campaign | Verify locally |
| macOS | Source/build experimentation | Not covered by RC1 campaign | No equivalent containment claim |
| Windows | Source/build experimentation or WSL2 | Not covered by RC1 campaign | No native-Windows containment claim |

The RC1 result applies only to the tested Ubuntu/rootless-Podman environment.
It is not a universal security guarantee. Custom environments must preserve
the documented isolation prerequisites.

## Installation

### Recommended release installation

On supported Linux x86_64 systems:

```bash
curl -fsSL https://raw.githubusercontent.com/bamm-squared/claw-code-bastion/main/scripts/install-release.sh | bash
```

The installer downloads the selected release archive and `SHA256SUMS`, verifies
the archive, and installs `claw` under `$HOME/.local/bin`. It does not require
`sudo` or modify shell profiles. Add that directory to `PATH` using your
shell's normal configuration if needed.

Pin a version with `CLAW_VERSION=0.1.0-rc.1` or
`--version 0.1.0-rc.1`. The RC1 archive is
`claw-code-bastion-v0.1.0-rc.1-linux-x86_64.tar.gz`.

### Developer/source installation

```bash
git clone https://github.com/bamm-squared/claw-code-bastion.git
cd claw-code-bastion
cargo build --manifest-path rust/Cargo.toml --release -p rusty-claude-cli
```

## Runtime setup

Secure isolated execution requires Linux, rootless Podman, user namespaces,
rootless storage, cgroup support, and a writable `XDG_RUNTIME_DIR`.

```bash
claw doctor --runtime
claw doctor --privacy
claw provider status
```

The standard runtime is:

```text
ghcr.io/bamm-squared/claw-bastion-runtime:<version>
```

If it is missing, pull the exact version as a trusted-user action. Claw does
not silently pull images or fall back to host execution. Trusted users may
select derived images with `CLAW_WORKER_IMAGE` and
`CLAW_VALIDATOR_IMAGE`; custom images do not remove the outer isolation policy.
Host binaries are never mounted as a workaround.

## Providers and local models

Use trusted user configuration or the supported commands:

```bash
claw provider setup
claw provider change
claw provider status
```

Supported patterns include Anthropic, OpenAI-compatible endpoints, xAI,
DashScope/Qwen, OpenRouter, and local compatible services supported by the
current build. Ollama can use the trusted `OLLAMA_HOST` setting:

```bash
export OLLAMA_HOST=http://127.0.0.1:11434
claw --model llama3.2 prompt "reply with ready"
```

Loopback endpoints are `LOCAL` where identified. Trusted configuration may
classify an endpoint `CONFIDENTIAL`; ordinary remote providers are `REMOTE
STANDARD`; unparseable/custom endpoints are `UNKNOWN`. Classification is
based on trusted endpoint configuration, not model names or project files.
Provider credentials remain on the trusted host.

## Strict private mode

```bash
claw --private
```

Private mode requires isolated execution and disables session persistence,
resume, WebFetch, WebSearch, provider fallback, and non-isolated extension
execution. Remote or unknown providers require the documented trusted-user
authorization. Private mode is not a guarantee of inference confidentiality
or anonymity; the selected provider can still receive prompts and code.

## MCP, hooks, and plugins

MCP stdio servers, hooks, and plugin tools run only through isolated
execution. Their executables and dependencies must exist in the selected
runtime image. They receive only the disposable candidate workspace and do not
receive host credentials, host sockets, or network access.

Remote MCP transports and credential-dependent MCP are outside the supported
isolated subset. Fix missing executables or tool discovery failures in trusted
runtime/configuration; do not mount host binaries or enable host fallback.
Extension changes affect the candidate and become authoritative only after
validation, review, and explicit Apply.

## Sessions, JSON, and headless use

Sessions are workspace-scoped. Resuming a conversation does not restore an
authoritative unapplied candidate, validation result, or review from an earlier
process. Private mode does not persist sessions and rejects disk-backed resume.

For automation, use JSON output where supported:

```bash
claw --output-format json --help
claw --output-format json provider status
```

JSON diagnostics provide stable error kinds, short errors, and optional hints.
Non-TTY startup fails with actionable errors instead of waiting for provider
setup input.

## Troubleshooting

- Podman missing or not rootless: configure rootless Podman through the host's approved administration process, then run `claw doctor --runtime`.
- Runtime image missing: pull the exact versioned Bastion image.
- Tool, MCP, hook, or plugin executable missing: add the reviewed dependency to a trusted derived image and select it with `CLAW_WORKER_IMAGE`.
- Provider setup fails: configure trusted credentials or endpoint variables, then run `claw provider status`.
- Private mode blocks web or resume: this is intentional fail-closed behavior.
- Resume is rejected: use the original workspace and a trusted session reference; do not bypass a path or workspace mismatch.

Do not solve security errors by mounting `$HOME`, `.ssh`, engine sockets, or
host binaries, enabling broad networking, or disabling isolation.

## Upgrade and uninstall

Rerun the installer with the desired version after checking its published
checksum. It replaces only the user-local `claw` binary.

```bash
rm "$HOME/.local/bin/claw"
rm -rf "$HOME/.claw"
podman rmi ghcr.io/bamm-squared/claw-bastion-runtime:<version>
```

The latter commands are optional and remove only Claw-owned state/image. They
do not remove user projects, unrelated images, or provider credentials.

## Release and security policy

Generic hosted CI performs ordinary Rust verification and packaging. The real
84-capability security gate is separate and requires a compliant dedicated
Ubuntu/rootless-Podman environment. Hosted packaging success is not
real-container security evidence.

RC and stable releases publish versioned archives, `SHA256SUMS`, and versioned
runtime images. Prereleases do not update stable `latest`; stable releases may
update it under the release policy.

RC1 passed 84/84 required capabilities at its tested commit and runtime. The
current `main` includes post-RC integration changes, so a fresh 84/84 campaign
is required at the next release gate.

## Provenance

Claw Code Bastion is derived from the Claw Code project by Yeachan-Heo and is
distributed under the applicable MIT license. It is maintained independently
and does not imply upstream endorsement.
