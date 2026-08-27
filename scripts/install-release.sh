#!/usr/bin/env bash
set -euo pipefail

repo="${CLAW_REPOSITORY:-bamm-squared/claw-code-bastion}"
version="${CLAW_VERSION:-}"
install_dir="${CLAW_INSTALL_DIR:-$HOME/.local/bin}"

usage() {
    cat <<'EOF'
Usage: scripts/install-release.sh [--version VERSION] [--install-dir DIR]

Downloads a Linux x86_64 Claw release, verifies SHA-256, and installs it to
$HOME/.local/bin by default. It does not require sudo or modify shell profiles.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) version="${2:?missing version}"; shift 2 ;;
        --install-dir) install_dir="${2:?missing install directory}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'error: unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

command -v curl >/dev/null 2>&1 || { printf 'error: curl is required\n' >&2; exit 1; }
command -v sha256sum >/dev/null 2>&1 || { printf 'error: sha256sum is required\n' >&2; exit 1; }

if [ -z "$version" ]; then
    release_json="$(curl -fsSL "https://api.github.com/repos/${repo}/releases/latest")"
    version="$(printf '%s' "$release_json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\([^"]*\)".*/\1/p' | head -n 1)"
fi
[ -n "$version" ] || { printf 'error: could not determine a release version\n' >&2; exit 1; }

base="https://github.com/${repo}/releases/download/v${version}"
archive="claw-code-bastion-v${version}-linux-x86_64.tar.gz"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/claw-install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/$archive" "$base/$archive"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
expected="$(awk -v name="$archive" '$2 == name || $2 == "*" name {print $1; exit}' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || { printf 'error: checksum entry missing for %s\n' "$archive" >&2; exit 1; }
actual="$(sha256sum "$tmp/$archive" | awk '{print $1}')"
[ "$actual" = "$expected" ] || { printf 'error: checksum verification failed\n' >&2; exit 1; }

mkdir -p "$install_dir"
tar -xzf "$tmp/$archive" -C "$tmp"
install -m 0755 "$tmp/claw" "$install_dir/claw"
printf 'Installed Claw Code Bastion v%s to %s/claw\n' "$version" "$install_dir"
case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *) printf 'Add %s to PATH to run `claw` directly.\n' "$install_dir" ;;
esac
