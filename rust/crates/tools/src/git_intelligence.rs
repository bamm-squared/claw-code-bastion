use std::io::Read;
use std::path::{Component, Path};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

const OUTPUT_LIMIT: usize = 24 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

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
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    execute_at(&cwd, name, input)
}

fn execute_at(cwd: &Path, name: &str, input: &serde_json::Value) -> Result<String, String> {
    let input: GitInput = serde_json::from_value(input.clone()).map_err(|e| e.to_string())?;
    let output = match name {
        "GitStatus" => run_at(cwd, &["status", "--short", "--branch"])?,
        "GitDiff" => diff(cwd, &input)?,
        "GitLog" => log(cwd, &input)?,
        "GitShow" => show(cwd, &input)?,
        "GitBlame" => blame(cwd, &input)?,
        "GitBranches" => run_at(
            cwd,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                "refs/heads",
                "refs/remotes",
            ],
        )?,
        "GitChangedFiles" => run_at(
            cwd,
            &[
                "diff",
                "--name-status",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "HEAD",
                "--",
            ],
        )?,
        _ => return Err(format!("unsupported Git intelligence tool: {name}")),
    };
    serde_json::to_string_pretty(&json!({
        "kind": "git_intelligence",
        "source": "canonical_git",
        "tool": name,
        "output": truncate(&output),
    }))
    .map_err(|e| e.to_string())
}

fn diff(cwd: &Path, input: &GitInput) -> Result<String, String> {
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-renames"];
    if input.staged {
        args.push("--cached");
    }
    if let Some(path) = &input.path {
        validate_path(path)?;
        args.extend(["--", path]);
    }
    run_at(cwd, &args)
}

fn log(cwd: &Path, input: &GitInput) -> Result<String, String> {
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
    run_at(cwd, &args)
}

fn show(cwd: &Path, input: &GitInput) -> Result<String, String> {
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
        "--end-of-options",
        revision,
    ];
    if let Some(path) = &input.path {
        validate_path(path)?;
        args.extend(["--", path]);
    }
    run_at(cwd, &args)
}

