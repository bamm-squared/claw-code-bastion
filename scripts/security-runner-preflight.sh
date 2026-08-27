#!/usr/bin/env bash
set -u

failures=0
pass() { printf 'PASS  %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1"; failures=$((failures + 1)); }

printf '%s\n' 'Claw isolation-runner preflight'
if command -v podman >/dev/null 2>&1; then
    version="$(podman --version 2>&1 || true)"
    pass "Podman executable ($version)"
else
    fail "Podman executable"
fi

rootless="$(podman info --format '{{.Host.Security.Rootless}}' 2>/dev/null || true)"
if [ "$rootless" = true ]; then pass "Rootless execution"; else fail "Rootless execution"; fi

runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [ -d "$runtime_dir" ] && [ -w "$runtime_dir" ]; then pass "Writable runtime directory ($runtime_dir)"; else fail "Writable runtime directory ($runtime_dir)"; fi
if unshare -Ur true >/dev/null 2>&1; then pass "User namespaces"; else fail "User namespaces"; fi
if [ -f /sys/fs/cgroup/cgroup.controllers ]; then pass "cgroup v2"; else fail "cgroup v2"; fi

image="${CLAW_REAL_PODMAN_IMAGE:-}"
if [ "${CLAW_PREFLIGHT_CONFIG_ONLY:-0}" = 1 ]; then
    printf '%s\n' 'Container policy probe: deferred until the test image is built.'
elif [ -z "$image" ]; then
    fail "Container creation (CLAW_REAL_PODMAN_IMAGE is unset)"
else
    fixture="$(mktemp -d "${TMPDIR:-/tmp}/claw-preflight.XXXXXX")"
    trap 'rm -rf "$fixture"' EXIT
    printf 'fixture\n' > "$fixture/input"
    if podman run --rm \
        --network=none --read-only --userns=keep-id --pid=private --ipc=private \
        --cap-drop=ALL --security-opt=no-new-privileges --pids-limit=512 \
        --tmpfs /tmp:rw,nosuid,nodev --tmpfs /home/worker:rw,nosuid,nodev \
        --mount "type=bind,src=$fixture,dst=/workspace/project,rw" \
        --workdir /workspace/project "$image" /bin/sh -lc '
            test "$(awk "/^NoNewPrivs:/ { print \$2 }" /proc/self/status)" = 1 &&
            test "$(awk "/^CapEff:/ { print \$2 }" /proc/self/status)" = 0000000000000000 &&
            ! touch /root/must-fail && touch /tmp/must-work && touch /workspace/project/output
        ' >/dev/null 2>&1; then
        pass "Container creation"
        pass "network=none"
        pass "read-only rootfs"
        pass "bind mount"
        pass "tmpfs"
        pass "no-new-privileges"
        pass "cap-drop ALL"
    else
        fail "Container creation and policy probe"
    fi
fi

if [ "$failures" -eq 0 ]; then
    printf '%s\n' 'Runner is suitable for real isolation verification.'
    exit 0
fi
printf '%s\n' 'REAL ISOLATION VERIFICATION: NOT RUN'
printf '%s\n' 'This runner does not satisfy the rootless-Podman contract.'
exit 1
