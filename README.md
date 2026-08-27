# Claw Code Bastion

A security-hardened coding-agent CLI derived from Claw Code.

Claw Code Bastion runs model-controlled code in isolated rootless containers,
keeps the canonical repository outside the hostile execution boundary, and
requires validation and review before authoritative Apply.

<p align="center">
  <a href="https://github.com/bamm-squared/claw-code-bastion">bamm-squared/claw-code-bastion</a>
  ·
  <a href="./USAGE.md">Usage</a>
  ·
  <a href="./rust/README.md">Rust workspace</a>
  ·
  <a href="./PARITY.md">Parity</a>
  ·
  <a href="./ROADMAP.md">Roadmap</a>
  ·
  <a href="https://discord.gg/jq6jnSGABY">UltraWorkers Discord</a>
</p>

<p align="center">
  <a href="https://star-history.com/#bamm-squared/claw-code-bastion&Date">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=bamm-squared/claw-code-bastion&type=Date&theme=dark" />
      <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=bamm-squared/claw-code-bastion&type=Date" />
      <img alt="Star history for bamm-squared/claw-code-bastion" src="https://api.star-history.com/svg?repos=bamm-squared/claw-code-bastion&type=Date" width="600" />
    </picture>
  </a>
</p>

<p align="center">
  <img src="assets/claw-hero.jpeg" alt="Claw Code Bastion" width="300" />
</p>

Claw Code Bastion is a security-hardened distribution derived from the Claw
Code project by Yeachan-Heo. The canonical implementation lives in
[`rust/`](./rust), and this distribution is maintained independently.

It provides isolated model-controlled execution, disposable candidate
workspaces, strict private mode, isolated MCP/hooks/plugins, a trusted
WebFetch/WebSearch broker with SSRF and DNS-rebinding defenses, local or
confidential provider support, and real rootless-Podman security verification.

The v0.1.0-rc.1 campaign passed 84/84 required capabilities on the tested
Ubuntu/rootless-Podman environment. See [`artifacts/security-verification.json`](./artifacts/security-verification.json)
for the recorded evidence; custom environments must preserve the documented
isolation prerequisites.

## Provenance

Claw Code Bastion is derived from the Claw Code project by Yeachan-Heo and is
distributed under the applicable MIT license. This distribution adds a
hardened execution, privacy, review, provider, and verification model that is
maintained independently. Upstream endorsement is not implied.

## Installing a v1 release

On supported Linux x86_64 systems, install the prebuilt CLI with checksum
verification:

```bash
curl -fsSL https://raw.githubusercontent.com/bamm-squared/claw-code-bastion/main/scripts/install-release.sh | bash
```

The installer uses `$HOME/.local/bin` and does not require `sudo`, modify shell
profiles, install Podman, or store credentials. The secure isolated runtime is
versioned with the CLI; see [`docs/release.md`](docs/release.md) for runtime
setup, source installation, and uninstall guidance.

> [!IMPORTANT]
> Start with [`USAGE.md`](./USAGE.md) for build, auth, CLI, session, and parity-harness workflows. Make `claw doctor` your first health check after building, use [`rust/README.md`](./rust/README.md) for crate-level details, read [`PARITY.md`](./PARITY.md) for the current Rust-port checkpoint, and see [`docs/container.md`](./docs/container.md) for the container-first workflow.

## Current repository shape

- **`rust/`** — canonical Rust workspace and the `claw` CLI binary
- **`USAGE.md`** — task-oriented usage guide for the current product surface
- **`PARITY.md`** — Rust-port parity status and migration notes
- **`ROADMAP.md`** — active roadmap and cleanup backlog
- **`PHILOSOPHY.md`** — project intent and system-design framing
- **`src/` + `tests/`** — companion Python/reference workspace and audit helpers; not the primary runtime surface

## Quick start

