# Claw Code Bastion release and installation

The controlled release namespace is `bamm-squared/claw-code-bastion`; the standard
runtime package is `ghcr.io/bamm-squared/claw-bastion-runtime`. Release coordinates are
trusted build configuration, not project configuration.

## Recommended installation

The v1 supported secure distribution is a prebuilt Linux x86_64 binary plus a
versioned rootless-Podman OCI runtime image. Install without root privileges:

```bash
curl -fsSL https://raw.githubusercontent.com/bamm-squared/claw-code-bastion/main/scripts/install-release.sh | bash
```

The installer downloads the release archive and `SHA256SUMS`, verifies the
archive before installation, and installs `claw` into `$HOME/.local/bin`. It
does not modify shell profiles, install Podman, or store credentials. Pin a
release with `CLAW_VERSION=0.1.0-rc.1` or `--version 0.1.0-rc.1`.

The RC1 archive is named
`claw-code-bastion-v0.1.0-rc.1-linux-x86_64.tar.gz` and is accompanied by
`SHA256SUMS`.

The Linux x86_64 release uses the compatible runtime image:

```text
ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.1
```

Pull it explicitly as a trusted-user action when Claw reports it is missing:

```bash
podman pull ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.1
```

Headless startup does not pull or prompt. Custom trusted images remain
available through `CLAW_WORKER_IMAGE` and `CLAW_VALIDATOR_IMAGE`.

## Developer/source installation

```bash
git clone https://github.com/bamm-squared/claw-code-bastion.git
cd claw-code-bastion
cargo build --manifest-path rust/Cargo.toml --release -p rusty-claude-cli
podman build --build-arg CLAW_VERSION=0.1.0-rc.1 -f Containerfile.worker -t ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.1 .
```

## Uninstall

Remove only the installed binary and optionally Claw-owned state:

```bash
rm "$HOME/.local/bin/claw"
rm -rf "$HOME/.claw"
podman rmi ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.1
```

The last two commands are optional and must not be used for custom images or
unrelated projects. Provider credentials are user state and are not removed by
the installer.

## Release gate

The release workflow builds the Linux binary, runs ordinary Rust verification,
creates checksums, and publishes the versioned runtime image. A strong
isolated-execution claim additionally requires the separate dedicated
rootless-Podman security workflow to pass. Build/release success alone never
implies real-container security verification.

## Release candidates and acceptance

The current acceptance candidate is `v0.1.0-rc.1`. A prerelease publishes the
matching versioned binary and runtime image but does not replace the stable
`latest` runtime tag. A stable `v0.1.0` release may update `latest`.

Before publishing, prepare local artifacts and run:

```bash
./scripts/v1-acceptance.sh \
  --version 0.1.0-rc.1 \
  --artifacts-dir path/to/release-artifacts \
  --non-interactive
```

The helper verifies local archive checksums and version output, checks runtime
availability, and reports product stages requiring manual confirmation. It is
not a substitute for the dedicated hostile isolation gate. Promotion to
`v0.1.0` requires Rust CI, release artifact/checksum acceptance, clean-machine
acceptance, a fresh complete real-container security gate for the release
commit, and consistent release documentation. The RC1 artifact records 84/84
for its own tested commit; it does not automatically cover later changes on
`main`.
