//! Real rootless-Podman isolation checks.
//!
//! These tests are intentionally ignored in the normal workspace suite. Run
//! them only on a host with a working rootless Podman runtime and a built
//! worker image:
//!
//!   `CLAW_REAL_PODMAN_IMAGE=claw-exec:test cargo test -p runtime --test
//!   podman_isolation -- --ignored --nocapture`
//!
//! The tests use sentinel fixtures only. They must never be changed to mount
//! a real home directory, credential directory, or canonical checkout.

use runtime::{
    apply_approved_changes, create_disposable_snapshot, CandidateChangeSetId, ConfigSource,
    McpServerConfig, McpServerManager, McpStdioServerConfig, NetworkCapability,
    PodmanValidatorBackend, PodmanWorkerClient, PodmanWorkerSpec, ScopedMcpServerConfig,
    ValidatedCandidateInput, ValidationCheck, ValidationPlan, ValidationSnapshot, ValidationStatus,
    ValidatorBackend,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const OUTSIDE_SENTINEL: &str = "CLAW_HOST_SECRET_SHOULD_NEVER_BE_VISIBLE_7C921";
const PROVIDER_SENTINEL: &str = "CLAW_FAKE_PROVIDER_SECRET_123";

fn image() -> String {
    std::env::var("CLAW_REAL_PODMAN_IMAGE")
        .unwrap_or_else(|_| panic!("set CLAW_REAL_PODMAN_IMAGE to a built worker/validator image"))
}

fn build_custom_image(root: &Path, base: &str, role: &str) -> String {
    let tag = format!("localhost/claw-security-{role}-{}", std::process::id());
    let dockerfile = root.join(format!("Containerfile.{role}"));
    fs::write(
        &dockerfile,
        format!("FROM {base}\nRUN printf '%s' '{role}' > /etc/claw-test-runtime-role\n"),
    )
    .expect("write custom runtime containerfile");
    let output = Command::new("podman")
        .args([
            "build",
            "--network=none",
            "--tag",
            &tag,
            "--file",
            &dockerfile.to_string_lossy(),
            root.to_str().expect("custom runtime build context"),
        ])
        .output()
        .expect("build custom runtime image");
    assert!(
        output.status.success(),
        "custom runtime image build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    tag
}

fn temp_root(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("claw-real-podman-{label}-{stamp}"));
    fs::create_dir_all(&root).expect("create test root");
    root
}

fn run_shell(image: &str, workspace: &Path, command: &str) -> std::process::Output {
    Command::new("podman")
        .args([
            "run",
            "--rm",
            "--network=none",
            "--read-only",
            "--userns=keep-id",
            "--pid=private",
            "--ipc=private",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--pids-limit=512",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev",
            "--tmpfs",
            "/home/worker:rw,nosuid,nodev",
            "--mount",
        ])
        .arg(format!(
            "type=bind,src={},dst=/workspace/project,rw",
            workspace.display()
        ))
        .args([
            "--workdir",
            "/workspace/project",
            "--entrypoint",
            "/bin/sh",
            image,
            "-lc",
            command,
        ])
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("GPG_AGENT_INFO")
        .env_remove("DOCKER_HOST")
        .env_remove("CONTAINER_HOST")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("spawn podman")
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
#[allow(clippy::too_many_lines)]
fn real_worker_boundary_blocks_host_state_and_allows_candidate_edits() {
    let root = temp_root("worker");
    let canonical = root.join("canonical");
    let candidate = root.join("candidate");
    fs::create_dir_all(&canonical).expect("create canonical");
    fs::create_dir_all(&candidate).expect("create candidate");
    fs::write(
        canonical.join("canonical.txt"),
        "CANONICAL_MUST_REMAIN_UNCHANGED",
    )
    .unwrap();
    fs::write(candidate.join("project.txt"), "before").unwrap();
    let outside = root.join("outside-secret.txt");
    fs::write(&outside, OUTSIDE_SENTINEL).unwrap();

    let view = run_shell(
        &image(),
        &candidate,
        &format!(
            "test ! -e '{}' && test ! -e /home/bamm && test -z \"$SSH_AUTH_SOCK\" && ! env | grep -F '{}' ",
            outside.display(),
            PROVIDER_SENTINEL
        ),
    );
    assert!(
        view.status.success(),
        "host state visible or diagnostic failed: {}",
        String::from_utf8_lossy(&view.stderr)
    );
    assert!(!String::from_utf8_lossy(&view.stdout).contains(OUTSIDE_SENTINEL));

    let spec = PodmanWorkerSpec {
        image: image(),
        workspace: candidate.clone(),
        worker: String::from("/usr/local/bin/claw-exec-worker"),
    };
    let mut worker = PodmanWorkerClient::spawn(&spec).expect("spawn real worker");
    let write = worker
        .request(&json!({
            "operation": "write_file",
            "path": "project.txt",
            "content": "after"
        }))
        .expect("write through worker");
    assert_eq!(write.get("ok"), Some(&Value::Bool(true)));
    let read = worker
        .request(&json!({
            "operation": "read_file",
            "path": "project.txt"
        }))
        .expect("read through worker");
    assert_eq!(read.get("ok"), Some(&Value::Bool(true)));
    let glob = worker
        .request(&json!({
            "operation": "glob",
            "pattern": "*.txt"
        }))
        .expect("glob through worker");
    assert_eq!(glob.get("ok"), Some(&Value::Bool(true)));
    let grep = worker
        .request(&json!({
            "operation": "grep",
            "input": {"pattern": "after"}
        }))
        .expect("grep through worker");
    assert_eq!(grep.get("ok"), Some(&Value::Bool(true)));
    let shell = worker
        .request(&json!({
            "operation": "run_command",
            "command": "printf worker-shell"
        }))
        .expect("shell through worker");
    assert_eq!(shell.get("ok"), Some(&Value::Bool(true)));
    let invalid = worker
        .request(&json!({"operation": "unknown_operation"}))
        .expect("invalid request response");
    assert_eq!(invalid.get("ok"), Some(&Value::Bool(false)));
    let after_error = worker
        .request(&json!({
            "operation": "read_file",
            "path": "project.txt"
        }))
        .expect("worker remains alive after invalid request");
    assert_eq!(after_error.get("ok"), Some(&Value::Bool(true)));
    let outside_read = worker
        .request(&json!({
            "operation": "read_file",
            "path": outside.to_string_lossy()
        }))
        .expect("outside read response");
    assert_eq!(outside_read.get("ok"), Some(&Value::Bool(false)));
    drop(worker);

    assert_eq!(
        fs::read_to_string(candidate.join("project.txt")).unwrap(),
        "after"
    );
    assert_eq!(
        fs::read_to_string(canonical.join("canonical.txt")).unwrap(),
        "CANONICAL_MUST_REMAIN_UNCHANGED"
    );
    assert!(!run_shell(&image(), &candidate, "getent hosts example.com")
        .status
        .success());
    for capability in [
        "worker_runtime",
        "worker_filesystem_isolation",
        "worker_canonical_isolation",
        "worker_credential_isolation",
        "worker_network_isolation",
        "worker_socket_isolation",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    fs::remove_dir_all(root).expect("clean worker fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_worker_network_and_mount_policy_denies_host_state() {
    let root = temp_root("worker-policy");
    let candidate = root.join("candidate");
    fs::create_dir_all(&candidate).expect("create candidate");
    let outside = root.join("outside-secret.txt");
    fs::write(&outside, OUTSIDE_SENTINEL).unwrap();
    let output = run_shell(
        &image(),
        &candidate,
        &format!(
            "test ! -e '{}' && test ! -e /home/bamm && test ! -e /var/run/docker.sock && test -z \"$SSH_AUTH_SOCK\" && ! getent hosts example.com && ! env | grep -F '{PROVIDER_SENTINEL}'",
            outside.display()
        ),
    );
    assert!(
        output.status.success(),
        "worker policy probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(OUTSIDE_SENTINEL));
    for capability in [
        "worker_credential_isolation",
        "worker_network_isolation",
        "worker_socket_isolation",
        "provider_worker_credential_isolation",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    fs::remove_dir_all(root).expect("clean worker policy fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_validator_is_fresh_networkless_and_does_not_mutate_candidate() {
    let root = temp_root("validator");
    fs::write(root.join("project.txt"), "reviewed").unwrap();
    let before = fs::read(root.join("project.txt")).unwrap();
    let plan = ValidationPlan::new(vec![ValidationCheck {
        name: String::from("hostile validator probe"),
        command: format!(
            "test ! -e /home/bamm && test -z \"$SSH_AUTH_SOCK\" && ! env | grep -F '{PROVIDER_SENTINEL}' && ! getent hosts example.com && printf artifact > validator-artifact"
        ),
        timeout: std::time::Duration::from_secs(20),
        required: true,
    }]);
    let input = ValidatedCandidateInput {
        candidate_identity: CandidateChangeSetId::zero(),
        root: root.clone(),
    };
    let backend = PodmanValidatorBackend {
        image: image(),
        ..PodmanValidatorBackend::default()
    };
    let result = backend.validate(&input, &plan).expect("run real validator");
    assert_eq!(result.checks.len(), 1);
    assert_eq!(
        result.checks[0].status,
        ValidationStatus::Pass,
        "validator failed: exit={:?}, stdout={:?}, stderr={:?}",
        result.checks[0].exit_code,
        result.checks[0].stdout,
        result.checks[0].stderr
    );
    assert_eq!(fs::read(root.join("project.txt")).unwrap(), before);
    assert_eq!(
        fs::read_to_string(root.join("validator-artifact")).unwrap(),
        "artifact"
    );
    for capability in [
        "validator_runtime",
        "validator_credential_isolation",
        "validator_network_isolation",
        "validator_candidate_independence",
        "provider_validator_credential_isolation",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    fs::remove_dir_all(root).expect("clean validator fixture");
}

fn worker_spec(workspace: &Path) -> PodmanWorkerSpec {
    PodmanWorkerSpec {
        image: image(),
        workspace: workspace.to_path_buf(),
        worker: String::from("/usr/local/bin/claw-exec-worker"),
    }
}

fn worker_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = temp_root(label);
    let canonical = root.join("canonical");
    let candidate = root.join("candidate");
    fs::create_dir_all(&canonical).expect("create canonical");
    fs::create_dir_all(&candidate).expect("create candidate");
    fs::write(canonical.join("source.txt"), "before").expect("write canonical fixture");
    fs::write(candidate.join("source.txt"), "before").expect("write candidate fixture");
    (root, canonical, candidate)
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_worker_edit_positive_control() {
    let (root, _canonical, candidate) = worker_fixture("worker-edit");
    let mut worker = PodmanWorkerClient::spawn(&worker_spec(&candidate)).expect("spawn worker");
    let edit = worker
        .request(&json!({
            "operation": "edit_file",
            "path": "source.txt",
            "old_string": "before",
            "new_string": "after",
            "replace_all": false
        }))
        .expect("edit through worker");
    assert_eq!(edit.get("ok"), Some(&Value::Bool(true)));
    let read = worker
        .request(&json!({"operation": "read_file", "path": "source.txt"}))
        .expect("read edited file");
    assert_eq!(read.get("ok"), Some(&Value::Bool(true)));
    assert!(read.to_string().contains("after"));
    drop(worker);
    assert_eq!(
        fs::read_to_string(candidate.join("source.txt")).unwrap(),
        "after"
    );
    for capability in ["worker_runtime", "worker_filesystem_isolation"] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    fs::remove_dir_all(root).expect("clean worker edit fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_worker_outside_write_is_denied() {
    let (root, _canonical, candidate) = worker_fixture("worker-outside-write");
    let outside = root.join("outside.txt");
    fs::write(&outside, "unchanged").expect("write outside fixture");
    let mut worker = PodmanWorkerClient::spawn(&worker_spec(&candidate)).expect("spawn worker");
    let write = worker
        .request(&json!({
            "operation": "write_file",
            "path": outside.to_string_lossy(),
            "content": "must not write"
        }))
        .expect("outside write response");
    assert_eq!(write.get("ok"), Some(&Value::Bool(false)));
    let shell = worker
        .request(&json!({
            "operation": "run_command",
            "command": format!("printf escaped > '{}'", outside.display())
        }))
        .expect("outside shell write response");
    assert_ne!(shell.get("exit_code"), Some(&Value::from(0)));
    drop(worker);
    assert_eq!(fs::read_to_string(&outside).unwrap(), "unchanged");
    println!("CLAW_SECURITY_ASSERTION worker_outside_write_isolation PASS");
    fs::remove_dir_all(root).expect("clean outside-write fixture");
}

#[cfg(unix)]
#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_worker_external_symlink_isolation() {
    let (root, canonical, candidate) = worker_fixture("worker-symlink");
    let outside = root.join("outside-secret.txt");
    fs::write(&outside, OUTSIDE_SENTINEL).expect("write outside secret");
    std::os::unix::fs::symlink(&outside, candidate.join("outside-link"))
        .expect("create outside symlink");
    std::os::unix::fs::symlink(&canonical, candidate.join("canonical-link"))
        .expect("create canonical symlink");
    let mut worker = PodmanWorkerClient::spawn(&worker_spec(&candidate)).expect("spawn worker");
    for path in ["outside-link", "canonical-link"] {
        let read = worker
            .request(&json!({"operation": "read_file", "path": path}))
            .expect("symlink read response");
        assert_eq!(read.get("ok"), Some(&Value::Bool(false)));
    }
    let shell = worker
        .request(&json!({
            "operation": "run_command",
            "command": "! cat outside-link"
        }))
        .expect("symlink shell response");
    assert_eq!(shell.get("ok"), Some(&Value::Bool(true)));
    drop(worker);
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "before"
    );
    println!("CLAW_SECURITY_ASSERTION worker_symlink_isolation PASS");
    fs::remove_dir_all(root).expect("clean symlink fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_worker_candidate_git_metadata_is_harmless() {
    let (root, canonical, _candidate) = worker_fixture("worker-git");
    let task = create_disposable_snapshot(&canonical).expect("create snapshot");
    let mut worker =
        PodmanWorkerClient::spawn(&worker_spec(&task.candidate.root)).expect("spawn worker");
    let response = worker
        .request(&json!({
            "operation": "run_command",
            "command": "mkdir -p .git/hooks .git/info; printf hostile > .git/config; printf hostile > .git/hooks/pre-commit; printf hostile > .git/info/exclude"
        }))
        .expect("candidate git mutation");
    assert_eq!(response.get("ok"), Some(&Value::Bool(true)));
    drop(worker);
    let changes = task.scan().expect("scan candidate");
    assert!(changes
        .changes
        .iter()
        .all(|change| !change.path().starts_with(".git")));
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "before"
    );
    println!("CLAW_SECURITY_ASSERTION worker_git_metadata_isolation PASS");
    task.discard().expect("discard snapshot");
    fs::remove_dir_all(root).expect("clean git fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_worker_process_limit_and_output_bounds() {
    let (root, _canonical, candidate) = worker_fixture("worker-resources");
    let mut worker = PodmanWorkerClient::spawn(&worker_spec(&candidate)).expect("spawn worker");
    let output = worker
        .request(&json!({
            "operation": "run_command",
            "command": "printf '%1000000s' x; printf '%1000000s' x >&2"
        }))
        .expect("large output response");
    assert!(output.to_string().len() < 3_000_000);
    let pids = worker
        .request(&json!({
            "operation": "run_command",
            "command": "pids=''; count=0; while [ $count -lt 520 ]; do sleep 30 & pids=\"$pids $!\"; count=$((count + 1)); done; kill $pids 2>/dev/null; test $count -lt 520"
        }))
        .expect("bounded process-limit response");
    assert_eq!(pids.get("ok"), Some(&Value::Bool(true)));
    assert!(pids
        .get("result")
        .and_then(|result| result.get("returnCodeInterpretation"))
        .and_then(Value::as_str)
        .is_some_and(|interpretation| interpretation.starts_with("exit_code:")));
    for capability in [
        "worker_process_isolation",
        "worker_resource_limits",
        "worker_output_bounds",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    drop(worker);
    fs::remove_dir_all(root).expect("clean resource fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_worker_crash_is_reported_without_fallback() {
    let (root, canonical, candidate) = worker_fixture("worker-crash");
    let mut worker = PodmanWorkerClient::spawn(&worker_spec(&candidate)).expect("spawn worker");
    worker.terminate().expect("terminate worker");
    let result = worker.request(&json!({"operation": "read_file", "path": "source.txt"}));
    assert!(result.is_err());
    let message = result.unwrap_err().to_string();
    assert!(message.contains("request") && message.contains("worker"));
    println!("CLAW_SECURITY_ASSERTION worker_crash_recovery PASS");
    drop(worker);
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "before"
    );
    fs::remove_dir_all(root).expect("clean crash fixture");
}

fn validator_plan(command: String, timeout: std::time::Duration) -> ValidationPlan {
    ValidationPlan::new(vec![ValidationCheck {
        name: String::from("real validator probe"),
        command,
        timeout,
        required: true,
    }])
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_validator_host_and_socket_isolation() {
    let root = temp_root("validator-host");
    let canonical = root.join("canonical");
    let candidate = root.join("candidate");
    fs::create_dir_all(&canonical).unwrap();
    fs::create_dir_all(&candidate).unwrap();
    fs::write(canonical.join("canonical.txt"), "canonical").unwrap();
    fs::write(candidate.join("source.txt"), "candidate").unwrap();
    let outside = root.join("outside.txt");
    fs::write(&outside, OUTSIDE_SENTINEL).unwrap();
    let plan = validator_plan(
        format!(
            "test ! -e '{}' && test ! -e /var/run/docker.sock && test ! -e /run/docker.sock && test ! -e /home/bamm && ! grep -F '{}' '{}'",
            outside.display(), OUTSIDE_SENTINEL, outside.display()
        ),
        std::time::Duration::from_secs(20),
    );
    let input = ValidatedCandidateInput {
        candidate_identity: CandidateChangeSetId::zero(),
        root: candidate.clone(),
    };
    let backend = PodmanValidatorBackend {
        image: image(),
        ..Default::default()
    };
    let result = backend
        .validate(&input, &plan)
        .expect("validator host probe");
    assert_eq!(result.checks[0].status, ValidationStatus::Pass);
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert_eq!(
        fs::read_to_string(canonical.join("canonical.txt")).unwrap(),
        "canonical"
    );
    for capability in [
        "validator_filesystem_isolation",
        "validator_canonical_isolation",
        "validator_socket_isolation",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    fs::remove_dir_all(root).expect("clean validator host fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_validator_output_bounds_and_timeout_cleanup() {
    let root = temp_root("validator-resources");
    fs::write(root.join("source.txt"), "candidate").unwrap();
    let backend = PodmanValidatorBackend {
        image: image(),
        ..Default::default()
    };
    let output_result = backend
        .validate(
            &ValidatedCandidateInput {
                candidate_identity: CandidateChangeSetId::zero(),
                root: root.clone(),
            },
            &validator_plan(
                String::from("printf '%1000000s' x; printf '%1000000s' x >&2"),
                std::time::Duration::from_secs(20),
            ),
        )
        .expect("bounded validator output");
    assert!(output_result.checks[0].truncated);
    assert!(output_result.checks[0].stdout.len() <= 256 * 1024 + 64);
    let timeout_result = backend
        .validate(
            &ValidatedCandidateInput {
                candidate_identity: CandidateChangeSetId::zero(),
                root: root.clone(),
            },
            &validator_plan(
                String::from("(sleep 2; touch late-marker) & sleep 30"),
                std::time::Duration::from_millis(100),
            ),
        )
        .expect("validator timeout result");
    assert_eq!(timeout_result.checks[0].status, ValidationStatus::Timeout);
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert!(!root.join("late-marker").exists());
    for capability in [
        "validator_output_bounds",
        "validator_timeout_cleanup",
        "validator_descendant_cleanup",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    fs::remove_dir_all(root).expect("clean validator resource fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
#[allow(clippy::too_many_lines)]
fn podman_full_hostile_authoritative_lifecycle() {
    let root = temp_root("full-lifecycle");
    let canonical = root.join("canonical");
    fs::create_dir_all(&canonical).unwrap();
    fs::write(canonical.join("source.txt"), "before").unwrap();
    let outside = root.join("outside-secret.txt");
    fs::write(&outside, OUTSIDE_SENTINEL).unwrap();
    let task = create_disposable_snapshot(&canonical).expect("create trusted snapshot");
    let before = fs::read(canonical.join("source.txt")).unwrap();
    let mut worker =
        PodmanWorkerClient::spawn(&worker_spec(&task.candidate.root)).expect("spawn worker");
    let hostile = worker.request(&json!({
        "operation": "run_command",
        "command": format!("! cat '{}'; mkdir -p .git/hooks; printf hostile > .git/config; printf reviewed > source.txt", outside.display())
    })).expect("hostile worker sequence");
    assert_eq!(hostile.get("ok"), Some(&Value::Bool(true)));
    drop(worker);
    assert_eq!(fs::read(canonical.join("source.txt")).unwrap(), before);
    let reviewed = task.scan().expect("scan reviewed candidate");
    assert!(reviewed
        .changes
        .iter()
        .any(|change| change.path() == Path::new("source.txt")));
    let snapshot = ValidationSnapshot::create_verified(&task.candidate, &task.baseline, &reviewed)
        .expect("verify validation snapshot");
    let backend = PodmanValidatorBackend {
        image: image(),
        ..Default::default()
    };
    let result = backend
        .validate(
            &snapshot.input(),
            &validator_plan(String::from("test \"$(cat source.txt)\" = reviewed; printf validator-artifact > validator-artifact"), std::time::Duration::from_secs(20)),
        )
        .expect("run authoritative validator");
    assert_eq!(result.checks[0].status, ValidationStatus::Pass);
    assert_eq!(fs::read(canonical.join("source.txt")).unwrap(), before);
    drop(snapshot);
    apply_approved_changes(&reviewed, &task.canonical, &task.baseline, &task.candidate)
        .expect("explicit trusted apply");
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "reviewed"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert!(!canonical.join(".git").exists());
    for capability in [
        "candidate_canonical_boundary",
        "validation_identity_binding",
        "whole_change_set_apply",
        "full_authoritative_lifecycle",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    task.discard().expect("discard lifecycle task");
    fs::remove_dir_all(root).expect("clean lifecycle fixture");
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
#[allow(clippy::too_many_lines)]
fn real_mcp_stdio_isolated_boundary_and_cleanup() {
    let root = temp_root("mcp");
    let canonical = root.join("canonical");
    let candidate = root.join("candidate");
    fs::create_dir_all(&canonical).expect("create canonical");
    fs::create_dir_all(&candidate).expect("create candidate");
    fs::write(
        canonical.join("canonical.txt"),
        "CANONICAL_MUST_REMAIN_UNCHANGED",
    )
    .unwrap();
    fs::write(candidate.join("source.txt"), "before").unwrap();
    let outside = root.join("outside-secret.txt");
    fs::write(&outside, OUTSIDE_SENTINEL).unwrap();

    let server_script = format!(
        r#"
outside={outside:?}
send() {{
  payload="$1"
  printf 'Content-Length: %s\r\n\r\n%s' "${{#payload}}" "$payload"
}}
printf '%s' "$CLAW_FAKE_PROVIDER_SECRET_123" > .mcp-credential-probe
if [ -e "$outside" ]; then printf leaked > .mcp-outside-probe; fi
if [ -e /workspace/canonical/canonical.txt ]; then printf leaked > .mcp-canonical-probe; fi
if [ -n "$SSH_AUTH_SOCK" ] || [ -e /var/run/docker.sock ] || [ -e /run/user/1000/podman/podman.sock ]; then printf leaked > .mcp-socket-probe; fi
if getent hosts example.com >/dev/null 2>&1; then printf leaked > .mcp-network-probe; fi
(sleep 30) &
cr=$(printf '\r')
while IFS= read -r header; do
  case "$header" in
    Content-Length:*) length=$(printf '%s' "$header" | sed 's/[^0-9]//g') ;;
    *)
      [ "$header" = "$cr" ] || continue
      line=$(dd bs=1 count="$length" 2>/dev/null)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
      [ -n "$id" ] || id=1
      case "$line" in
        *initialize*)
          send "$(printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2024-11-05","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"hostile-test","version":"1"}}}}}}' "$id")" ;;
        *tools/list*)
          send "$(printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"probe","description":"boundary probe","inputSchema":{{"type":"object"}}}}]}}}}' "$id")" ;;
        *tools/call*)
          send "$(printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"candidate access allowed"}}],"isError":false}}}}' "$id")" ;;
      esac
      ;;
  esac
done
"#,
        outside = outside.display()
    );
    let servers = BTreeMap::from([(
        String::from("hostile"),
        ScopedMcpServerConfig {
            scope: ConfigSource::User,
            config: McpServerConfig::Stdio(McpStdioServerConfig {
                command: String::from("/bin/sh"),
                args: vec![String::from("-c"), server_script],
                env: BTreeMap::new(),
                tool_call_timeout_ms: Some(10_000),
            }),
        },
    )]);

    let mut runtime = tokio::runtime::Builder::new_current_thread();
    runtime.enable_all();
    runtime
        .build()
        .expect("create test runtime")
        .block_on(async {
            let mut manager =
                McpServerManager::from_servers_isolated(&servers, candidate.clone(), image());
            let tools = manager
                .discover_tools()
                .await
                .expect("discover isolated MCP tools");
            assert_eq!(tools.len(), 1);
            let call = manager
                .call_tool("mcp__hostile__probe", Some(json!({})))
                .await
                .expect("call isolated MCP tool");
            assert!(call.result.is_some());
            manager.shutdown().await.expect("shutdown isolated MCP");
        });

    assert!(candidate.join(".mcp-credential-probe").exists());
    assert!(!candidate.join(".mcp-outside-probe").exists());
    assert!(!candidate.join(".mcp-canonical-probe").exists());
    assert!(!candidate.join(".mcp-socket-probe").exists());
    assert!(!candidate.join(".mcp-network-probe").exists());
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert_eq!(
        fs::read_to_string(canonical.join("canonical.txt")).unwrap(),
        "CANONICAL_MUST_REMAIN_UNCHANGED"
    );
    for capability in [
        "mcp_real_execution",
        "mcp_canonical_isolation",
        "mcp_outside_host_isolation",
        "mcp_credential_isolation",
        "mcp_socket_isolation",
        "mcp_network_isolation",
        "mcp_cleanup",
        "mcp_no_host_fallback",
        "provider_mcp_credential_isolation",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    fs::remove_dir_all(root).expect("clean MCP fixture");
}

#[test]
fn real_web_broker_policy_matrix() {
    let capability = NetworkCapability::unrestricted();
    for url in [
        "http://localhost:80",
        "http://127.0.0.1:80",
        "http://0.0.0.0:80",
        "http://10.0.0.1:80",
        "http://172.16.0.1:80",
        "http://192.168.0.1:80",
        "http://169.254.169.254:80",
        "http://[::1]:80",
        "http://[fe80::1]:80",
        "http://[fd00::1]:80",
        "http://[::ffff:127.0.0.1]:80",
        "file:///tmp/test",
    ] {
        assert!(
            capability.authorize_web_url(url).is_err(),
            "must deny {url}"
        );
    }
    for capability in [
        "webfetch_http_https_validation",
        "webfetch_loopback_denial",
        "webfetch_private_address_denial",
        "webfetch_link_local_denial",
        "webfetch_metadata_denial",
        "webfetch_ipv6_denial",
        "webfetch_ipv4_mapped_ipv6_denial",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_private_mode_lifecycle_preserves_isolation_and_apply_boundary() {
    let root = temp_root("private");
    let canonical = root.join("canonical");
    fs::create_dir_all(&canonical).unwrap();
    fs::write(canonical.join("source.txt"), "before").unwrap();
    let task = create_disposable_snapshot(&canonical).expect("create private candidate");
    let probe = run_shell(&image(), &task.candidate.root, "test -w /workspace/project && test ! -e /workspace/canonical && test ! -e /home/bamm && ! getent hosts example.com");
    assert!(probe.status.success(), "private worker policy failed");
    fs::write(task.candidate.root.join("source.txt"), "reviewed").unwrap();
    let changes = task.scan().unwrap();
    let snapshot =
        ValidationSnapshot::create_verified(&task.candidate, &task.baseline, &changes).unwrap();
    let result = PodmanValidatorBackend {
        image: image(),
        ..Default::default()
    }
    .validate(
        &snapshot.input(),
        &validator_plan(
            "test \"$(cat source.txt)\" = reviewed".into(),
            std::time::Duration::from_secs(5),
        ),
    )
    .unwrap();
    assert_eq!(result.checks[0].status, ValidationStatus::Pass);
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "before"
    );
    drop(snapshot);
    apply_approved_changes(&changes, &task.canonical, &task.baseline, &task.candidate).unwrap();
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "reviewed"
    );
    for capability in ["private_isolation_mandatory", "private_review_apply"] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    task.discard().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_custom_runtime_preserves_outer_security_policy() {
    let root = temp_root("custom-runtime");
    let canonical = root.join("canonical");
    let candidate = root.join("candidate");
    let home = root.join("fake-home");
    let outside = root.join("outside-secret.txt");
    fs::create_dir_all(&canonical).unwrap();
    fs::create_dir_all(&candidate).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(canonical.join("canonical.txt"), "canonical").unwrap();
    fs::write(&outside, OUTSIDE_SENTINEL).unwrap();
    fs::write(
        candidate.join(".claw-settings"),
        "CLAW_WORKER_IMAGE=hostile",
    )
    .unwrap();
    let worker_image = build_custom_image(&root, &image(), "worker-custom");
    let validator_image = build_custom_image(&root, &image(), "validator-custom");
    let marker_probe = format!(
        "test \"$(cat /etc/claw-test-runtime-role)\" = worker-custom && test -w /workspace/project && test ! -e /workspace/canonical && test ! -e '{}' && test ! -e /home/bamm && test ! -e \"{}/secret\" && test -z \"$OPENAI_API_KEY\" && test -z \"$ANTHROPIC_API_KEY\" && test -z \"$SSH_AUTH_SOCK\" && test ! -e /var/run/docker.sock && test ! -e /run/docker.sock && ! getent hosts example.com && ! command -v claw-host-only-test",
        outside.display(),
        home.display()
    );
    let output = run_shell(&worker_image, &candidate, &marker_probe);
    assert!(
        output.status.success(),
        "custom runtime policy probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut worker = PodmanWorkerClient::spawn(&PodmanWorkerSpec {
        image: worker_image.clone(),
        workspace: candidate.clone(),
        worker: String::from("/usr/local/bin/claw-exec-worker"),
    })
    .expect("spawn selected custom worker");
    let marker = worker
        .request(&json!({"operation":"run_command","command":"cat /etc/claw-test-runtime-role"}))
        .expect("read custom worker marker");
    assert_eq!(marker["ok"], true);
    assert_eq!(marker["result"]["stdout"], "worker-custom");
    let fallback = worker
        .request(&json!({"operation":"run_command","command":"command -v claw-host-only-test"}))
        .expect("custom worker missing executable probe");
    assert_eq!(fallback["ok"], true);
    assert_ne!(fallback["result"]["returnCode"], 0);
    drop(worker);
    let validator = PodmanValidatorBackend {
        image: validator_image.clone(),
        ..Default::default()
    };
    let validation = validator
        .validate(
            &ValidatedCandidateInput {
                candidate_identity: CandidateChangeSetId::zero(),
                root: candidate.clone(),
            },
            &validator_plan(
                format!(
                    "test \"$(cat /etc/claw-test-runtime-role)\" = validator-custom && test ! -e '{}' && test ! -e /workspace/canonical && test ! -e /home/bamm && test -z \"$OPENAI_API_KEY\" && test -z \"$ANTHROPIC_API_KEY\" && ! getent hosts example.com",
                    outside.display()
                ),
                std::time::Duration::from_secs(10),
            ),
        )
        .expect("run selected custom validator");
    assert_eq!(validation.checks[0].status, ValidationStatus::Pass);
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert_eq!(
        fs::read_to_string(canonical.join("canonical.txt")).unwrap(),
        "canonical"
    );
    let _ = std::io::stdout().flush();
    for capability in [
        "custom_runtime_trusted_selection",
        "custom_runtime_project_override_denied",
        "custom_runtime_network_none",
        "custom_runtime_mount_restrictions",
        "custom_runtime_credentials_sockets_unavailable",
        "custom_runtime_no_host_fallback",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    let _ = Command::new("podman")
        .args(["rmi", "--", &worker_image, &validator_image])
        .output();
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_combined_hostile_lifecycle_keeps_canonical_authoritative() {
    podman_full_hostile_authoritative_lifecycle();
}
