use std::fs;
use std::path::{Component, Path};
use std::process::Command;

const MAX_REFERENCES: usize = 8;
const MAX_CONTEXT_CHARS: usize = 32_000;
const MAX_FILE_CHARS: usize = 8_000;
const MAX_SYMBOL_FILES: usize = 200;
type LineRange<'a> = (&'a str, Option<(usize, usize)>);

pub fn reference_count(prompt: &str) -> usize {
    prompt
        .split_whitespace()
        .filter(|word| word.strip_prefix('@').is_some_and(is_reference_token))
        .take(MAX_REFERENCES)
        .count()
}

pub fn expand_user_references(prompt: &str) -> Result<String, String> {
    let references = prompt
        .split_whitespace()
        .filter_map(|word| word.strip_prefix('@'))
        .filter(|reference| !reference.is_empty())
        .take(MAX_REFERENCES);
    let root = std::env::current_dir()
        .map_err(|error| error.to_string())?
        .canonicalize()
        .map_err(|error| format!("workspace unavailable: {error}"))?;
    let mut bundle = String::from("\n\n[Trusted user context references]\n");
    let mut count = 0;
    for reference in references {
        let (label, content) = resolve_reference(&root, reference)?;
        let entry = format!("\n{label}\n{content}\n");
        if bundle.chars().count() + entry.chars().count() > MAX_CONTEXT_CHARS {
            return Err("context references exceed the bounded context budget".into());
        }
        bundle.push_str(&entry);
        count += 1;
    }
    if count == 0 {
        Ok(prompt.to_string())
    } else {
        Ok(format!("{prompt}{bundle}"))
    }
}

fn resolve_reference(root: &Path, reference: &str) -> Result<(String, String), String> {
    match reference {
        "git:status" => git_reference(root, "status", &["status", "--short", "--branch"]),
        "git:diff" => git_reference(
            root,
            "diff",
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--",
            ],
        ),
        value if value.starts_with("candidate:") || value.starts_with("canonical:") => {
            Err("candidate/canonical references require an active task view".into())
        }
        value if value.starts_with("symbol:") => resolve_symbol(root, &value[7..]),
        value => resolve_file(root, value),
    }
}

fn resolve_symbol(root: &Path, symbol: &str) -> Result<(String, String), String> {
    if symbol.is_empty()
        || !symbol
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err("symbol references must use a simple identifier".into());
    }
    let mut checked = 0;
    let mut hits = Vec::new();
    collect_symbol_hits(root, root, symbol, &mut checked, &mut hits)?;
    if hits.is_empty() {
        return Err(format!("symbol `{symbol}` was not found in the workspace"));
    }
    Ok((format!("[symbol:{symbol}]"), hits.join("\n")))
}

fn collect_symbol_hits(
    root: &Path,
    directory: &Path,
    symbol: &str,
    checked: &mut usize,
    hits: &mut Vec<String>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".git" | "target" | "node_modules" | ".claw")
            ) {
                continue;
            }
            collect_symbol_hits(root, &path, symbol, checked, hits)?;
            continue;
        }
        if *checked >= MAX_SYMBOL_FILES || !is_text_file(&path) {
            continue;
        }
        *checked += 1;
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for (line, value) in content.lines().enumerate() {
            if value.contains(symbol) {
                hits.push(format!(
                    "[{}:{}]\n{}",
                    path.strip_prefix(root).unwrap_or(&path).display(),
                    line + 1,
                    value.trim()
                ));
                break;
            }
        }
    }
    Ok(())
}

fn resolve_file(root: &Path, value: &str) -> Result<(String, String), String> {
    let (path_value, range) = split_line_range(value)?;
    validate_relative_path(path_value)?;
    let path = root
        .join(path_value)
        .canonicalize()
        .map_err(|error| format!("context path `{path_value}` unavailable: {error}"))?;
    if !path.starts_with(root) {
        return Err("context path escapes the workspace".into());
    }
    let content = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let lines = content.lines().collect::<Vec<_>>();
    let (start, end) = range.unwrap_or((1, lines.len().min(200)));
    if start == 0 || start > lines.len() || end < start {
        return Err("context line range is outside the file".into());
    }
    let selected = lines[start - 1..end.min(lines.len())].join("\n");
    Ok((
        format!(
            "[file:{}:{}-{}]",
            path.strip_prefix(root).unwrap_or(&path).display(),
            start,
            end.min(lines.len())
        ),
        selected.chars().take(MAX_FILE_CHARS).collect(),
    ))
}

fn git_reference(root: &Path, name: &str, args: &[&str]) -> Result<(String, String), String> {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_PAGER", "cat")
        .env("GIT_EXTERNAL_DIFF", "")
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok((
        format!("[git:{name}]"),
        String::from_utf8_lossy(&output.stdout)
            .chars()
            .take(MAX_FILE_CHARS)
            .collect(),
    ))
}

fn split_line_range(value: &str) -> Result<LineRange<'_>, String> {
    let Some(index) = value.rfind(':') else {
        return Ok((value, None));
    };
    let suffix = &value[index + 1..];
    if suffix.is_empty()
        || !suffix
            .chars()
            .all(|character| character.is_ascii_digit() || character == '-')
    {
        return Ok((value, None));
    }
    let mut parts = suffix.split('-');
    let start = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or("invalid line reference")?;
    let end = parts.next().map_or(Ok(start), |part| {
        part.parse().map_err(|_| "invalid line reference")
    })?;
    if end < start || end - start > 200 {
        return Err("context line range must be at most 200 lines".into());
    }
    Ok((&value[..index], Some((start, end))))
}

fn validate_relative_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || value.contains('\\')
        || value.contains(':')
        || value.contains('\0')
        || value.starts_with('~')
    {
        return Err("context paths must be workspace-relative".into());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("context path contains traversal".into());
    }
    Ok(())
}

fn is_text_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
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

fn is_reference_token(value: &str) -> bool {
    matches!(value, "git:status" | "git:diff")
        || value.starts_with("symbol:")
        || (!value.starts_with("candidate:") && !value.starts_with("canonical:"))
}

#[cfg(test)]
mod tests {
    use super::{
        expand_user_references, reference_count, split_line_range, validate_relative_path,
    };

    #[test]
    fn parses_bounded_line_ranges() {
        assert_eq!(
            split_line_range("src/lib.rs:12"),
            Ok(("src/lib.rs", Some((12, 12))))
        );
        assert_eq!(
            split_line_range("src/lib.rs:12-18"),
            Ok(("src/lib.rs", Some((12, 18))))
        );
    }

    #[test]
    fn rejects_external_paths() {
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("/etc/passwd").is_err());
        assert!(validate_relative_path("C:/Users/me").is_err());
    }

    #[test]
    fn leaves_plain_prompt_unchanged() {
        assert_eq!(
            expand_user_references("plain prompt").unwrap(),
            "plain prompt"
        );
    }

    #[test]
    fn counts_only_supported_reference_tokens() {
        assert_eq!(
            reference_count("email@example.com @src/main.rs @git:status"),
            2
        );
    }
}
