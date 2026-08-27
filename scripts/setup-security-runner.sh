#!/usr/bin/env bash
set -u

printf '%s\n' 'Claw security-runner requirements'
printf 'user: '; id -un
printf 'uid: '; id -u
printf 'distribution: '; . /etc/os-release 2>/dev/null && printf '%s\n' "${PRETTY_NAME:-unknown}" || printf '%s\n' unknown
printf 'kernel: '; uname -sr

missing=0
for command in cargo rustc git podman unshare; do
    if command -v "$command" >/dev/null 2>&1; then
        printf 'PASS  %-28s %s\n' "$command" "$(command -v "$command")"
    else
        printf 'FAIL  %-28s missing\n' "$command"
        missing=$((missing + 1))
    fi
done

uid="$(id -u)"
runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$uid}"
if [ -d "$runtime_dir" ] && [ -w "$runtime_dir" ]; then
    printf 'PASS  %-28s %s\n' 'writable runtime directory' "$runtime_dir"
else
    printf 'FAIL  %-28s %s\n' 'writable runtime directory' "$runtime_dir"
    missing=$((missing + 1))
fi

if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
    printf 'PASS  %-28s cgroup v2\n' 'cgroup hierarchy'
else
    printf 'FAIL  %-28s cgroup v2 required\n' 'cgroup hierarchy'
    missing=$((missing + 1))
fi

if [ -r /etc/subuid ] && rg -q "^$(id -un):" /etc/subuid; then printf '%s\n' 'PASS  subordinate UID range'; else printf '%s\n' 'WARN  subordinate UID range not found'; fi
if [ -r /etc/subgid ] && rg -q "^$(id -un):" /etc/subgid; then printf '%s\n' 'PASS  subordinate GID range'; else printf '%s\n' 'WARN  subordinate GID range not found'; fi

if command -v podman >/dev/null 2>&1; then
    rootless="$(podman info --format '{{.Host.Security.Rootless}}' 2>/dev/null || true)"
    if [ "$rootless" = true ]; then printf '%s\n' 'PASS  rootless Podman configuration'; else printf '%s\n' 'FAIL  rootless Podman configuration'; missing=$((missing + 1)); fi
fi

printf '%s\n' '' 'This script does not change system configuration.'
printf '%s\n' 'Install missing packages using your approved host-management process.'
printf '%s\n' 'Ubuntu 24.04 commonly needs: podman uidmap fuse-overlayfs git build-essential curl.'
printf '%s\n' 'Start a real systemd user session before running the container preflight.'

if [ "$missing" -eq 0 ]; then
    printf '%s\n' 'Host prerequisites appear present; run test-real-isolation.sh for the real container probe.'
    exit 0
fi
printf '%s\n' "Host prerequisites incomplete ($missing required checks failed)."
exit 1