> [!NOTE]
> [!WARNING]
> **`cargo install claw-code` installs the wrong thing.** The `claw-code` crate on crates.io is a deprecated stub that places `claw-code-deprecated.exe` — not `claw`. Running it only prints `"claw-code has been renamed to agent-code"`. **Do not use `cargo install claw-code`.** Either build from source (this repo) or install the upstream binary:
> ```bash
> cargo install agent-code   # upstream binary — installs 'agent.exe' (Windows) / 'agent' (Unix), NOT 'agent-code'
> ```
> Source installation is the developer fallback. For normal users, install the
> prebuilt Linux release described above; follow the steps below when building
> from source.

```bash
# 1. Clone and build
git clone https://github.com/bamm-squared/claw-code-bastion
cd claw-code-bastion/rust
cargo build --workspace

# 2. Set your API key (Anthropic API key — not a Claude subscription)
export ANTHROPIC_API_KEY="sk-ant-..."

# 3. Verify everything is wired correctly
./target/debug/claw doctor

# 4. Run a prompt
./target/debug/claw prompt "say hello"
```

> [!NOTE]
> **Windows (PowerShell):** the binary is `claw.exe`, not `claw`. Use `.\target\debug\claw.exe` or run `cargo run -- prompt "say hello"` to skip the path lookup.

### Windows setup

**PowerShell is a supported Windows path.** Use whichever shell works for you. The common onboarding issues on Windows are:

1. **Install Rust first** — download from <https://rustup.rs/> and run the installer. Close and reopen your terminal when it finishes.
2. **Verify Rust is on PATH:**
   ```powershell
   cargo --version
   ```
   If this fails, reopen your terminal or run the PATH setup from the Rust installer output, then retry.
3. **Clone and build** (works in PowerShell, Git Bash, or WSL):
   ```powershell
   git clone https://github.com/bamm-squared/claw-code-bastion
   cd claw-code-bastion/rust
   cargo build --workspace
   ```
4. **Run** (PowerShell — note `.exe` and backslash):
   ```powershell
   $env:ANTHROPIC_API_KEY = "sk-ant-..."
   .\target\debug\claw.exe prompt "say hello"
   ```

**Git Bash / WSL** are optional alternatives, not requirements. If you prefer bash-style paths (`/c/Users/you/...` instead of `C:\Users\you\...`), Git Bash (ships with Git for Windows) works well. In Git Bash, the `MINGW64` prompt is expected and normal — not a broken install.

> [!NOTE]
> **Auth:** claw requires an **API key** (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.) — Claude subscription login is not a supported auth path.

Run the workspace test suite:

```bash
cd rust
cargo test --workspace
```

## Documentation map

- [`USAGE.md`](./USAGE.md) — quick commands, auth, sessions, config, parity harness
- [`rust/README.md`](./rust/README.md) — crate map, CLI surface, features, workspace layout
- [`PARITY.md`](./PARITY.md) — parity status for the Rust port
- [`rust/MOCK_PARITY_HARNESS.md`](./rust/MOCK_PARITY_HARNESS.md) — deterministic mock-service harness details
- [`ROADMAP.md`](./ROADMAP.md) — active roadmap and open cleanup work
- [`PHILOSOPHY.md`](./PHILOSOPHY.md) — why the project exists and how it is operated

## Ecosystem

Claw Code Bastion is built in the open alongside the broader UltraWorkers toolchain:

- [clawhip](https://github.com/Yeachan-Heo/clawhip)
- [oh-my-openagent](https://github.com/code-yeongyu/oh-my-openagent)
- [oh-my-claudecode](https://github.com/Yeachan-Heo/oh-my-claudecode)
- [oh-my-codex](https://github.com/Yeachan-Heo/oh-my-codex)
- [UltraWorkers Discord](https://discord.gg/jq6jnSGABY)

## Ownership / affiliation disclaimer

- This repository does **not** claim ownership of the original Claude Code source material.
- This repository is **not affiliated with, endorsed by, or maintained by Anthropic**.
