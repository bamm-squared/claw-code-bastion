#!/usr/bin/env python3
"""Drive the packaged CLI through the real interactive coding lifecycle."""

from __future__ import annotations

import os
import pty
import select
import signal
import subprocess
import sys
import time


TIMEOUT = 45.0


def fail(message: str, transcript: bytes) -> int:
    sys.stderr.write(f"acceptance PTY failure: {message}\n")
    sys.stderr.write(transcript.decode("utf-8", errors="replace")[-12000:])
    return 1


def main() -> int:
    if len(sys.argv) != 5:
        print(f"usage: {sys.argv[0]} BINARY PROJECT BASE_URL RUNTIME_IMAGE", file=sys.stderr)
        return 2
    binary, project, base_url, runtime_image = sys.argv[1:]
    env = os.environ.copy()
    # Rootless Podman resolves local images from the user's container storage,
    # not from the application's HOME. Keep that image-store identity while
    # isolating all Bastion-owned state below the disposable project.
    podman_data_home = env.get(
        "XDG_DATA_HOME",
        os.path.join(os.path.expanduser("~"), ".local", "share"),
    )
    env.update(
        {
            "ANTHROPIC_BASE_URL": base_url,
            "CLAW_WORKER_IMAGE": runtime_image,
            "CLAW_VALIDATOR_IMAGE": runtime_image,
            "HOME": os.path.join(project, ".acceptance-home"),
            "CLAW_CONFIG_HOME": os.path.join(project, ".acceptance-config"),
            "NO_COLOR": "1",
            "TERM": "xterm-256color",
            "PATH": "/usr/bin:/bin",
            "XDG_DATA_HOME": podman_data_home,
        }
    )
    os.makedirs(env["HOME"], exist_ok=True)
    os.makedirs(env["CLAW_CONFIG_HOME"], exist_ok=True)

    master, slave = pty.openpty()
    command = [
        binary,
        "--model",
        "sonnet",
        "--permission-mode",
        "workspace-write",
        "--allowed-tools",
        "write_file",
    ]
    process = subprocess.Popen(
        command,
        cwd=project,
        env=env,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        start_new_session=True,
        close_fds=True,
    )
    os.close(slave)
    os.set_blocking(master, False)
    transcript = bytearray()

    def read_until(needle: bytes, timeout: float = TIMEOUT) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if process.poll() is not None:
                try:
                    transcript.extend(os.read(master, 65536))
                except OSError:
                    pass
                return needle in transcript
            ready, _, _ = select.select([master], [], [], 0.25)
            if ready:
                try:
                    transcript.extend(os.read(master, 65536))
                except OSError:
                    return needle in transcript
            if needle in transcript:
                return True
        return False

    def send(value: str) -> None:
        os.write(master, value.encode())

    try:
        # Prompt styling differs between rustyline versions and terminal
        # themes. Drain startup for a bounded interval, then submit through
        # the PTY even when the visible prompt is not byte-for-byte `> `.
        read_until(b"> ", 5)
        send("PARITY_SCENARIO:write_file_allowed Create the acceptance file.\r")
        if not read_until(b"Select [a/r/d", TIMEOUT):
            return fail("candidate review prompt did not appear", transcript)
        send("a\r")
        if not read_until(b"Candidate changes applied", TIMEOUT):
            return fail("trusted Apply did not complete", transcript)
        # EOF is the line editor's supported exit path. It avoids matching a
        # stale startup prompt in the accumulated transcript while the
        # successful turn is still being flushed.
        send("\x04")
        try:
            exit_status = process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            exit_status = process.wait(timeout=15)
        if exit_status != 0:
            return fail(f"CLI exited with status {process.returncode}", transcript)
    except (OSError, subprocess.TimeoutExpired) as error:
        return fail(str(error), transcript)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
        os.close(master)

    result = os.path.join(project, "generated", "output.txt")
    try:
        with open(result, encoding="utf-8") as handle:
            content = handle.read()
    except OSError as error:
        return fail(f"expected applied result is unavailable: {error}", transcript)
    if content != "created by mock service\n":
        return fail(f"unexpected applied result: {content!r}", transcript)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
