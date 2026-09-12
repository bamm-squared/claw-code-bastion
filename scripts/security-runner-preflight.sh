#!/usr/bin/env bash
set -u

failures=0
pass() { printf 'PASS  %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1"; failures=$((failures + 1)); }

printf '%s\n' 'Claw isolation-runner preflight'
for command in podman crun newuidmap newgidmap slirp4netns git cargo rustc; do
    if command -v "$command" >/dev/null 2>&1; then
        pass "$command executable ($(command -v "$command"))"
    else
        fail "$command executable"
    fi
done

if command -v podman >/dev/null 2>&1; then
    version="$(podman --version 2>&1 || true)"
    pass "Podman version ($version)"
else
    fail "Podman version"
fi

rootless="$(podman info --format '{{.Host.Security.Rootless}}' 2>/dev/null || true)"
if [ "$rootless" = true ]; then pass "Rootless execution"; else fail "Rootless execution"; fi

runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if [ -d "$runtime_dir" ] && [ -w "$runtime_dir" ]; then pass "Writable runtime directory ($runtime_dir)"; else fail "Writable runtime directory ($runtime_dir)"; fi
if timeout 15 podman unshare cat /proc/self/uid_map 2>/dev/null | grep -q '[0-9]'; then pass "Podman user namespaces"; else fail "Podman user namespaces"; fi
if [ -f /sys/fs/cgroup/cgroup.controllers ]; then pass "cgroup v2"; else fail "cgroup v2"; fi
if [ -r /etc/subuid ] && grep -q "^$(id -un):" /etc/subuid; then pass "Subordinate UID mapping"; else fail "Subordinate UID mapping"; fi
if [ -r /etc/subgid ] && grep -q "^$(id -un):" /etc/subgid; then pass "Subordinate GID mapping"; else fail "Subordinate GID mapping"; fi
network_backend="$(podman info --format '{{.Host.NetworkBackend}}' 2>/dev/null || true)"
if [ "$network_backend" = netavark ]; then pass "Netavark network backend"; else fail "Netavark network backend"; fi
if [ "${CLAW_REQUIRE_DISPOSABLE_VALIDATOR:-0}" = 1 ]; then
    if [ "${CLAW_VALIDATOR_WORKER_CLASS:-}" = "disposable-vm" ]; then
        pass "Supported worker class (disposable-vm)"
    else
        fail "Supported worker class (set CLAW_VALIDATOR_WORKER_CLASS=disposable-vm)"
    fi
fi

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
