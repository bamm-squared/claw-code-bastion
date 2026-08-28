use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;

const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_RESULTS: usize = 12;
const MAX_CANDIDATES: usize = 256;
const MAX_SCANNED_FILES: usize = 20_000;
const MAX_SNIPPET_CHARS: usize = 700;

#[derive(Debug, Deserialize)]
struct SearchInput {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug)]
struct Match {
    score: usize,
    path: String,
    line: usize,
    snippet: String,
    reason: String,
}

pub fn execute(input: &serde_json::Value) -> Result<String, String> {
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    execute_at(&root, input)
}

fn execute_at(workspace: &Path, input: &serde_json::Value) -> Result<String, String> {
    let input: SearchInput = serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
    let query = input.query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Err("ContextSearch query must not be empty".into());
    }
    let root = workspace
        .canonicalize()
        .map_err(|e| format!("workspace unavailable: {e}"))?;
    let search_root = input
        .path
        .as_deref()
        .map_or_else(|| Ok(root.clone()), |path| resolve_root(&root, path))?;
    let terms = query.split_whitespace().collect::<Vec<_>>();
    let mut matches = Vec::new();
    let mut scanned_files = 0;
    let mut bytes_read = 0;
    scan(
        &root,
        &search_root,
        &terms,
        &mut matches,
        &mut scanned_files,
        &mut bytes_read,
    )?;
    let limit = input.max_results.unwrap_or(5).clamp(1, MAX_RESULTS);
    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    let results = matches
        .into_iter()
        .take(limit)
        .map(|m| {
            json!({"path": m.path, "line": m.line, "snippet": m.snippet, "score": m.score, "reason": m.reason})
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&json!({
        "kind": "context_search",
        "scope": "current_workspace",
        "persistent": false,
        "network": false,
        "scanned_files": scanned_files,
        "bytes_read": bytes_read,
        "results": results,
    }))
    .map_err(|e| e.to_string())
}

fn scan(
    root: &Path,
    dir: &Path,
    terms: &[&str],
    matches: &mut Vec<Match>,
    scanned_files: &mut usize,
    bytes_read: &mut usize,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot search {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some(".git" | "target" | "node_modules" | ".claw")
            ) {
                continue;
            }
            scan(root, &path, terms, matches, scanned_files, bytes_read)?;
            continue;
        }
        if *scanned_files >= MAX_SCANNED_FILES {
            continue;
        }
        *scanned_files += 1;
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        if metadata.len() > MAX_FILE_BYTES || !is_text_file(&path) || is_sensitive_path(&path) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        *bytes_read = bytes_read.saturating_add(content.len());
        let lower = content.to_ascii_lowercase();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "search path escaped workspace".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let path_lower = relative.to_ascii_lowercase();
        let hits = terms.iter().filter(|term| lower.contains(**term)).count();
        let identifier_hits = terms
            .iter()
            .filter(|term| is_identifier(term) && lower.contains(**term))
            .count();
        let path_hits = terms
            .iter()
            .filter(|term| path_lower.contains(**term))
            .count();
        let proximity = terms
            .windows(2)
            .filter(|pair| lower.contains(&format!("{} {}", pair[0], pair[1])))
            .count();
        let score = hits * 10 + path_hits * 15 + identifier_hits * 12 + proximity * 8;
        if score == 0 {
            continue;
        }
        let line = lower.find(terms[0]).map_or(1, |offset| {
            lower[..offset].bytes().filter(|b| *b == b'\n').count() + 1
        });
        let snippet = content
            .lines()
            .skip(line.saturating_sub(1))
            .take(8)
            .collect::<Vec<_>>()
            .join("\n");
        let snippet = snippet.chars().take(MAX_SNIPPET_CHARS).collect::<String>();
        let reason = if path_hits > 0 {
            "path and lexical match"
        } else {
            "lexical match"
        };
        matches.push(Match {
            score,
            path: relative,
            line,
            snippet,
            reason: reason.into(),
        });
        if matches.len() > MAX_CANDIDATES {
            matches.sort_by(|left, right| {
                right
                    .score
                    .cmp(&left.score)
                    .then_with(|| left.path.cmp(&right.path))
            });
            matches.truncate(MAX_CANDIDATES);
        }
    }
    Ok(())
}

fn resolve_root(root: &Path, path: &str) -> Result<PathBuf, String> {
    let candidate = root.join(path);
    let canonical = candidate
        .canonicalize()
        .map_err(|e| format!("search path unavailable: {e}"))?;
    if !canonical.starts_with(root) {
        return Err("search path escapes workspace".into());
    }
    Ok(canonical)
}

fn is_text_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some(
            "rs" | "md"
                | "txt"
                | "toml"
                | "json"
                | "yaml"
                | "yml"
                | "js"
                | "ts"
                | "tsx"
                | "jsx"
                | "py"
                | "go"
                | "c"
                | "h"
                | "cpp"
                | "hpp"
                | "java"
                | "kt"
                | "swift"
                | "rb"
                | "php"
                | "sh"
                | "html"
                | "css"
                | "sql"
        )
    )
}

fn is_identifier(term: &str) -> bool {
    term.len() > 2
        && term
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
        && term
            .chars()
            .any(|character| character.is_ascii_alphabetic())
}

