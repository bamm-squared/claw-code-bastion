#!/usr/bin/env python3
"""Provider-free proof of the evaluation runner's crash behavior."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / ".github/scripts/ticket-runner.py"


CHILD = r'''
import json, os, subprocess, sys, time
from pathlib import Path

mode, artifact = sys.argv[1], Path(sys.argv[2])
work = Path(os.environ["TMPDIR"]) / "fixture" / "project"
work.mkdir(parents=True, exist_ok=True)
(work / "candidate.txt").write_text("candidate mutation\n")
with (artifact / "scripted-events.jsonl").open("a") as stream:
    stream.write(json.dumps({"event": "first_mutation", "mode": mode}) + "\n")
    stream.flush()
if mode == "normal":
    (artifact / "product-result.jsonl").write_text(json.dumps({"status": "PASS", "candidate": True}) + "\n")
    raise SystemExit(0)
if mode == "crash":
    os._exit(23)
if mode == "provider-hang":
    (artifact / "provider-attempt.json").write_text(json.dumps({"attempt": 1, "state": "in_flight"}))
if mode == "hang":
    sleeper = subprocess.Popen(["sleep", "120"])
    (artifact / "grandchild.pid").write_text(str(sleeper.pid))
while True:
    time.sleep(1)
'''


def run_case(root: Path, mode: str, timeout: float) -> tuple[int, dict]:
    case = root / mode
    case.mkdir(parents=True, exist_ok=True)
    child = case / "child.py"
    child.write_text(CHILD)
    command = [
        sys.executable,
        str(RUNNER),
        "--task-id",
        f"proof-{mode}",
        "--artifacts",
        str(case),
        "--cwd",
        str(ROOT),
        "--timeout",
        str(timeout),
        "--result",
        str(case / "product-result.jsonl"),
        "--",
        sys.executable,
        str(child),
        mode,
        str(case),
    ]
    completed = subprocess.run(command, cwd=ROOT, timeout=timeout + 30)
    result = json.loads((case / "runner-result.json").read_text())
    return completed.returncode, result


def main() -> int:
    root = Path(os.environ.get("RUNNER_PROOF_DIR", str(ROOT / "runner-proof-artifacts")))
    if root.exists():
        import shutil

        shutil.rmtree(root)
    root.mkdir(parents=True)
    results = {}
    code, result = run_case(root, "normal", 10)
    assert code == 0 and result["classification"] == "CHILD_TERMINAL"
    assert (root / "normal/candidate-snapshot/candidate.txt").is_file()
    results["normal"] = result
    for mode in ("hang", "provider-hang"):
        code, result = run_case(root, mode, 2)
        assert code == 124 and result["reason"] == "RUNNER_TIMEOUT"
        assert (root / f"{mode}/candidate-snapshot/candidate.txt").is_file()
        results[mode] = result
    grandchild = root / "hang/grandchild.pid"
    if grandchild.is_file():
        pid = int(grandchild.read_text())
        time.sleep(1)
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError(f"hang grandchild {pid} survived process-group cleanup")
    code, result = run_case(root, "crash", 10)
    assert code == 1 and result["classification"] == "INFRA_FAIL"
    assert result["reason"] == "CHILD_EXIT_WITHOUT_TERMINAL_RESULT"
    assert (root / "crash/candidate-snapshot/candidate.txt").is_file()
    results["crash"] = result
    (root / "proof-summary.json").write_text(json.dumps(results, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"proof": "PASS", "cases": sorted(results)}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
