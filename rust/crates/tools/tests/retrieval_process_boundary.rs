use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use runtime::{create_disposable_snapshot, PermissionMode, PermissionPolicy, Session};
use serde_json::json;
use tools::GlobalToolRegistry;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const CANARY: &str = "RESUME_RETRIEVAL_CANARY_7f31";
const PRIVATE_CANARY: &str = "PRIVATE_RETRIEVAL_CANARY_91ab";

#[test]
fn process_child_entry() {
    match std::env::var("CLAW_RETRIEVAL_CHILD").as_deref() {
        Ok("resume-stage-1") => resume_stage_one(),
        Ok("resume-stage-2") => resume_stage_two(),
        Ok("private-stage") => private_stage(),
        _ => {}
    }
}

#[test]
fn retrieval_resume_uses_current_task_authority_after_process_exit() {
    let root = unique_temp_dir("retrieval-resume-process");
    let canonical = root.join("canonical");
    let state = root.join("state");
    fs::create_dir_all(&canonical).expect("canonical directory");
    fs::create_dir_all(&state).expect("state directory");
    fs::write(canonical.join("source.rs"), "canonical source\n").expect("canonical source");
    let session_path = state.join("session.jsonl");
    let report_path = state.join("stage-1-report");

    run_child(
        "resume-stage-1",
        &[
            ("CLAW_RETRIEVAL_ROOT", &root),
            ("CLAW_RETRIEVAL_CANONICAL", &canonical),
            ("CLAW_RETRIEVAL_SESSION", &session_path),
            ("CLAW_RETRIEVAL_REPORT", &report_path),
        ],
    );
    assert!(report_path.exists(), "stage one did not report completion");
    let session = Session::load_from_path(&session_path).expect("session should persist");
    assert_eq!(session.messages.len(), 1, "conversation should persist");
    assert!(!fs::read(&session_path)
        .expect("session bytes")
        .windows(CANARY.len())
        .any(|w| w == CANARY.as_bytes()));

    run_child(
        "resume-stage-2",
        &[
            ("CLAW_RETRIEVAL_ROOT", &root),
            ("CLAW_RETRIEVAL_CANONICAL", &canonical),
            ("CLAW_RETRIEVAL_SESSION", &session_path),
            ("CLAW_RETRIEVAL_REPORT", &report_path),
        ],
    );
    assert_eq!(
        fs::read_to_string(&report_path).expect("resume report"),
        "resumed\n"
    );
    assert!(!contains_bytes(&state, CANARY.as_bytes()));
    fs::remove_dir_all(root).expect("cleanup");
    println!("RETRIEVAL_RESUME_BOUNDARY session_resumed=YES stale_candidate=NO");
}

#[test]
fn private_retrieval_canary_does_not_persist_across_process_exit() {
    let root = unique_temp_dir("private-retrieval-process");
    let canonical = root.join("canonical");
    let state = root.join("state");
    fs::create_dir_all(&canonical).expect("canonical directory");
    fs::create_dir_all(&state).expect("state directory");
    fs::write(canonical.join("source.rs"), "private canonical source\n").expect("source");

    run_child(
        "private-stage",
        &[
            ("CLAW_RETRIEVAL_ROOT", &root),
            ("CLAW_RETRIEVAL_CANONICAL", &canonical),
            ("CLAW_RETRIEVAL_STATE", &state),
        ],
    );
    assert!(!contains_bytes(&state, PRIVATE_CANARY.as_bytes()));
    println!("PRIVATE_RETRIEVAL_CANARY_PERSISTENCE private=YES found=YES persisted=NO");
    fs::remove_dir_all(root).expect("cleanup");
}

fn resume_stage_one() {
    let canonical = env_path("CLAW_RETRIEVAL_CANONICAL");
    let session_path = env_path("CLAW_RETRIEVAL_SESSION");
    let report_path = env_path("CLAW_RETRIEVAL_REPORT");
    let task = create_disposable_snapshot(&canonical).expect("candidate snapshot");
    fs::write(task.candidate.root.join("candidate.rs"), CANARY).expect("candidate mutation");
    let found = execute_context_search(&task.candidate.root, CANARY);
    assert!(found, "candidate canary should be searchable before exit");
    let mut session = Session::new()
        .with_workspace_root(canonical)
        .with_persistence_path(session_path.clone());
    session
        .push_user_text("resume integration metadata")
        .expect("session message");
    session
        .save_to_path(&session_path)
        .expect("session should save");
    fs::write(report_path, "candidate-found\n").expect("stage report");
    task.discard().expect("candidate cleanup");
}

fn resume_stage_two() {
    let canonical = env_path("CLAW_RETRIEVAL_CANONICAL");
    let session_path = env_path("CLAW_RETRIEVAL_SESSION");
    let report_path = env_path("CLAW_RETRIEVAL_REPORT");
    let session = Session::load_from_path(&session_path).expect("resumed session");
    assert_eq!(
        session.messages.len(),
        1,
        "conversation metadata should resume"
    );
    let task = create_disposable_snapshot(&canonical).expect("fresh task snapshot");
    assert!(!execute_context_search(&task.candidate.root, CANARY));
    task.discard().expect("candidate cleanup");
    fs::write(report_path, "resumed\n").expect("resume report");
}

fn private_stage() {
    assert_eq!(std::env::var("CLAW_PRIVATE_MODE").as_deref(), Ok("1"));
    let canonical = env_path("CLAW_RETRIEVAL_CANONICAL");
    let state = env_path("CLAW_RETRIEVAL_STATE");
    let task = create_disposable_snapshot(&canonical).expect("private candidate snapshot");
    fs::write(task.candidate.root.join("private.rs"), PRIVATE_CANARY)
        .expect("private candidate mutation");
    assert!(execute_context_search(&task.candidate.root, PRIVATE_CANARY));
    assert!(!state.join("sessions").exists());
    task.discard().expect("private candidate cleanup");
}

fn execute_context_search(workspace: &Path, query: &str) -> bool {
    std::env::set_current_dir(workspace).expect("ContextSearch workspace");
    let policy = PermissionPolicy::new(PermissionMode::ReadOnly)
        .with_tool_requirement("ContextSearch", PermissionMode::ReadOnly);
    let registry = GlobalToolRegistry::builtin().with_enforcer(
        runtime::permission_enforcer::PermissionEnforcer::new(policy),
    );
    let output = registry
        .execute("ContextSearch", &json!({"query": query, "max_results": 12}))
        .expect("ContextSearch dispatch");
    output.contains(query)
}

fn run_child(stage: &str, paths: &[(&str, &Path)]) {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .arg("--exact")
        .arg("process_child_entry")
        .env_clear();
    command.env("PATH", "/usr/bin:/bin");
    command.env("CLAW_RETRIEVAL_CHILD", stage);
    if stage == "private-stage" {
        command.env("CLAW_PRIVATE_MODE", "1");
    }
    for (key, path) in paths {
        command.env(key, path);
    }
    let mut child = command.spawn().expect("child process should launch");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().expect("child status").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .expect("child output after timeout");
            panic!(
                "child {stage} timed out after 20s:\nstdout={}\nstderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("child output");
    assert!(
        output.status.success(),
        "child {stage} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn env_path(key: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(key).expect("child path environment"))
}

fn contains_bytes(root: &Path, needle: &[u8]) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            contains_bytes(&path, needle)
        } else {
            fs::read(path).is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
        }
    })
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "claw-{label}-{}-{timestamp}-{counter}",
        std::process::id()
    ))
}
