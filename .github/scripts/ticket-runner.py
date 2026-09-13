#!/usr/bin/env python3
"""Crash-durable host wrapper for one noninteractive production ticket.

This file is evaluation orchestration, not a coding lifecycle.  The child
command remains the existing production agent-bench path.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


CHECKPOINT_IGNORED_DIRS = {
    ".git",
    ".cache",
    "build",
    "dist",
    "node_modules",
    "target",
}


def atomic_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def append_event(path: Path, event: str, **fields: Any) -> None:
    record = {"timestamp": time.time(), "event": event, **fields}
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(record, sort_keys=True) + "\n")
        stream.flush()
        os.fsync(stream.fileno())


def proc_parent_map() -> dict[int, int]:
    parents: dict[int, int] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            text = (entry / "stat").read_text(encoding="utf-8")
            _, rest = text.split(") ", 1)
            fields = rest.split()
            parents[int(entry.name)] = int(fields[1])
        except (FileNotFoundError, PermissionError, ValueError, IndexError):
            continue
    return parents


def descendants(root: int) -> set[int]:
    parents = proc_parent_map()
    children: dict[int, list[int]] = {}
    for pid, parent in parents.items():
        children.setdefault(parent, []).append(pid)
    found: set[int] = set()
    pending = list(children.get(root, []))
    while pending:
        pid = pending.pop()
        if pid in found:
            continue
        found.add(pid)
        pending.extend(children.get(pid, []))
    return found


def signal_tree(root: int, signum: int) -> None:
    pids = descendants(root)
    try:
        os.killpg(root, signum)
    except ProcessLookupError:
        pass
    except PermissionError:
        pass
    for pid in sorted(pids, reverse=True):
        try:
            os.kill(pid, signum)
        except (ProcessLookupError, PermissionError):
            pass


def project_state(work_root: Path, task_fixture: Path | None) -> dict[str, Any] | None:
    projects = sorted(path for path in work_root.glob("*/project") if path.is_dir())
    if not projects:
        return None
    project = projects[-1]
    digest = hashlib.sha256()
    files: list[str] = []
    for root, directories, names in os.walk(project):
        directories[:] = sorted(
            name for name in directories if name not in CHECKPOINT_IGNORED_DIRS
        )
        for name in sorted(names):
            path = Path(root) / name
            if not path.is_file():
                continue
            relative = path.relative_to(project).as_posix()
            files.append(relative)
            digest.update(relative.encode("utf-8"))
            digest.update(b"\0")
            try:
                digest.update(path.read_bytes())
            except OSError:
                continue
    changed: list[str] = []
    if task_fixture and task_fixture.is_file():
        try:
            document = json.loads(task_fixture.read_text(encoding="utf-8"))
            baseline = document[0].get("fixture", {}).get("files", {})
            for relative in files:
                current = project / relative
                expected = baseline.get(relative)
                if expected is None or current.read_bytes() != expected.encode("utf-8"):
                    changed.append(relative)
            changed.extend(sorted(set(baseline) - set(files)))
        except (OSError, ValueError, UnicodeError, IndexError, AttributeError):
            changed = []
    return {
        "root": str(project),
        "digest": digest.hexdigest(),
        "file_count": len(files),
        "changed_paths": sorted(set(changed)),
    }


def snapshot_project(state: dict[str, Any], snapshot: Path) -> None:
    source = Path(state["root"])
    temporary = snapshot.with_name(f".{snapshot.name}.{os.getpid()}.tmp")
    if temporary.exists():
        shutil.rmtree(temporary)
    shutil.copytree(
        source,
        temporary,
        ignore=shutil.ignore_patterns(*sorted(CHECKPOINT_IGNORED_DIRS)),
    )
    if snapshot.exists():
        shutil.rmtree(snapshot)
    os.replace(temporary, snapshot)


def telemetry_summary(path: Path) -> dict[str, Any]:
    if not path.is_file():
        return {}
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(value, dict):
            return {}
        return {
            key: value[key]
            for key in (
                "terminal_status",
                "active_wu",
                "wu_state",
                "provider_calls",
                "provider_attempts",
                "provider_retries",
                "logical_writer_turns",
                "model_turns",
                "candidate_mutations",
                "validation_result",
                "rework_cycles",
            )
            if key in value
        }
    except (OSError, ValueError, UnicodeError):
        return {}


def product_result(path: Path) -> Any:
    if not path.is_file():
        return None
    try:
        lines = [line for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
        return json.loads(lines[-1]) if lines else None
    except (OSError, ValueError, UnicodeError):
        return None


def process_alive(pid: int) -> bool:
    try:
        stat = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
        _, rest = stat.split(") ", 1)
        return rest.split()[0] != "Z"
    except (FileNotFoundError, PermissionError, ValueError, IndexError):
        return False


def watchdog_main(arguments: list[str]) -> int:
    (
        parent_pid,
        child_pid,
        timeout,
        task_id,
        journal_name,
        host_result_name,
        candidate_name,
        telemetry_name,
        heartbeat_name,
        timeout_marker_name,
    ) = arguments
    parent = int(parent_pid)
    child = int(child_pid)
    deadline = time.monotonic() + float(timeout)
    journal = Path(journal_name)
    host_result = Path(host_result_name)
    candidate_file = Path(candidate_name)
    telemetry_file = Path(telemetry_name)
    heartbeat_file = Path(heartbeat_name)
    timeout_marker = Path(timeout_marker_name)
    while time.monotonic() < deadline and not host_result.exists():
        time.sleep(0.5)
    if host_result.exists():
        return 0

    append_event(journal, "watchdog_timeout", child_pid=child, watchdog_pid=os.getpid())
    atomic_json(
        timeout_marker,
        {"task_id": task_id, "child_pid": child, "timestamp": time.time()},
    )
    signal_tree(child, signal.SIGTERM)
    time.sleep(12)
    if process_alive(child):
        append_event(journal, "watchdog_force_kill", child_pid=child, watchdog_pid=os.getpid())
        signal_tree(child, signal.SIGKILL)
    candidate = None
    if candidate_file.is_file():
        try:
            candidate = json.loads(candidate_file.read_text(encoding="utf-8"))
        except (OSError, ValueError, UnicodeError):
            candidate = None
    heartbeat = None
    if heartbeat_file.is_file():
        try:
            heartbeat = json.loads(heartbeat_file.read_text(encoding="utf-8"))
        except (OSError, ValueError, UnicodeError):
            heartbeat = None
    projection = {
        "classification": "INFRA_FAIL",
        "reason": "RUNNER_TIMEOUT",
        "task_id": task_id,
        "child_pid": child,
        "last_event": heartbeat.get("last_event") if isinstance(heartbeat, dict) else None,
        "candidate": candidate,
        "telemetry": telemetry_summary(telemetry_file),
    }
    append_event(journal, "terminal", **projection)
    atomic_json(host_result, projection)
    try:
        os.kill(parent, signal.SIGTERM)
    except ProcessLookupError:
        pass
    return 124


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "--watchdog":
        return watchdog_main(sys.argv[2:])
    parser = argparse.ArgumentParser()
    parser.add_argument("--task-id", required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--cwd", type=Path, required=True)
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("--task-file", type=Path)
    parser.add_argument("--telemetry", type=Path)
    parser.add_argument("--result", type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = list(args.command)
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("a child command is required after --")

    run_dir = args.artifacts
    run_dir.mkdir(parents=True, exist_ok=True)
    work_root = run_dir / "work"
    work_root.mkdir(parents=True, exist_ok=True)
    journal = run_dir / "lifecycle.journal.jsonl"
    heartbeat = run_dir / "heartbeat.json"
    heartbeat_history = run_dir / "heartbeat.jsonl"
    host_result = run_dir / "runner-result.json"
    snapshot = run_dir / "candidate-snapshot"
    metadata = {
        "task_id": args.task_id,
        "pid": os.getpid(),
        "cwd": str(args.cwd),
        "command": command,
        "timeout_seconds": args.timeout,
        "started_at": time.time(),
        "artifact_directory": str(run_dir),
        "source_revision": os.environ.get("CLAW_SOURCE_REVISION"),
    }
    atomic_json(run_dir / "runner-metadata.json", metadata)
    append_event(journal, "runner_started", task_id=args.task_id, pid=os.getpid())
    env = os.environ.copy()
    env["TMPDIR"] = str(work_root)
    if args.telemetry:
        env["CLAW_BENCH_TELEMETRY"] = str(args.telemetry)
    child_stdout = (run_dir / "child.stdout.log").open("ab")
    child_stderr = (run_dir / "child.stderr.log").open("ab")
    child = subprocess.Popen(
        command,
        cwd=args.cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=child_stdout,
        stderr=child_stderr,
        start_new_session=True,
    )
    append_event(journal, "child_started", pid=child.pid, process_group=child.pid)
    watchdog = subprocess.Popen(
        [
            sys.executable,
            str(Path(__file__).resolve()),
            "--watchdog",
            str(os.getpid()),
            str(child.pid),
            str(args.timeout),
            args.task_id,
            str(journal),
            str(host_result),
            str(run_dir / "candidate.json"),
            str(args.telemetry) if args.telemetry else "",
            str(heartbeat),
            str(run_dir / "watchdog-timeout.json"),
        ],
        cwd=args.cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
        close_fds=True,
    )
    last_event = "child_started"
    last_digest: str | None = None
    started = time.monotonic()
    timed_out = False
    alarm_fired = False

    def watchdog_alarm(_signum: int, _frame: Any) -> None:
        nonlocal alarm_fired
        alarm_fired = True
        signal_tree(child.pid, signal.SIGTERM)

    signal.signal(signal.SIGALRM, watchdog_alarm)
    signal.setitimer(signal.ITIMER_REAL, args.timeout)
    try:
        while child.poll() is None:
            state = project_state(work_root, args.task_file)
            if state and state["digest"] != last_digest:
                snapshot_project(state, snapshot)
                atomic_json(run_dir / "candidate.json", {**state, "snapshot": str(snapshot)})
                append_event(
                    journal,
                    "candidate_checkpoint",
                    candidate=state,
                    snapshot=str(snapshot),
                )
                last_digest = state["digest"]
                last_event = "candidate_checkpoint"
            summary = telemetry_summary(args.telemetry) if args.telemetry else {}
            heartbeat_value = {
                "timestamp": time.time(),
                "task_id": args.task_id,
                "pid": os.getpid(),
                "child_pid": child.pid,
                "stage": summary.get("terminal_status", "coding"),
                "active_wu": summary.get("active_wu"),
                "logical_turns": summary.get("logical_writer_turns", summary.get("model_turns")),
                "provider_attempts": summary.get("provider_attempts", summary.get("provider_calls")),
                "last_event": last_event,
                "elapsed_seconds": time.monotonic() - started,
            }
            atomic_json(heartbeat, heartbeat_value)
            with heartbeat_history.open("a", encoding="utf-8") as stream:
                stream.write(json.dumps(heartbeat_value, sort_keys=True) + "\n")
                stream.flush()
            if alarm_fired or time.monotonic() - started >= args.timeout:
                timed_out = True
                append_event(journal, "watchdog_timeout", child_pid=child.pid)
                signal_tree(child.pid, signal.SIGTERM)
                grace_deadline = time.monotonic() + 12
                while child.poll() is None and time.monotonic() < grace_deadline:
                    time.sleep(0.2)
                if child.poll() is None:
                    append_event(journal, "watchdog_force_kill", child_pid=child.pid)
                    signal_tree(child.pid, signal.SIGKILL)
                break
            time.sleep(0.5)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        if child.poll() is None:
            signal_tree(child.pid, signal.SIGKILL)
        try:
            child.wait(timeout=15)
        except subprocess.TimeoutExpired:
            signal_tree(child.pid, signal.SIGKILL)
        if watchdog.poll() is None:
            watchdog.terminate()
            try:
                watchdog.wait(timeout=5)
            except subprocess.TimeoutExpired:
                watchdog.kill()
        if (run_dir / "watchdog-timeout.json").is_file():
            timed_out = True
            child.wait(timeout=15)
        child_stdout.close()
        child_stderr.close()

    final_state = project_state(work_root, args.task_file)
    if final_state and final_state["digest"] != last_digest:
        snapshot_project(final_state, snapshot)
        atomic_json(run_dir / "candidate.json", {**final_state, "snapshot": str(snapshot)})
        append_event(journal, "candidate_checkpoint", candidate=final_state, snapshot=str(snapshot))
    return_code = child.returncode
    result = product_result(args.result) if args.result else None
    if timed_out:
        projection = {
            "classification": "INFRA_FAIL",
            "reason": "RUNNER_TIMEOUT",
            "task_id": args.task_id,
            "child_pid": child.pid,
            "last_event": last_event,
            "candidate": json.loads((run_dir / "candidate.json").read_text())
            if (run_dir / "candidate.json").is_file()
            else None,
            "telemetry": telemetry_summary(args.telemetry) if args.telemetry else {},
        }
    elif return_code == 0 and result is not None:
        projection = {"classification": "CHILD_TERMINAL", "task_id": args.task_id, "product_result": result}
    else:
        projection = {
            "classification": "INFRA_FAIL",
            "reason": "CHILD_EXIT_WITHOUT_TERMINAL_RESULT",
            "task_id": args.task_id,
            "child_exit_code": return_code,
            "last_event": last_event,
            "candidate": json.loads((run_dir / "candidate.json").read_text())
            if (run_dir / "candidate.json").is_file()
            else None,
            "telemetry": telemetry_summary(args.telemetry) if args.telemetry else {},
        }
    append_event(journal, "terminal", **projection)
    atomic_json(host_result, projection)
    return 124 if timed_out else (0 if return_code == 0 and result is not None else 1)


if __name__ == "__main__":
    sys.exit(main())
