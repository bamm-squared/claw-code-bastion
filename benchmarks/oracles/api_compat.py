#!/usr/bin/env python3
"""Hidden compatibility oracle for the additive request-builder benchmark."""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


PRESERVED = {
    "Cargo.toml": "[package]\nname = \"claw_api_compat_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
    "src/lib.rs": "pub mod client;\npub mod request;\n",
}

HIDDEN_TEST = """use claw_api_compat_fixture::{client::Client, request::{build_request, RequestError}};

#[test]
fn old_and_new_public_apis_coexist() {
    let legacy = build_request("GET", "/health").unwrap();
    assert_eq!(legacy.method, "GET");
    assert_eq!(legacy.path, "/health");
    assert_eq!(Client::new().send(legacy.clone()), Ok(String::from("GET /health")));

    let no_query = Client::builder("GET", "/health").unwrap().build().unwrap();
    assert_eq!(no_query, legacy);

    let with_query = Client::builder("GET", "/search")
        .unwrap()
        .query_param("q", "a b")
        .query_param("filter", "x&y=z?")
        .build()
        .unwrap();
    assert_eq!(with_query.path, "/search?q=a%20b&filter=x%26y%3Dz%3F");
    assert_eq!(Client::new().send(with_query), Ok(String::from("GET /search?q=a%20b&filter=x%26y%3Dz%3F")));

    assert_eq!(build_request("TRACE", "/health"), Err(RequestError::UnsupportedMethod));
    assert_eq!(build_request("GET", ""), Err(RequestError::EmptyPath));
    assert!(Client::builder("TRACE", "/health").is_err());
    assert!(Client::builder("GET", "").is_err());
}
"""


def main() -> int:
    if len(sys.argv) != 2:
        return 2
    project = Path(sys.argv[1]).resolve()
    for relative, expected in PRESERVED.items():
        if (project / relative).read_text(encoding="utf-8") != expected:
            return 1
    with tempfile.TemporaryDirectory(prefix="claw-api-compat-oracle-") as directory:
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