fn is_sensitive_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.starts_with("credentials")
        || lower.starts_with("secret")
        || matches!(
            Path::new(&lower)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("pem" | "key")
        )
        || lower == "id_rsa"
        || lower == "id_ed25519"
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{execute_at, is_sensitive_path, is_text_file};

    fn fixture() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("claw-context-test-{id}"));
        fs::create_dir_all(&root).expect("fixture directory");
        root
    }

    #[test]
    fn excludes_sensitive_files_from_indexing() {
        for path in [
            ".env",
            ".env.local",
            "credentials.json",
            "server.pem",
            "id_rsa",
        ] {
            assert!(
                is_sensitive_path(Path::new(path)),
                "indexed sensitive path {path}"
            );
        }
        assert!(!is_sensitive_path(Path::new("src/main.rs")));
    }

    #[test]
    fn only_indexes_supported_text_extensions() {
        assert!(is_text_file(Path::new("src/main.rs")));
        assert!(!is_text_file(Path::new("assets/archive.bin")));
    }

    #[test]
    fn search_is_fresh_and_skips_escape_symlinks() {
        let root = fixture();
        let outside = root.with_extension("outside");
        fs::create_dir_all(&outside).expect("outside directory");
        fs::write(outside.join("secret.txt"), "OUTSIDE_CONTEXT_TOKEN").expect("outside file");
        fs::write(root.join("main.rs"), "OLD_CONTEXT_TOKEN").expect("source file");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("outside.txt"))
            .expect("outside symlink");

        let search = |query: &str| {
            execute_at(&root, &json!({"query": query, "max_results": 12})).expect("search")
        };
        assert!(search("OLD_CONTEXT_TOKEN").contains("main.rs"));
        assert!(!search("OUTSIDE_CONTEXT_TOKEN").contains("secret.txt"));
        fs::write(root.join("main.rs"), "NEW_CONTEXT_TOKEN").expect("updated source");
        assert!(!search("OLD_CONTEXT_TOKEN").contains("main.rs"));
        assert!(search("NEW_CONTEXT_TOKEN").contains("main.rs"));
        fs::write(root.join("new.rs"), "NEW_FILE_CONTEXT_TOKEN").expect("new source");
        assert!(search("NEW_FILE_CONTEXT_TOKEN").contains("new.rs"));
        fs::remove_file(root.join("main.rs")).expect("delete source");
        assert!(!search("NEW_CONTEXT_TOKEN").contains("main.rs"));
        assert!(execute_at(&root, &json!({"query":"x", "path":"../outside"})).is_err());
        fs::remove_dir_all(root).expect("cleanup root");
        fs::remove_dir_all(outside).expect("cleanup outside");
    }

    #[test]
    fn search_reports_hard_result_bounds() {
        let root = fixture();
        for index in 0..300 {
            fs::write(
                root.join(format!("match-{index}.rs")),
                "BOUND_CONTEXT_TOKEN\n",
            )
            .expect("fixture file");
        }
        let report = execute_at(
            &root,
            &json!({"query":"BOUND_CONTEXT_TOKEN", "max_results": 12}),
        )
        .expect("bounded search");
        let value: serde_json::Value = serde_json::from_str(&report).expect("JSON report");
        assert_eq!(value["results"].as_array().expect("results").len(), 12);
        assert!(value["scanned_files"].as_u64().expect("scan count") <= 20_000);
        assert!(value["bytes_read"].as_u64().expect("bytes") <= 300 * 32);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn reports_scan_benchmark_for_representative_repositories() {
        for (label, file_count) in [("small", 20), ("medium", 200), ("large-ish", 1_000)] {
            let root = fixture();
            let source = "ValidationIdentity provider fallback network redirect authorization\n";
            for index in 0..file_count {
                fs::write(root.join(format!("module-{index}.rs")), source).expect("source file");
            }
            let input = json!({"query":"ValidationIdentity provider fallback", "max_results":12});
            let started = Instant::now();
            let cold = execute_at(&root, &input).expect("cold search");
            let cold_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let started = Instant::now();
            let repeat = execute_at(&root, &input).expect("repeat search");
            let repeat_ms = started.elapsed().as_secs_f64() * 1_000.0;
            fs::write(root.join("module-0.rs"), "NEW_CANDIDATE_TOKEN\n").expect("candidate edit");
            let started = Instant::now();
            let edited =
                execute_at(&root, &json!({"query":"NEW_CANDIDATE_TOKEN"})).expect("edit search");
            let edit_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let report: serde_json::Value = serde_json::from_str(&cold).expect("cold JSON");
            let repeat_report: serde_json::Value =
                serde_json::from_str(&repeat).expect("repeat JSON");
            assert!(edited.contains("module-0.rs"));
            println!(
                "CONTEXT_BENCHMARK {label} files={file_count} source_bytes={} scanned_files={} bytes_read={} cold_ms={cold_ms:.3} repeat_ms={repeat_ms:.3} edit_ms={edit_ms:.3}",
                file_count * source.len(),
                report["scanned_files"],
                report["bytes_read"],
            );
            assert_eq!(report["scanned_files"], repeat_report["scanned_files"]);
            fs::remove_dir_all(root).expect("cleanup");
        }
    }
}