fn blame(cwd: &Path, input: &GitInput) -> Result<String, String> {
    let path = input.path.as_deref().ok_or("GitBlame requires path")?;
    validate_path(path)?;
    let limit = input.limit.unwrap_or(200).clamp(1, 1000).to_string();
    run_at(cwd, &["blame", "--no-textconv", "--", path]).map(|output| {
        output
            .lines()
            .take(limit.parse().unwrap_or(200))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn run_at(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_LOCAL", "/dev/null")
        .env("GIT_PAGER", "cat")
        .env("GIT_EDITOR", ":")
        .env("GIT_EXTERNAL_DIFF", "")
        .args([
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.pager=cat",
            "-c",
            "credential.helper=",
            "-c",
            "diff.external=",
            "-c",
            "interactive.diffFilter=",
        ])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output =
        run_bounded(&mut command).map_err(|e| format!("git operation unavailable: {e}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git operation failed: {}", truncate(detail.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

struct BoundedOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_bounded(command: &mut Command) -> Result<BoundedOutput, String> {
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().ok_or("Git stdout pipe unavailable")?;
    let stderr = child.stderr.take().ok_or("Git stderr pipe unavailable")?;
    let stdout_thread = thread::spawn(|| read_bounded(stdout));
    let stderr_thread = thread::spawn(|| read_bounded(stderr));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if started.elapsed() >= COMMAND_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err("Git operation timed out".into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| "Git stdout reader failed".to_string())??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| "Git stderr reader failed".to_string())??;
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded(mut reader: impl Read) -> Result<Vec<u8>, String> {
    let mut retained = Vec::with_capacity(OUTPUT_LIMIT.min(8192));
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(retained);
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
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
        || revision.contains(':')
    {
        return Err("invalid Git revision".into());
    }
    Ok(())
}

fn truncate(value: &str) -> String {
    if value.len() <= OUTPUT_LIMIT {
        return value.to_string();
    }
    format!(
        "{}\n[output truncated]",
        value.chars().take(OUTPUT_LIMIT).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{execute_at, run_bounded, validate_path, validate_revision, OUTPUT_LIMIT};

    fn fixture() -> PathBuf {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("claw-git-test-{id}"));
        fs::create_dir_all(&root).expect("fixture directory");
        root
    }

    fn git(root: &PathBuf, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .expect("git available");
        assert!(status.success(), "git command failed: {args:?}");
    }

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

    #[test]
    fn bounded_reader_caps_large_child_output() {
        let mut command = Command::new("sh");
        command.args(["-c", "yes output | head -c 100000"]);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let output = run_bounded(&mut command).expect("bounded command");
        assert_eq!(output.stdout.len(), OUTPUT_LIMIT);
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn all_git_tools_use_sterile_local_dispatch() {
        let root = fixture();
        git(&root, &["init"]);
        git(&root, &["config", "user.email", "test@example.invalid"]);
        git(&root, &["config", "user.name", "Test"]);
        let canary = root.join("executed");
        let script = root.join("canary.sh");
        fs::write(&script, format!("#!/bin/sh\ntouch {}\n", canary.display())).expect("script");
        let mut permissions = fs::metadata(&script)
            .expect("script metadata")
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o700);
        }
        fs::set_permissions(&script, permissions).expect("script permissions");
        git(
            &root,
            &["config", "core.fsmonitor", script.to_str().unwrap()],
        );
        git(
            &root,
            &[
                "config",
                "core.hooksPath",
                root.join("hooks").to_str().unwrap(),
            ],
        );
        git(&root, &["config", "core.pager", script.to_str().unwrap()]);
        git(
            &root,
            &["config", "diff.external", script.to_str().unwrap()],
        );
        git(
            &root,
            &["config", "diff.evil.textconv", script.to_str().unwrap()],
        );
        git(
            &root,
            &["config", "credential.helper", script.to_str().unwrap()],
        );
        fs::create_dir_all(root.join("hooks")).expect("hooks");
        fs::write(root.join("hooks/pre-commit"), "#!/bin/sh\ntouch executed\n").expect("hook");
        fs::write(root.join(".gitattributes"), "*.txt diff=evil\n").expect("attributes");
        fs::write(root.join("file.txt"), "before\n").expect("file");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "fixture"]);
        fs::write(root.join("file.txt"), "after\n").expect("changed file");
        let _ = fs::remove_file(&canary);

        for (name, input) in [
            ("GitStatus", json!({})),
            ("GitDiff", json!({})),
            ("GitLog", json!({})),
            ("GitShow", json!({"revision":"HEAD"})),
            ("GitBlame", json!({"path":"file.txt"})),
            ("GitBranches", json!({})),
            ("GitChangedFiles", json!({})),
        ] {
            let _ = fs::remove_file(&canary);
            let result = execute_at(&root, name, &input).expect("Git tool should succeed");
            assert!(result.contains("canonical_git"));
            assert!(!canary.exists(), "{name} executed hostile configuration");
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn read_only_git_tools_never_contact_configured_remotes() {
        let root = fixture();
        git(&root, &["init"]);
        git(&root, &["config", "user.email", "test@example.invalid"]);
        git(&root, &["config", "user.name", "Test"]);
        fs::write(root.join("file.rs"), "fn main() {}\n").expect("source");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "fixture"]);

        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let address = listener.local_addr().expect("listener address");
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_connections = Arc::clone(&connections);
        let thread_stop = Arc::clone(&stop);
        let accept_thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        thread_connections.fetch_add(1, Ordering::Relaxed);
                        drop(stream);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        TcpStream::connect(address).expect("baseline listener connection");
        for _ in 0..100 {
            if connections.load(Ordering::Relaxed) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(connections.load(Ordering::Relaxed) > 0);
        connections.store(0, Ordering::Relaxed);
        let remote = format!("http://{address}/repo.git");
        git(&root, &["remote", "add", "origin", &remote]);
        git(&root, &["remote", "add", "secondary", &remote]);

        for (name, input) in [
            ("GitStatus", json!({})),
            ("GitDiff", json!({})),
            ("GitLog", json!({})),
            ("GitShow", json!({"revision":"HEAD"})),
            ("GitBlame", json!({"path":"file.rs"})),
            ("GitBranches", json!({})),
            ("GitChangedFiles", json!({})),
        ] {
            execute_at(&root, name, &input).expect("local Git tool");
            std::thread::sleep(Duration::from_millis(20));
            println!(
                "GIT_NETWORK_ZERO tool={name} connections={}",
                connections.load(Ordering::Relaxed)
            );
            assert_eq!(
                connections.load(Ordering::Relaxed),
                0,
                "{name} contacted remote"
            );
        }
        assert_eq!(connections.load(Ordering::Relaxed), 0);
        stop.store(true, Ordering::Relaxed);
        accept_thread.join().expect("listener thread");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn production_git_boundary_rejects_option_injection() {
        let root = fixture();
        git(&root, &["init"]);
        git(&root, &["config", "user.email", "test@example.invalid"]);
        git(&root, &["config", "user.name", "Test"]);
        fs::write(root.join("file.rs"), "fn main() {}\n").expect("source");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "fixture"]);

        for revision in [
            "--help",
            "-c",
            "--config-env=FOO",
            "--git-dir=/tmp/evil",
            "--work-tree=/tmp/evil",
            "--exec-path",
            "--no-pager",
            "--paginate",
        ] {
            assert!(
                execute_at(&root, "GitShow", &json!({"revision": revision})).is_err(),
                "accepted option-shaped revision {revision}"
            );
        }
        for path in [
            "--help",
            "-c",
            "--config-env=FOO",
            "--git-dir=/tmp/evil",
            "--work-tree=/tmp/evil",
            "--exec-path",
            "--no-pager",
            "--paginate",
        ] {
            let diff = execute_at(&root, "GitDiff", &json!({"path": path}));
            let log = execute_at(&root, "GitLog", &json!({"path": path}));
            let blame = execute_at(&root, "GitBlame", &json!({"path": path}));
            assert!(diff.is_err() || !diff.unwrap().contains("usage: git"));
            assert!(log.is_err() || !log.unwrap().contains("usage: git"));
            assert!(blame.is_err() || !blame.unwrap().contains("usage: git"));
        }
        fs::remove_dir_all(root).expect("cleanup");
    }
}
