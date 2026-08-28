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
    let input: SearchInput = serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
    let query = input.query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Err("ContextSearch query must not be empty".into());
    }
    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("workspace unavailable: {e}"))?;
    let search_root = input
        .path
        .as_deref()
        .map_or_else(|| Ok(root.clone()), |path| resolve_root(&root, path))?;
    let terms = query.split_whitespace().collect::<Vec<_>>();
    let mut matches = Vec::new();
    let mut scanned_files = 0;
    scan(
        &root,
        &search_root,
        &terms,
        &mut matches,
        &mut scanned_files,
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
            scan(root, &path, terms, matches, scanned_files)?;
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
        let lower = content.to_ascii_lowercase();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "search path escaped workspace".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let path_lower = relative.to_ascii_lowercase();
        let hits = terms.iter().filter(|term| lower.contains(**term)).count();
        let path_hits = terms
            .iter()
            .filter(|term| path_lower.contains(**term))
            .count();
        let score = hits * 10 + path_hits * 15;
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
    use super::{is_sensitive_path, is_text_file};
    use std::path::Path;

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
}
