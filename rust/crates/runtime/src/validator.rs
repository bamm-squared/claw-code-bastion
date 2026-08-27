//! Fresh, identity-bound validation for hostile candidate workspaces.
//!
//! The validator never receives the editing candidate or the canonical
//! checkout.  It runs each check in a new networkless Podman container over a
//! disposable copy of the reviewed candidate.

use crate::snapshot::{
    scan_candidate, CandidateChangeSet, CandidateChangeSetId, TrustedBaseline, UntrustedCandidate,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

pub const MAX_CHECKS: usize = 32;
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
pub const MAX_TOTAL_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_mins(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationIdentity([u8; 32]);

impl ValidationIdentity {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for ValidationIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCheck {
    pub name: String,
    pub command: String,
    pub timeout: Duration,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationPlan {
    pub checks: Vec<ValidationCheck>,
    pub version: u32,
}

impl ValidationPlan {
    #[must_use]
    pub fn new(checks: Vec<ValidationCheck>) -> Self {
        Self {
            checks: checks.into_iter().take(MAX_CHECKS).collect(),
            version: 1,
        }
    }

    #[must_use]
    pub fn identity(&self, backend: &str) -> ValidationIdentity {
        let mut hasher = Sha256::new();
        hasher.update(self.version.to_le_bytes());
        hasher.update(backend.as_bytes());
        for check in &self.checks {
            hasher.update(check.name.as_bytes());
            hasher.update([0]);
            hasher.update(check.command.as_bytes());
            hasher.update(check.timeout.as_millis().to_le_bytes());
            hasher.update([u8::from(check.required)]);
        }
        ValidationIdentity(hasher.finalize().into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationStatus {
    Pass,
    Fail,
    Blocked,
    Timeout,
    Error,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCheckResult {
    pub name: String,
    pub required: bool,
    pub status: ValidationStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    pub candidate_identity: CandidateChangeSetId,
    pub validation_identity: ValidationIdentity,
    pub checks: Vec<ValidationCheckResult>,
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationPolicy {
    pub require_validation: bool,
    pub allow_blocked_with_warning: bool,
    pub optional_fail_blocks: bool,
}

impl Default for ValidationPolicy {
    fn default() -> Self {
        Self {
            require_validation: true,
            allow_blocked_with_warning: true,
            optional_fail_blocks: false,
        }
    }
}

impl ValidationResult {
    #[must_use]
    pub fn matches(
        &self,
        candidate: CandidateChangeSetId,
        plan: &ValidationPlan,
        backend: &str,
    ) -> bool {
        self.candidate_identity == candidate && self.validation_identity == plan.identity(backend)
    }

    #[must_use]
    pub fn allows_apply(
        &self,
        candidate: CandidateChangeSetId,
        policy: ValidationPolicy,
        blocked_override: bool,
    ) -> bool {
        if self.candidate_identity != candidate || !policy.require_validation {
            return false;
        }
        for check in &self.checks {
            let terminal_failure = matches!(
                check.status,
                ValidationStatus::Fail | ValidationStatus::Timeout | ValidationStatus::Error
            );
            let incomplete = matches!(
                check.status,
                ValidationStatus::Blocked | ValidationStatus::Skipped
            );
            if terminal_failure && (check.required || policy.optional_fail_blocks) {
                return false;
            }
            if incomplete
                && check.required
                && !(blocked_override && policy.allow_blocked_with_warning)
            {
                return false;
            }
        }
        true
    }

    #[must_use]
    pub fn blocked(
        candidate: CandidateChangeSetId,
        plan: &ValidationPlan,
        backend: &str,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            candidate_identity: candidate,
            validation_identity: plan.identity(backend),
            checks: vec![ValidationCheckResult {
                name: String::from("validator startup"),
                required: true,
                status: ValidationStatus::Blocked,
                exit_code: None,
                stdout: String::new(),
                stderr: reason.into(),
                truncated: false,
            }],
            duration: Duration::ZERO,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCandidateInput {
    pub candidate_identity: CandidateChangeSetId,
    pub root: PathBuf,
}

#[derive(Debug)]
pub struct ValidationSnapshot {
    pub root: PathBuf,
    pub candidate_identity: CandidateChangeSetId,
    task_root: PathBuf,
}

impl ValidationSnapshot {
    pub fn create(
        candidate: &UntrustedCandidate,
        identity: CandidateChangeSetId,
    ) -> io::Result<Self> {
        let source = candidate.root.canonicalize()?;
        let task_root = std::env::temp_dir().join(format!("claw-validation-{}", unique_stamp()));
        let root = task_root.join("project");
        fs::create_dir_all(&root)?;
        if let Err(error) = copy_tree(&source, &root, &source) {
            let _ = fs::remove_dir_all(&task_root);
            return Err(error);
        }
        Ok(Self {
            root,
            candidate_identity: identity,
            task_root,
        })
    }

    pub fn create_verified(
        candidate: &UntrustedCandidate,
        baseline: &TrustedBaseline,
        reviewed: &CandidateChangeSet,
    ) -> io::Result<Self> {
        if scan_candidate(baseline, candidate)?.id != reviewed.id {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "candidate changed before validation snapshot creation",
            ));
        }
        let snapshot = Self::create(candidate, reviewed.id)?;
        let snapshot_candidate = UntrustedCandidate {
            root: snapshot.root.clone(),
        };
        if scan_candidate(baseline, &snapshot_candidate)?.id != reviewed.id {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "validation snapshot does not match reviewed candidate",
            ));
        }
        Ok(snapshot)
    }

    #[must_use]
    pub fn input(&self) -> ValidatedCandidateInput {
        ValidatedCandidateInput {
            candidate_identity: self.candidate_identity,
            root: self.root.clone(),
        }
    }
}

impl Drop for ValidationSnapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.task_root);
    }
}

pub trait ValidatorBackend: Send + Sync + std::fmt::Debug {
    fn backend_id(&self) -> &'static str;
    fn validate(
        &self,
        candidate: &ValidatedCandidateInput,
        plan: &ValidationPlan,
    ) -> io::Result<ValidationResult>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodmanValidatorBackend {
    pub image: String,
    pub shell: String,
}

impl Default for PodmanValidatorBackend {
    fn default() -> Self {
        Self {
            image: crate::DEFAULT_RUNTIME_IMAGE.to_string(),
            shell: String::from("/bin/sh"),
        }
    }
}

impl PodmanValidatorBackend {
    #[must_use]
    pub fn command(
        &self,
        candidate: &ValidatedCandidateInput,
        check: &ValidationCheck,
    ) -> Vec<String> {
        vec![
            String::from("podman"),
            String::from("run"),
            String::from("--rm"),
            String::from("--network=none"),
            String::from("--read-only"),
            String::from("--userns=keep-id"),
            String::from("--pid=private"),
            String::from("--ipc=private"),
            String::from("--cap-drop=ALL"),
            String::from("--security-opt=no-new-privileges"),
            String::from("--pids-limit=512"),
            String::from("--tmpfs"),
            String::from("/tmp:rw,nosuid,nodev"),
            String::from("--tmpfs"),
            String::from("/home/validator:rw,nosuid,nodev"),
            String::from("--mount"),
            format!(
                "type=bind,src={},dst=/workspace/project,rw",
                candidate.root.display()
            ),
            String::from("--workdir"),
            String::from("/workspace/project"),
            String::from("--entrypoint"),
            self.shell.clone(),
            self.image.clone(),
            String::from("-lc"),
            check.command.clone(),
        ]
    }
}

impl ValidatorBackend for PodmanValidatorBackend {
    fn backend_id(&self) -> &'static str {
        "podman-validator-v1"
    }

    fn validate(
        &self,
        candidate: &ValidatedCandidateInput,
        plan: &ValidationPlan,
    ) -> io::Result<ValidationResult> {
        if !candidate.root.is_absolute() || plan.checks.len() > MAX_CHECKS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid validator input",
            ));
        }
        let started = Instant::now();
        let validation_identity = plan.identity(self.backend_id());
        let mut results = Vec::with_capacity(plan.checks.len());
        for check in &plan.checks {
            results.push(run_check(&self.command(candidate, check), check)?);
        }
        let mut remaining = MAX_TOTAL_OUTPUT_BYTES;
        for result in &mut results {
            truncate_result(&mut result.stdout, &mut result.truncated, &mut remaining);
            truncate_result(&mut result.stderr, &mut result.truncated, &mut remaining);
        }
        Ok(ValidationResult {
            candidate_identity: candidate.candidate_identity,
            validation_identity,
            checks: results,
            duration: started.elapsed(),
        })
    }
}

fn truncate_result(output: &mut String, truncated: &mut bool, remaining: &mut usize) {
    let bytes = output.as_bytes();
    if bytes.len() <= *remaining {
        *remaining -= bytes.len();
        return;
    }
    let mut end = (*remaining).min(bytes.len());
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    output.truncate(end);
    output.push_str("\n[total validator output truncated]\n");
    *remaining = 0;
    *truncated = true;
}

fn run_check(command: &[String], check: &ValidationCheck) -> io::Result<ValidationCheckResult> {
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("validator stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("validator stderr unavailable"))?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let deadline = Instant::now() + check.timeout;
    let timed_out = wait_with_deadline(&mut child, deadline)?;
    if timed_out {
        let _ = child.kill();
        let _ = child.wait();
    }
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| io::Error::other("validator stdout reader panicked"))??;
    let (stderr, stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| io::Error::other("validator stderr reader panicked"))??;
    let truncated = stdout_truncated || stderr_truncated;
    let status = if timed_out {
        ValidationStatus::Timeout
    } else {
        let exit = child
            .try_wait()?
            .ok_or_else(|| io::Error::other("validator process state unavailable"))?;
        if exit.success() {
            ValidationStatus::Pass
        } else {
            ValidationStatus::Fail
        }
    };
    let exit_code = child.try_wait()?.and_then(|status| status.code());
    Ok(ValidationCheckResult {
        name: check.name.clone(),
        required: check.required,
        status,
        exit_code,
        stdout,
        stderr,
        truncated,
    })
}

fn wait_with_deadline(child: &mut Child, deadline: Instant) -> io::Result<bool> {
    loop {
        if child.try_wait()?.is_some() {
            return Ok(false);
        }
        if Instant::now() >= deadline {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_bounded(mut reader: impl Read) -> io::Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if bytes.len() < MAX_OUTPUT_BYTES {
            let remaining = MAX_OUTPUT_BYTES - bytes.len();
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        }
        if bytes.len() >= MAX_OUTPUT_BYTES && count > MAX_OUTPUT_BYTES.saturating_sub(bytes.len()) {
            truncated = true;
        }
    }
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push_str("\n[validator output truncated]\n");
    }
    Ok((output, truncated))
}

#[must_use]
pub fn detect_validation_plan(root: &Path) -> ValidationPlan {
    let mut checks = Vec::new();
    if root.join("Cargo.toml").is_file() {
        checks.push(check("cargo fmt", "cargo fmt --check"));
        checks.push(check("cargo test --workspace", "cargo test --workspace"));
        checks.push(check(
            "cargo clippy",
            "cargo clippy --workspace --all-targets -- -D warnings",
        ));
    }
    if let Ok(package) = fs::read_to_string(root.join("package.json")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&package) {
            for name in ["test", "lint", "typecheck"] {
                if value
                    .get("scripts")
                    .and_then(|scripts| scripts.get(name))
                    .and_then(serde_json::Value::as_str)
                    .is_some()
                {
                    checks.push(check(format!("npm {name}"), format!("npm run {name}")));
                }
            }
        }
    }
    if root.join("pytest.ini").is_file() || root.join("pyproject.toml").is_file() {
        checks.push(check("pytest", "pytest"));
    }
    ValidationPlan::new(checks)
}

