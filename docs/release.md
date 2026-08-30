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
release with `CLAW_VERSION=0.1.0-rc.2` or `--version 0.1.0-rc.2`.

The RC2 archive is named
`claw-code-bastion-v0.1.0-rc.2-linux-x86_64.tar.gz` and is accompanied by
`SHA256SUMS`.

The Linux x86_64 release uses the compatible runtime image:

```text
ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.2
```

Pull it explicitly as a trusted-user action when Claw reports it is missing:

```bash
podman pull ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.2
```

Headless startup does not pull or prompt. Custom trusted images remain
available through `CLAW_WORKER_IMAGE` and `CLAW_VALIDATOR_IMAGE`.

## Developer/source installation

```bash
git clone https://github.com/bamm-squared/claw-code-bastion.git
cd claw-code-bastion
cargo build --manifest-path rust/Cargo.toml --release -p rusty-claude-cli
podman build --build-arg CLAW_VERSION=0.1.0-rc.2 -f Containerfile.worker -t ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.2 .
```

## Uninstall

Remove only the installed binary and optionally Claw-owned state:

```bash
rm "$HOME/.local/bin/claw"
rm -rf "$HOME/.claw"
podman rmi ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.2
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

The current acceptance candidate is `v0.1.0-rc.2`. A prerelease publishes the
matching versioned binary and runtime image but does not replace the stable
`latest` runtime tag. A stable `v0.1.0` release may update `latest`.

Before publishing, prepare local artifacts and run:

```bash
./scripts/v1-acceptance.sh \
  --version 0.1.0-rc.2 \
  --artifacts-dir path/to/release-artifacts \
  --non-interactive
```

The helper verifies local archive checksums and version output, resolves the
selected runtime, starts the checked-in localhost mock provider, and drives the
packaged binary through a PTY. The deterministic acceptance creates a disposable
candidate with a real isolated tool-call, validation/review, and Apply lifecycle;
it uses only test-owned HOME/configuration and a synthetic provider key. It never
uses external provider traffic. It is not a substitute for the dedicated hostile
isolation gate. Promotion to `v0.1.0` requires Rust CI, release
artifact/checksum acceptance, clean-machine acceptance, a fresh complete
real-container security gate for the release commit, and consistent release
documentation. The historical RC1 artifact records 84/84 for its own tested
commit; it does not automatically cover later changes on `main`.

The current release verifier includes six additional composite assertions for
post-RC boundaries: trusted Git, retrieval, trusted `@` context, terminal
rendering, trusted external attachments, and typed multimodal image input.
The resulting current release inventory is 90 assertions. The gate builds one
revision-labeled runtime image, passes its exact reference to the real
security campaign, and fails before testing if the resolved image ID or source
revision differs from the recorded gate identity.

## RC2 release notes

RC2 includes trusted Git intelligence, local ContextSearch retrieval, trusted
`@` context references, the context tray, external file attachments, typed
multimodal image input with capability preflight, candidate-native Review/Diff,
the command palette, and the task/tool inspector. Release acceptance uses the
packaged binary, a localhost deterministic provider fixture, exact runtime
provenance, and automated PTY coverage. The release gate requires 90
assertions with real-container verification.

Deferred features are clipboard image attachment, the full-screen Review TUI,
side-by-side diff, and Batch 6 sandbox-host compatibility. Image requests send
original image bytes; images are not re-encoded and EXIF/GPS metadata is not
stripped. This behavior is disclosed to users.
