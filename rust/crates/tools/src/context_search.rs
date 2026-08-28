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
        let Ok(raw) = fs::read(&path) else {
            continue;
        };
        if raw.contains(&0) {
            continue;
        }
        let Ok(content) = String::from_utf8(raw) else {
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

    use runtime::{apply_approved_changes, create_disposable_snapshot};

    use super::{execute_at, is_sensitive_path, is_text_file, scan};

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

    #[test]
    fn candidate_request_changes_and_apply_keep_retrieval_current() {
        let canonical = fixture();
        fs::write(canonical.join("source.rs"), "ORIGINAL_TOKEN\n").expect("canonical source");
        let task = create_disposable_snapshot(&canonical).expect("candidate snapshot");
        let search = |root: &Path, query: &str| {
            execute_at(root, &json!({"query": query, "max_results": 12})).expect("search")
        };

        fs::write(
            task.candidate.root.join("source.rs"),
            "FIRST_CANDIDATE_TOKEN\n",
        )
        .expect("first candidate edit");
        assert!(search(&task.candidate.root, "FIRST_CANDIDATE_TOKEN").contains("source.rs"));
        let first_review = task.scan().expect("first review");

        fs::write(
            task.candidate.root.join("source.rs"),
            "SECOND_CANDIDATE_TOKEN\n",
        )
        .expect("request-changes edit");
        assert!(search(&task.candidate.root, "SECOND_CANDIDATE_TOKEN").contains("source.rs"));
        assert!(!search(&task.candidate.root, "FIRST_CANDIDATE_TOKEN").contains("source.rs"));
        let original: serde_json::Value =
            serde_json::from_str(&search(&task.candidate.root, "ORIGINAL_TOKEN"))
                .expect("original report");
        assert!(original["results"]
            .as_array()
            .expect("original results")
            .is_empty());

        let final_review = task.scan().expect("final review");
        apply_approved_changes(
            &final_review,
            &task.canonical,
            &task.baseline,
            &task.candidate,
        )
        .expect("apply reviewed candidate");
        assert_eq!(
            fs::read_to_string(canonical.join("source.rs")).expect("applied source"),
            "SECOND_CANDIDATE_TOKEN\n"
        );
        assert!(search(&canonical, "SECOND_CANDIDATE_TOKEN").contains("source.rs"));
        assert!(!search(&canonical, "FIRST_CANDIDATE_TOKEN").contains("source.rs"));
        assert!(!search(&canonical, "ORIGINAL_TOKEN").contains("source.rs"));
        assert_eq!(first_review.changes.len(), final_review.changes.len());
        task.discard().expect("discard task fixture");
        fs::remove_dir_all(canonical).expect("cleanup canonical");
    }

    #[test]
    fn private_retrieval_is_local_and_non_persistent() {
        let root = fixture();
        let canary = "PRIVATE_RETRIEVAL_CANARY_7f31";
        fs::write(root.join("private.rs"), canary).expect("private source");
        let previous = std::env::var_os("CLAW_PRIVATE_MODE");
        std::env::set_var("CLAW_PRIVATE_MODE", "1");
        let report = execute_at(&root, &json!({"query": canary})).expect("private search");
        match previous {
            Some(value) => std::env::set_var("CLAW_PRIVATE_MODE", value),
            None => std::env::remove_var("CLAW_PRIVATE_MODE"),
        }
        let value: serde_json::Value = serde_json::from_str(&report).expect("search report");
        assert_eq!(value["persistent"], false);
        assert_eq!(value["network"], false);
        assert!(report.contains(canary));
        let persisted = fs::read_dir(&root)
            .expect("state fixture")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path != &root.join("private.rs"))
            .collect::<Vec<_>>();
        assert!(
            persisted.is_empty(),
            "private retrieval created state: {persisted:?}"
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn retrieval_resource_bounds_cover_large_fixture_and_file_size() {
        let root = fixture();
        for index in 0..20_050 {
            fs::write(
                root.join(format!("module-{index:05}.rs")),
                "CEILING_CONTEXT_TOKEN\n",
            )
            .expect("ceiling file");
        }
        fs::write(
            root.join("below-limit.rs"),
            "BELOW_LIMIT_TOKEN\n".repeat(8_000),
        )
        .expect("below-limit file");
        fs::write(
            root.join("above-limit.rs"),
            format!(
                "ALLOWED_PREFIX_TOKEN\n{}OVER_LIMIT_TOKEN\n",
                "x".repeat(512 * 1024)
            ),
        )
        .expect("above-limit file");
        let report = execute_at(
            &root,
            &json!({"query":"CEILING_CONTEXT_TOKEN", "max_results":12}),
        )
        .expect("ceiling search");
        let value: serde_json::Value = serde_json::from_str(&report).expect("ceiling report");
        assert!(value["scanned_files"].as_u64().expect("scanned files") <= 20_000);
        assert_eq!(value["results"].as_array().expect("results").len(), 12);
        assert!(value["bytes_read"].as_u64().expect("bytes read") <= 20_000 * 32);

        let size_root = fixture();
        fs::write(
            size_root.join("below-limit.rs"),
            "BELOW_LIMIT_TOKEN\n".repeat(8_000),
        )
        .expect("below-limit file");
        fs::write(
            size_root.join("above-limit.rs"),
            format!(
                "ALLOWED_PREFIX_TOKEN\n{}OVER_LIMIT_TOKEN\n",
                "x".repeat(512 * 1024)
            ),
        )
        .expect("above-limit file");
        let size_report =
            execute_at(&size_root, &json!({"query":"OVER_LIMIT_TOKEN"})).expect("size search");
        let size_value: serde_json::Value = serde_json::from_str(&size_report).expect("size JSON");
        assert!(size_value["results"]
            .as_array()
            .expect("size results")
            .is_empty());
        assert!(size_value["bytes_read"].as_u64().expect("size bytes") < 512 * 1024);
        fs::remove_dir_all(size_root).expect("cleanup size fixture");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn retrieval_benchmarks_five_and_twenty_thousand_files() {
        for (label, file_count) in [("5k", 5_000), ("20k", 20_000)] {
            let root = fixture();
            let source = "ValidationIdentity provider fallback network redirect authorization\n";
            for index in 0..file_count {
                let path = root.join(format!("src/module-{index:05}.rs"));
                if index == 0 {
                    fs::create_dir_all(path.parent().expect("source parent")).expect("src dir");
                }
                fs::write(path, source).expect("benchmark source");
            }
            let input = json!({"query":"ValidationIdentity provider fallback", "max_results":12});
            let started = Instant::now();
            let cold = execute_at(&root, &input).expect("cold benchmark");
            let cold_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let started = Instant::now();
            let repeat = execute_at(&root, &input).expect("repeat benchmark");
            let repeat_ms = started.elapsed().as_secs_f64() * 1_000.0;
            fs::write(root.join("src/module-00000.rs"), "EDIT_REFRESH_TOKEN\n")
                .expect("benchmark edit");
            let started = Instant::now();
            let edit =
                execute_at(&root, &json!({"query":"EDIT_REFRESH_TOKEN"})).expect("edit benchmark");
            let edit_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let value: serde_json::Value = serde_json::from_str(&cold).expect("cold report");
            let repeat_value: serde_json::Value =
                serde_json::from_str(&repeat).expect("repeat report");
            let edit_value: serde_json::Value = serde_json::from_str(&edit).expect("edit report");
            assert_eq!(value["scanned_files"], repeat_value["scanned_files"]);
            assert!(edit_value["results"]
                .as_array()
                .expect("edit results")
                .iter()
                .any(|item| item["path"] == "src/module-00000.rs"));
            println!(
                "CONTEXT_BENCHMARK {label} files={file_count} source_bytes={} scanned_files={} bytes_read={} cold_ms={cold_ms:.3} repeat_ms={repeat_ms:.3} edit_ms={edit_ms:.3}",
                file_count * source.len(),
                value["scanned_files"],
                value["bytes_read"],
            );
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn retrieval_excludes_ignored_binary_unicode_and_handles_long_lines() {
        let root = fixture();
        fs::create_dir_all(root.join("target/generated")).expect("generated directory");
        fs::create_dir_all(root.join("node_modules/pkg")).expect("node modules");
        fs::create_dir_all(root.join(".git")).expect("git directory");
        fs::write(
            root.join("target/generated/IGNORED_TOKEN.rs"),
            "IGNORED_TOKEN",
        )
        .expect("ignored");
        fs::write(
            root.join("node_modules/pkg/DEPENDENCY_TOKEN.js"),
            "DEPENDENCY_TOKEN",
        )
        .expect("dependency");
        fs::write(root.join(".git/metadata.rs"), "GIT_METADATA_TOKEN").expect("git metadata");
        fs::write(root.join("binary.rs"), b"\0BINARY_TOKEN\0").expect("binary fixture");
        fs::write(
            root.join("unicode-世界.rs"),
            "世界 UNICODE_TOKEN ".repeat(80),
        )
        .expect("unicode");
        fs::write(
            root.join("long.rs"),
            format!("BEGIN_TOKEN {} END_TOKEN", "x".repeat(10_000)),
        )
        .expect("long line");
        let search = |query: &str| {
            execute_at(&root, &json!({"query":query, "max_results":12})).expect("search")
        };
        let has_result = |report: &str, token: &str| {
            let value: serde_json::Value = serde_json::from_str(report).expect("search report");
            value["results"]
                .as_array()
                .expect("results")
                .iter()
                .any(|result| result.to_string().contains(token))
        };
        assert!(!has_result(&search("IGNORED_TOKEN"), "IGNORED_TOKEN"));
        assert!(!has_result(&search("DEPENDENCY_TOKEN"), "DEPENDENCY_TOKEN"));
        assert!(!has_result(
            &search("GIT_METADATA_TOKEN"),
            "GIT_METADATA_TOKEN"
        ));
        assert!(!has_result(&search("BINARY_TOKEN"), "BINARY_TOKEN"));
        let unicode = search("UNICODE_TOKEN");
        assert!(unicode.contains("unicode-世界.rs"));
        assert!(unicode.len() < 2_000);
        let long = search("END_TOKEN");
        let long_value: serde_json::Value = serde_json::from_str(&long).expect("long report");
        assert!(
            long_value["results"][0]["snippet"]
                .as_str()
                .expect("snippet")
                .chars()
                .count()
                <= 700
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn retrieval_ranking_and_candidate_ceiling_are_deterministic() {
        let root = fixture();
        fs::create_dir_all(root.join("src/security")).expect("security directory");
        fs::create_dir_all(root.join("src/providers")).expect("provider directory");
        fs::create_dir_all(root.join("src/network")).expect("network directory");
        fs::create_dir_all(root.join("src/candidate")).expect("candidate directory");
        fs::write(
            root.join("src/security/validation_identity.rs"),
            "struct ValidationIdentity;\n",
        )
        .expect("validation fixture");
        fs::write(
            root.join("src/providers/fallback.rs"),
            "fn provider_fallback() {}\n",
        )
        .expect("provider fixture");
        fs::write(
            root.join("src/network/redirect_authorization.rs"),
            "fn network_redirect_authorization() {}\n",
        )
        .expect("network fixture");
        fs::write(
            root.join("src/candidate/lifecycle.rs"),
            "fn candidate_lifecycle() {}\n",
        )
        .expect("candidate fixture");
        fs::write(
            root.join("incidental.rs"),
            "Validation Identity appears apart\n",
        )
        .expect("incidental fixture");

        let top_path = |query: &str| {
            let value: serde_json::Value = serde_json::from_str(
                &execute_at(&root, &json!({"query":query, "max_results":1}))
                    .expect("ranking search"),
            )
            .expect("ranking JSON");
            value["results"][0]["path"]
                .as_str()
                .expect("top path")
                .to_string()
        };
        assert_eq!(
            top_path("ValidationIdentity"),
            "src/security/validation_identity.rs"
        );
        assert_eq!(top_path("provider fallback"), "src/providers/fallback.rs");
        assert_eq!(
            top_path("network redirect authorization"),
            "src/network/redirect_authorization.rs"
        );
        assert_eq!(
            top_path("candidate lifecycle"),
            "src/candidate/lifecycle.rs"
        );

        let mut matches = Vec::new();
        let mut scanned_files = 0;
        let mut bytes_read = 0;
        for index in 0..300 {
            fs::write(
                root.join(format!("match-{index}.rs")),
                "CANDIDATE_CEILING_TOKEN\n",
            )
            .expect("candidate bound fixture");
        }
        scan(
            &root,
            &root,
            &["candidate_ceiling_token"],
            &mut matches,
            &mut scanned_files,
            &mut bytes_read,
        )
        .expect("bounded scan");
        assert!(matches.len() <= 256);
        assert_eq!(scanned_files, 305);
        assert!(bytes_read > 0);
        fs::remove_dir_all(root).expect("cleanup");
    }
}
