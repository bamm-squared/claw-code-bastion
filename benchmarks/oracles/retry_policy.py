#!/usr/bin/env python3
"""Hidden semantic oracle for the retry-policy benchmark."""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


PRESERVED = {
    "Cargo.toml": "[package]\nname = \"claw_retry_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
    "src/lib.rs": "pub mod client;\npub mod config;\npub mod retry;\n",
}

HIDDEN_TEST = """use claw_retry_fixture::{client::Client, config::RetryConfig, retry};

#[test]
fn retry_policy_semantics_are_bounded_and_threaded() {
    let default = RetryConfig::default();
    assert_eq!(default.max_attempts, 3);
    assert_eq!(default.base_delay_ms, 100);
    assert_eq!(default.max_delay_ms, 500);
    assert_eq!(retry::retry_delays(&default, 5), vec![100, 200, 400, 500, 500]);

    let custom = RetryConfig {
        max_attempts: 5,
        base_delay_ms: 100,
        max_delay_ms: 250,
    };
    assert_eq!(retry::retry_delays(&custom, 4), vec![100, 200, 250, 250]);
    assert_eq!(Client::new(custom).retry_delays(4), vec![100, 200, 250, 250]);

    let zero_base = RetryConfig {
        max_attempts: 5,
        base_delay_ms: 0,
        max_delay_ms: 250,
    };
    assert_eq!(retry::retry_delays(&zero_base, 4), vec![0, 0, 0, 0]);
}
"""


def main() -> int:
    if len(sys.argv) != 2:
        return 2
    project = Path(sys.argv[1]).resolve()
    for relative, expected in PRESERVED.items():
        if (project / relative).read_text(encoding="utf-8") != expected:
            return 1
    with tempfile.TemporaryDirectory(prefix="claw-retry-oracle-") as directory:
        root = Path(directory)
        shutil.copy2(project / "Cargo.toml", root / "Cargo.toml")
        shutil.copytree(project / "src", root / "src")
        tests = root / "tests"
        tests.mkdir()
        (tests / "hidden.rs").write_text(HIDDEN_TEST, encoding="utf-8")
        result = subprocess.run(
            ["cargo", "test", "--offline", "--quiet", "--test", "hidden"],
            cwd=root,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=30,
            check=False,
        )
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
