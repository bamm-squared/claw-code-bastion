#!/usr/bin/env bash
set -euo pipefail

version="0.1.0-rc.1"
artifacts_dir=""
binary="${CLAW_BINARY:-}"
non_interactive=0

usage() {
    cat <<'EOF'
Usage: scripts/v1-acceptance.sh [options]

Run safe, staged Linux v1 acceptance checks. This helper never publishes
artifacts, pulls arbitrary images, changes shell profiles, or modifies a
project outside its disposable test fixtures.

Options:
  --version VERSION       Expected CLI version (default: 0.1.0-rc.1)
  --artifacts-dir DIR     Verify and use a locally prepared release archive
  --binary PATH           Use an already installed/local claw binary
  --non-interactive       Run deterministic checks and report manual stages
  -h, --help              Show this help
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) version="${2:?missing version}"; shift 2 ;;
        --artifacts-dir) artifacts_dir="${2:?missing artifacts directory}"; shift 2 ;;
        --binary) binary="${2:?missing binary path}"; shift 2 ;;
        --non-interactive) non_interactive=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) printf 'error: unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
done

command -v sha256sum >/dev/null 2>&1 || { printf 'error: sha256sum is required\n' >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { printf 'error: tar is required\n' >&2; exit 1; }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/claw-acceptance.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

if [ -n "$artifacts_dir" ]; then
    archive="$artifacts_dir/claw-code-bastion-v${version}-linux-x86_64.tar.gz"
    sums="$artifacts_dir/SHA256SUMS"
    [ -f "$archive" ] || { printf 'error: missing release archive %s\n' "$archive" >&2; exit 1; }
    [ -f "$sums" ] || { printf 'error: missing checksum file %s\n' "$sums" >&2; exit 1; }
    expected="$(awk -v name="$(basename "$archive")" '$2 == name || $2 == "*" name {print $1; exit}' "$sums")"
    actual="$(sha256sum "$archive" | awk '{print $1}')"
    [ -n "$expected" ] && [ "$expected" = "$actual" ] || { printf 'error: release checksum verification failed\n' >&2; exit 1; }
    tar -xzf "$archive" -C "$tmp"
    install_dir="$tmp/home/.local/bin"
    mkdir -p "$install_dir"
    install -m 0755 "$tmp/claw" "$install_dir/claw"
    binary="$install_dir/claw"
fi

if [ -z "$binary" ]; then
    binary="$(command -v claw || true)"
fi
[ -n "$binary" ] && [ -x "$binary" ] || {
    printf 'INSTALL                  NOT RUN\n'
    printf 'No release artifact or claw binary supplied.\n'
    printf 'ACCEPTANCE: NOT RUN\n'
    exit 0
}

actual_version="$($binary --version 2>/dev/null || true)"
case "$actual_version" in
    *"$version"*) version_result=PASS ;;
    *) version_result=FAIL ;;
esac

runtime_result=NOT_RUN
if CLAW_WORKER_IMAGE="${CLAW_WORKER_IMAGE:-}" CLAW_VALIDATOR_IMAGE="${CLAW_VALIDATOR_IMAGE:-}" "$binary" doctor --runtime >/dev/null 2>&1; then
    runtime_result=PASS
else
    runtime_result=FAIL
fi

printf 'Claw Code Bastion v%s Acceptance\n\n' "$version"
if [ -n "$artifacts_dir" ]; then
    printf 'Install                  PASS (temporary user prefix)\n'
else
    printf 'Install                  EXTERNAL BINARY\n'
fi
printf 'Version                  %s\n' "$version_result"
printf 'Runtime doctor           %s\n' "$runtime_result"

manual_stages=(
    "Provider onboarding" "Normal coding" "Request Changes"
    "Validation/review" "Explicit Apply" "Private mode" "MCP"
    "Hooks" "Plugins" "WebFetch" "Exit/cleanup"
)
manual_pending=0
for stage in "${manual_stages[@]}"; do
    result="MANUAL / NOT RUN"
    if [ "$non_interactive" -eq 0 ] && [ -t 0 ]; then
        printf '%s: confirm PASS, or press Enter to leave pending: ' "$stage"
        read -r answer
        case "$answer" in
            y|Y|yes|YES|pass|PASS) result=PASS ;;
        esac
    fi
    [ "$result" = PASS ] || manual_pending=1
    printf '%-25s %s\n' "$stage" "$result"
done

if [ "$version_result" = PASS ] && [ "$runtime_result" = PASS ] && [ "$manual_pending" -eq 0 ]; then
    printf '\nACCEPTANCE: PASS\n'
elif [ "$manual_pending" -eq 1 ]; then
    printf '\nACCEPTANCE: MANUAL / NOT RUN\n'
else
    printf '\nACCEPTANCE: FAIL\n'
fi
