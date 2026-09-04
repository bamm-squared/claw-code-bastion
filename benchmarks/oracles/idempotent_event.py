#!/usr/bin/env python3
"""Hidden semantic oracle for the event-ledger benchmark."""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


PRESERVED = {
    "Cargo.toml": "[package]\nname = \"claw_event_ledger_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
    "src/lib.rs": "pub mod event;\npub mod ledger;\n",
    "src/event.rs": "#[derive(Clone, Debug, PartialEq, Eq)]\npub enum Event {\n    Deposit { id: u64, amount: u64 },\n    Withdraw { id: u64, amount: u64 },\n}\n\nimpl Event {\n    pub fn id(&self) -> u64 {\n        match self {\n            Self::Deposit { id, .. } | Self::Withdraw { id, .. } => *id,\n        }\n    }\n}\n",
}

HIDDEN_TEST = """use claw_event_ledger_fixture::{event::Event, ledger::{ApplyOutcome, Ledger, LedgerError}};

#[test]
fn idempotency_and_atomicity_are_preserved() {
    let mut ledger = Ledger::new();
    assert_eq!(ledger.apply(Event::Deposit { id: 10, amount: 80 }), Ok(ApplyOutcome::Applied));
    assert_eq!(ledger.apply(Event::Deposit { id: 10, amount: 80 }), Ok(ApplyOutcome::Duplicate));
    assert_eq!(ledger.balance(), 80);

    let error = ledger.apply(Event::Withdraw { id: 20, amount: 100 }).unwrap_err();
    assert_eq!(error, LedgerError::InsufficientFunds { requested: 100, available: 80 });
    assert_eq!(ledger.balance(), 80);

    assert_eq!(ledger.apply(Event::Deposit { id: 30, amount: 20 }), Ok(ApplyOutcome::Applied));
    assert_eq!(ledger.apply(Event::Withdraw { id: 20, amount: 100 }), Ok(ApplyOutcome::Applied));
    assert_eq!(ledger.apply(Event::Withdraw { id: 20, amount: 100 }), Ok(ApplyOutcome::Duplicate));
    assert_eq!(ledger.balance(), 0);
}
"""


def main() -> int:
    if len(sys.argv) != 2:
        return 2
    project = Path(sys.argv[1]).resolve()
    for relative, expected in PRESERVED.items():
        if (project / relative).read_text(encoding="utf-8") != expected:
            return 1
    with tempfile.TemporaryDirectory(prefix="claw-event-ledger-oracle-") as directory:
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
