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
  --non-interactive       Run deterministic checks without prompting (default)
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

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
runtime_image="${CLAW_WORKER_IMAGE:-${CLAW_VALIDATOR_IMAGE:-}}"
acceptance_result=PASS

if [ "$version_result" != PASS ] || [ "$runtime_result" != PASS ]; then
    acceptance_result=FAIL
fi

printf '\nDeterministic acceptance\n'
if [ -z "$runtime_image" ]; then
    printf 'Runtime image           FAIL (CLAW_WORKER_IMAGE or CLAW_VALIDATOR_IMAGE is required)\n'
    acceptance_result=FAIL
else
    printf 'Runtime image           PASS (%s)\n' "$runtime_image"
fi

fixture="$repo_root/rust/target/release/mock-anthropic-service"
if [ ! -x "$fixture" ]; then
    if ! (cd "$repo_root/rust" && cargo build --release -p mock-anthropic-service >/dev/null); then
        printf 'Provider fixture        FAIL (mock service build failed)\n'
        acceptance_result=FAIL
    fi
fi

project="$tmp/project"
mkdir -p "$project"
git -C "$project" init -q
git -C "$project" config user.email acceptance@example.invalid
git -C "$project" config user.name 'Claw Acceptance'
printf 'acceptance baseline\n' > "$project/README.md"
git -C "$project" add README.md
git -C "$project" commit -qm baseline

mock_log="$tmp/mock-anthropic.log"
mock_pid=""
if [ -x "$fixture" ] && [ -n "$runtime_image" ]; then
    "$fixture" >"$mock_log" 2>&1 &
    mock_pid=$!
    for _ in $(seq 1 100); do
        if grep -q '^MOCK_ANTHROPIC_BASE_URL=' "$mock_log"; then break; fi
        sleep 0.1
    done
    base_url="$(sed -n 's/^MOCK_ANTHROPIC_BASE_URL=//p' "$mock_log" | head -n 1)"
    if [ -n "$base_url" ] && [ -x "$repo_root/scripts/release-acceptance-pty.py" ]; then
        if ANTHROPIC_API_KEY=acceptance-key \
            ANTHROPIC_BASE_URL="$base_url" \
            CLAW_WORKER_IMAGE="$runtime_image" \
            CLAW_VALIDATOR_IMAGE="$runtime_image" \
            python3 "$repo_root/scripts/release-acceptance-pty.py" \
                "$binary" "$project" "$base_url" "$runtime_image"; then
            printf 'Provider fixture        PASS (localhost mock)\n'
            printf 'Normal coding          PASS (tool-call loop)\n'
            printf 'Validation/review/apply PASS\n'
            printf 'Interactive PTY        PASS\n'
        else
            printf 'Provider fixture        FAIL\n'
            printf 'Normal coding          FAIL\n'
            printf 'Validation/review/apply FAIL\n'
            printf 'Interactive PTY        FAIL\n'
            acceptance_result=FAIL
        fi
    else
        printf 'Provider fixture        FAIL (fixture did not start)\n'
        printf 'Interactive PTY        FAIL\n'
        acceptance_result=FAIL
    fi
else
    printf 'Provider fixture        NOT RUN\n'
    printf 'Interactive PTY        NOT RUN\n'
    acceptance_result=FAIL
fi

if [ -n "$mock_pid" ]; then
    kill "$mock_pid" 2>/dev/null || true
    wait "$mock_pid" 2>/dev/null || true
fi

if [ -n "$runtime_image" ] && "$binary" --private doctor --privacy >/dev/null 2>&1; then
    printf 'Private policy          PASS\n'
else
    printf 'Private policy          FAIL\n'
    acceptance_result=FAIL
fi
printf 'Required manual stages  0\n'

if [ "$acceptance_result" = PASS ]; then
    printf '\nACCEPTANCE: PASS\n'
else
    printf '\nACCEPTANCE: FAIL\n'
    exit 1
fi