fn check(name: impl Into<String>, command: impl Into<String>) -> ValidationCheck {
    ValidationCheck {
        name: name.into(),
        command: command.into(),
        timeout: DEFAULT_TIMEOUT,
        required: true,
    }
}

fn copy_tree(source: &Path, destination: &Path, root: &Path) -> io::Result<()> {
    for item in WalkDir::new(source).follow_links(false) {
        let item = item.map_err(|error| io::Error::other(error.to_string()))?;
        let relative = item.path().strip_prefix(source).map_err(io::Error::other)?;
        if relative.as_os_str().is_empty() || is_git_path(relative) {
            continue;
        }
        copy_entry(item.path(), &destination.join(relative), root)?;
    }
    Ok(())
}

fn copy_entry(source: &Path, destination: &Path, root: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(source)?;
        validate_link_target(source.parent().unwrap_or(root), &target, root)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, destination)?;
        #[cfg(not(unix))]
        return Err(io::Error::other("symlink validation snapshot unsupported"));
    } else if file_type.is_dir() {
        fs::create_dir_all(destination)?;
    } else if file_type.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported special file in validation snapshot",
        ));
    }
    Ok(())
}

fn validate_link_target(link_parent: &Path, target: &Path, root: &Path) -> io::Result<()> {
    if target.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "external symlink in validation snapshot",
        ));
    }
    let relative_parent = link_parent.strip_prefix(root).unwrap_or(Path::new(""));
    let mut depth = 0_i32;
    for component in relative_parent.join(target).components() {
        match component {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            _ => {}
        }
        if depth < 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "external symlink in validation snapshot",
            ));
        }
    }
    Ok(())
}

