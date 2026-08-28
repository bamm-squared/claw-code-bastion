use std::path::{Component, Path};
use std::process::Command;

use serde::Deserialize;
use serde_json::json;

const OUTPUT_LIMIT: usize = 24 * 1024;

#[derive(Debug, Deserialize)]
struct GitInput {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    staged: bool,
}

pub fn execute(name: &str, input: &serde_json::Value) -> Result<String, String> {
    let input: GitInput = serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
    let output = match name {
        "GitStatus" => run(&["status", "--short", "--branch"])?,
        "GitDiff" => diff(&input)?,
        "GitLog" => log(&input)?,
        "GitShow" => show(&input)?,
        "GitBlame" => blame(&input)?,
        "GitBranches" => run(&[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ])?,
        "GitChangedFiles" => run(&[
            "diff",
            "--name-status",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "HEAD",
            "--",
        ])?,
        _ => return Err(format!("unsupported Git intelligence tool: {name}")),
    };
    serde_json::to_string_pretty(&json!({
        "kind": "git_intelligence",
        "source": "trusted_workspace",
        "tool": name,
        "output": truncate(&output),
    }))
    .map_err(|e| e.to_string())
}

fn diff(input: &GitInput) -> Result<String, String> {
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-renames"];
    if input.staged {
        args.push("--cached");
    }
    if let Some(path) = &input.path {
        validate_path(path)?;
        args.extend(["--", path]);
    }
    run(&args)
}

fn log(input: &GitInput) -> Result<String, String> {
    let limit = input.limit.unwrap_or(20).clamp(1, 100).to_string();
    let mut args = vec![
        "log",
        "--no-decorate",
        "--date=short",
        "--format=%h %ad %an %s",
        "-n",
        &limit,
    ];
    if let Some(path) = &input.path {
        validate_path(path)?;
        args.extend(["--", path]);
    }
    run(&args)
}

fn show(input: &GitInput) -> Result<String, String> {
    let revision = input
        .revision
        .as_deref()
        .ok_or("GitShow requires revision")?;
    validate_revision(revision)?;
    let mut args = vec![
        "show",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--stat",
        revision,
    ];
    if let Some(path) = &input.path {
        validate_path(path)?;
        args.extend(["--", path]);
    }
    run(&args)
}

fn blame(input: &GitInput) -> Result<String, String> {
    let path = input.path.as_deref().ok_or("GitBlame requires path")?;
    validate_path(path)?;
    let limit = input.limit.unwrap_or(200).clamp(1, 1000).to_string();
    run(&["blame", "--", path]).map(|output| {
        output
            .lines()
            .take(limit.parse().unwrap_or(200))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn run(args: &[&str]) -> Result<String, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let output = Command::new("git")
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_PAGER", "cat")
        .env("GIT_EXTERNAL_DIFF", "")
        .args([
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "diff.external=",
            "-c",
            "interactive.diffFilter=",
        ])
        .args(args)
        .output()
        .map_err(|e| format!("git operation unavailable: {e}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git operation failed: {}", truncate(detail.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn validate_path(path: &str) -> Result<(), String> {
    let parsed = Path::new(path);
    if path.is_empty() || parsed.is_absolute() || path.contains('\\') || path.contains('\0') {
        return Err("Git path must be a non-empty workspace-relative path".into());
    }
    if parsed
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("Git path contains parent traversal".into());
    }
    Ok(())
}

fn validate_revision(revision: &str) -> Result<(), String> {
    if revision.is_empty()
        || revision.len() > 256
        || revision.starts_with('-')
        || revision.chars().any(char::is_whitespace)
        || revision.contains('\0')
    {
        return Err("invalid Git revision".into());
    }
    Ok(())
}

fn truncate(value: &str) -> String {
    if value.len() <= OUTPUT_LIMIT {
        return value.to_string();
    }
    format!("{}\n[output truncated]", &value[..OUTPUT_LIMIT])
}

#[cfg(test)]
mod tests {
    use super::{validate_path, validate_revision};

    #[test]
    fn rejects_unsafe_paths() {
        for path in ["../secret", "/etc/passwd", "foo\\..\\secret", ""] {
            assert!(
                validate_path(path).is_err(),
                "accepted unsafe path {path:?}"
            );
        }
        assert!(validate_path("src/lib.rs").is_ok());
    }

    #[test]
    fn rejects_revision_options_and_control_data() {
        assert!(validate_revision("--exec=evil").is_err());
        assert!(validate_revision("HEAD\n").is_err());
        assert!(validate_revision("HEAD").is_ok());
    }
}