fn is_git_path(path: &Path) -> bool {
    path.components()
        .next()
        .is_some_and(|component| matches!(component, Component::Normal(value) if value == ".git"))
}

fn unique_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::CandidateChangeSetId;

    #[test]
    fn validation_identity_changes_when_plan_changes() {
        let first = ValidationPlan::new(vec![check("test", "cargo test")]);
        let second = ValidationPlan::new(vec![check("test", "cargo test --workspace")]);
        assert_ne!(
            first.identity("podman-validator-v1"),
            second.identity("podman-validator-v1")
        );
    }

    #[test]
    fn podman_command_is_fresh_networkless_and_credential_free() {
        let backend = PodmanValidatorBackend::default();
        let candidate = ValidatedCandidateInput {
            candidate_identity: CandidateChangeSetId::zero(),
            root: PathBuf::from("/tmp/validation"),
        };
        let command = backend
            .command(&candidate, &check("test", "cargo test"))
            .join(" ");
        for required in [
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
            "no-new-privileges",
            "--pid=private",
            "/workspace/project",
        ] {
            assert!(command.contains(required), "missing {required}");
        }
        for forbidden in [
            "--privileged",
            "--network=host",
            ".ssh",
            ".aws",
            "docker.sock",
            "podman.sock",
            "SSH_AUTH_SOCK",
            "canonical",
        ] {
            assert!(!command.contains(forbidden), "forbidden {forbidden}");
        }
    }

    #[test]
    fn candidate_and_validation_identities_must_match() {
        let plan = ValidationPlan::new(Vec::new());
        let result = ValidationResult {
            candidate_identity: CandidateChangeSetId::zero(),
            validation_identity: plan.identity("podman-validator-v1"),
            checks: Vec::new(),
            duration: Duration::ZERO,
        };
        assert!(result.matches(CandidateChangeSetId::zero(), &plan, "podman-validator-v1"));
        assert!(!result.matches(
            CandidateChangeSetId::new([1; 32]),
            &plan,
            "podman-validator-v1"
        ));
    }

    #[test]
    fn failed_and_timeout_results_cannot_apply() {
        let result = ValidationResult {
            candidate_identity: CandidateChangeSetId::zero(),
            validation_identity: ValidationIdentity([0; 32]),
            checks: vec![ValidationCheckResult {
                name: String::from("test"),
                required: true,
                status: ValidationStatus::Fail,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: String::new(),
                truncated: false,
            }],
            duration: Duration::ZERO,
        };
        assert!(!result.allows_apply(
            CandidateChangeSetId::zero(),
            ValidationPolicy::default(),
            false
        ));
    }

    #[test]
    fn validation_snapshot_excludes_git_and_is_independent() {
        let root = std::env::temp_dir().join(format!("claw-validator-test-{}", unique_stamp()));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("source.txt"), b"candidate").unwrap();
        fs::write(root.join(".git/config"), b"hostile").unwrap();
        let candidate = UntrustedCandidate { root: root.clone() };
        let snapshot =
            ValidationSnapshot::create(&candidate, CandidateChangeSetId::zero()).unwrap();
        assert!(snapshot.root.join("source.txt").is_file());
        assert!(!snapshot.root.join(".git/config").exists());
        fs::write(snapshot.root.join("source.txt"), b"validator artifact").unwrap();
        assert_eq!(fs::read(root.join("source.txt")).unwrap(), b"candidate");
        drop(snapshot);
        let _ = fs::remove_dir_all(root);
    }
}
