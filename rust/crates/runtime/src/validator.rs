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
    pub command: String,
    pub required: bool,
    pub status: ValidationStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationDisposition {
    Pass,
    BaselineFailure,
    CandidateFailure,
    InfrastructureFailure,
}

impl ValidationDisposition {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::BaselineFailure => "baseline_failure",
            Self::CandidateFailure => "candidate_failure",
            Self::InfrastructureFailure => "infrastructure_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationCheckDisposition {
    Pass,
    UnchangedBaselineFailure,
    CandidateFixedBaselineFailure,
    CandidateFailure,
    InfrastructureFailure,
}

impl ValidationCheckDisposition {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::UnchangedBaselineFailure => "unchanged_baseline_failure",
            Self::CandidateFixedBaselineFailure => "candidate_fixed_baseline_failure",
            Self::CandidateFailure => "candidate_failure",
            Self::InfrastructureFailure => "infrastructure_failure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCheckComparison {
    pub name: String,
    pub baseline_status: ValidationStatus,
    pub candidate_status: ValidationStatus,
    pub baseline_fingerprints: Vec<String>,
    pub candidate_fingerprints: Vec<String>,
    pub unchanged_failures: Vec<String>,
    pub new_failures: Vec<String>,
    pub resolved_failures: Vec<String>,
    pub disposition: ValidationCheckDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationComparison {
    pub baseline_identity: CandidateChangeSetId,
    pub candidate_identity: CandidateChangeSetId,
    pub checks: Vec<ValidationCheckComparison>,
    pub disposition: ValidationDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    pub candidate_identity: CandidateChangeSetId,
    pub validation_identity: ValidationIdentity,
    pub checks: Vec<ValidationCheckResult>,
    pub duration: Duration,
    pub comparison: Option<ValidationComparison>,
}

impl ValidationResult {
    /// Infrastructure could not produce trustworthy candidate evidence.
    #[must_use]
    pub fn has_infrastructure_failure(&self) -> bool {
        self.comparison.as_ref().is_some_and(|comparison| {
            comparison.disposition == ValidationDisposition::InfrastructureFailure
        }) || self.checks.is_empty()
            || self.checks.iter().any(|check| {
                matches!(
                    check.status,
                    ValidationStatus::Blocked | ValidationStatus::Error | ValidationStatus::Timeout
                )
            })
    }
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
        if self.candidate_identity != candidate
            || !policy.require_validation
            || self.checks.is_empty()
        {
            return false;
        }
        if self.comparison.as_ref().is_some_and(|comparison| {
            matches!(
                comparison.disposition,
                ValidationDisposition::CandidateFailure
                    | ValidationDisposition::InfrastructureFailure
            )
        }) {
            return false;
        }
        let baseline_aware = self.comparison.as_ref().is_some_and(|comparison| {
            matches!(
                comparison.disposition,
                ValidationDisposition::Pass | ValidationDisposition::BaselineFailure
            )
        });
        for check in &self.checks {
            let terminal_failure = matches!(
                check.status,
                ValidationStatus::Fail | ValidationStatus::Timeout | ValidationStatus::Error
            );
            let incomplete = matches!(
                check.status,
                ValidationStatus::Blocked | ValidationStatus::Skipped
            );
            if terminal_failure
                && !baseline_aware
                && (check.required || policy.optional_fail_blocks)
            {
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
                command: String::from("validator startup"),
                required: true,
                status: ValidationStatus::Blocked,
                exit_code: None,
                stdout: String::new(),
                stderr: reason.into(),
                truncated: false,
            }],
            duration: Duration::ZERO,
            comparison: None,
        }
    }
}

#[must_use]
pub fn compare_validation_results(
    baseline: &ValidationResult,
    candidate: &ValidationResult,
    baseline_identity: CandidateChangeSetId,
) -> ValidationComparison {
    if baseline.checks.len() != candidate.checks.len()
        || baseline
            .checks
            .iter()
            .zip(&candidate.checks)
            .any(|(left, right)| left.name != right.name || left.command != right.command)
    {
        return ValidationComparison {
            baseline_identity,
            candidate_identity: candidate.candidate_identity,
            checks: Vec::new(),
            disposition: ValidationDisposition::InfrastructureFailure,
        };
    }

    let checks = baseline
        .checks
        .iter()
        .zip(&candidate.checks)
        .map(|(baseline, candidate)| compare_check(baseline, candidate))
        .collect::<Vec<_>>();
    let disposition = if checks
        .iter()
        .any(|check| check.disposition == ValidationCheckDisposition::InfrastructureFailure)
    {
        ValidationDisposition::InfrastructureFailure
    } else if checks
        .iter()
        .any(|check| check.disposition == ValidationCheckDisposition::CandidateFailure)
    {
        ValidationDisposition::CandidateFailure
    } else if checks.iter().any(|check| {
        matches!(
            check.disposition,
            ValidationCheckDisposition::UnchangedBaselineFailure
                | ValidationCheckDisposition::CandidateFixedBaselineFailure
        )
    }) {
        ValidationDisposition::BaselineFailure
    } else {
        ValidationDisposition::Pass
    };
    ValidationComparison {
        baseline_identity,
        candidate_identity: candidate.candidate_identity,
        checks,
        disposition,
    }
}

fn compare_check(
    baseline: &ValidationCheckResult,
    candidate: &ValidationCheckResult,
) -> ValidationCheckComparison {
    let baseline_fingerprints = failure_fingerprints(baseline);
    let candidate_fingerprints = failure_fingerprints(candidate);
    let unchanged_failures = candidate_fingerprints
        .iter()
        .filter(|fingerprint| baseline_fingerprints.contains(fingerprint))
        .cloned()
        .collect::<Vec<_>>();
    let new_failures = candidate_fingerprints
        .iter()
        .filter(|fingerprint| !baseline_fingerprints.contains(fingerprint))
        .cloned()
        .collect::<Vec<_>>();
    let resolved_failures = baseline_fingerprints
        .iter()
        .filter(|fingerprint| !candidate_fingerprints.contains(fingerprint))
        .cloned()
        .collect::<Vec<_>>();
    let baseline_infra = is_infrastructure_status(baseline.status);
    let candidate_infra = is_infrastructure_status(candidate.status);
    let candidate_failed = is_failure_status(candidate.status);
    let baseline_failed = is_failure_status(baseline.status);
    let disposition = if baseline_infra || candidate_infra {
        ValidationCheckDisposition::InfrastructureFailure
    } else if candidate_failed && (!baseline_failed || !new_failures.is_empty()) {
        ValidationCheckDisposition::CandidateFailure
    } else if !candidate_failed && baseline_failed {
        ValidationCheckDisposition::CandidateFixedBaselineFailure
    } else if candidate_failed && baseline_failed {
        ValidationCheckDisposition::UnchangedBaselineFailure
    } else {
        ValidationCheckDisposition::Pass
    };
    ValidationCheckComparison {
        name: candidate.name.clone(),
        baseline_status: baseline.status,
        candidate_status: candidate.status,
        baseline_fingerprints,
        candidate_fingerprints,
        unchanged_failures,
        new_failures,
        resolved_failures,
        disposition,
    }
}

fn is_failure_status(status: ValidationStatus) -> bool {
    status == ValidationStatus::Fail
}

fn is_infrastructure_status(status: ValidationStatus) -> bool {
    matches!(
        status,
        ValidationStatus::Blocked
            | ValidationStatus::Skipped
            | ValidationStatus::Timeout
            | ValidationStatus::Error
    )
}

fn failure_fingerprints(check: &ValidationCheckResult) -> Vec<String> {
    if !is_failure_status(check.status) {
        return Vec::new();
    }
    let mut fingerprints = Vec::new();
    for line in check.stdout.lines().chain(check.stderr.lines()) {
        let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
        let lower = normalized.to_ascii_lowercase();
        let high_signal = lower.starts_with("error")
            || lower.starts_with("warning")
            || lower.starts_with("test ")
            || lower.contains(" failed")
            || lower.contains("panicked")
            || lower.contains("assertion")
            || lower.contains("could not compile")
            || lower.contains("test result:")
            || lower.contains("diff in");
        if high_signal && !normalized.is_empty() && !fingerprints.contains(&normalized) {
            fingerprints.push(normalized);
        }
        if fingerprints.len() >= 32 {
            break;
        }
    }
    if fingerprints.is_empty() {
        fingerprints.push(format!(
            "status={:?} exit_code={:?}",
            check.status, check.exit_code
        ));
    }
    fingerprints
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
        initialize_validation_repository(&root)?;
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

    pub fn create_baseline(baseline: &TrustedBaseline) -> io::Result<Self> {
        Self::create(
            &UntrustedCandidate {
                root: baseline.root.clone(),
            },
            baseline.identity(),
        )
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
    fn identity(&self) -> String {
        self.backend_id().to_string()
    }
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
            String::from("--env"),
            String::from("CARGO_TARGET_DIR=/tmp/claw-cargo-target"),
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
            String::from("-c"),
            check.command.clone(),
        ]
    }

    #[must_use]
    pub fn identity(&self) -> String {
        format!("{}:{}", self.backend_id(), self.image)
    }
}

impl ValidatorBackend for PodmanValidatorBackend {
    fn backend_id(&self) -> &'static str {
        "podman-validator-v2"
    }

    fn identity(&self) -> String {
        self.identity()
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
        let validation_identity = plan.identity(&self.identity());
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
            comparison: None,
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
        } else if validator_startup_failure(exit.code(), &stderr) {
            ValidationStatus::Blocked
        } else {
            ValidationStatus::Fail
        }
    };
    let exit_code = child.try_wait()?.and_then(|status| status.code());
    Ok(ValidationCheckResult {
        name: check.name.clone(),
        command: check.command.clone(),
        required: check.required,
        status,
        exit_code,
        stdout,
        stderr,
        truncated,
    })
}

fn validator_startup_failure(exit_code: Option<i32>, stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    exit_code == Some(127)
        || exit_code == Some(125)
        || [
            "command not found",
            "cargo: not found",
            "rustc: not found",
            "unable to find image",
            "no such image",
            "failed to download",
            "failed to get",
            "no matching package named",
            "attempting to make an http request",
            "offline mode",
            "failed to load source",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        || (lower.contains("source directory") && lower.contains("does not exist"))
        || (lower.contains("permission denied")
            && (lower.contains("/usr/local/cargo/") || lower.contains("/usr/local/rustup/")))
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
    let mut tail = Vec::new();
    let mut overflowed = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if !overflowed && bytes.len().saturating_add(count) <= MAX_OUTPUT_BYTES {
            bytes.extend_from_slice(&buffer[..count]);
            continue;
        }
        if !overflowed {
            overflowed = true;
            let split = MAX_OUTPUT_BYTES / 2;
            tail.extend_from_slice(&bytes[split..]);
            bytes.truncate(split);
        }
        tail.extend_from_slice(&buffer[..count]);
        let tail_limit = MAX_OUTPUT_BYTES / 2;
        if tail.len() > tail_limit {
            let remove = tail.len() - tail_limit;
            tail.drain(..remove);
        }
    }
    if !overflowed {
        return Ok((String::from_utf8_lossy(&bytes).into_owned(), false));
    }
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    output.push_str("\n[validator output truncated; tail retained]\n");
    output.push_str(&String::from_utf8_lossy(&tail));
    Ok((output, true))
}

#[must_use]
pub fn detect_validation_plan(root: &Path) -> ValidationPlan {
    let mut checks = Vec::new();
    if let Some(manifest) = find_manifest(root, "Cargo.toml") {
        let manifest_arg = shell_quote(&manifest);
        checks.push(check(
            "cargo fmt",
            format!("cargo fmt --check --all --manifest-path {manifest_arg}"),
        ));
        checks.push(check(
            "cargo test --workspace",
            format!("cargo test --workspace --manifest-path {manifest_arg}"),
        ));
        checks.push(check(
            "cargo clippy",
            format!(
                "cargo clippy --workspace --all-targets --manifest-path {manifest_arg} -- -D warnings"
            ),
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

/// Detect fast, candidate-only feedback checks for the coding loop.
///
/// Trusted validation intentionally remains broad and is built by
/// [`detect_validation_plan`].  Development feedback should instead answer
/// whether the current edit still formats and type-checks without forcing the
/// writer to wait for the full workspace test and lint matrix after every
/// checkpoint.
#[must_use]
pub fn detect_development_validation_plan(root: &Path) -> ValidationPlan {
    detect_development_validation_plan_for_changes(root, &[])
}

/// Detect fast feedback checks scoped to the packages touched by a candidate.
#[must_use]
pub fn detect_development_validation_plan_for_changes(
    root: &Path,
    changed_paths: &[PathBuf],
) -> ValidationPlan {
    let mut checks = Vec::new();
    if let Some(manifest) = find_manifest(root, "Cargo.toml") {
        let manifest_arg = shell_quote(&manifest);
        checks.push(check(
            "cargo fmt",
            format!("cargo fmt --check --all --manifest-path {manifest_arg}"),
        ));
        let packages = changed_paths
            .iter()
            .filter_map(|path| package_name_for_path(root, path))
            .collect::<std::collections::BTreeSet<_>>();
        let package_args = packages
            .iter()
            .map(|package| format!(" -p {}", shell_quote(package)))
            .collect::<String>();
        let scope = if package_args.is_empty() {
            " --workspace".to_string()
        } else {
            package_args
        };
        checks.push(check(
            "cargo check",
            format!("cargo check{scope} --manifest-path {manifest_arg}"),
        ));
    }
    if let Ok(package) = fs::read_to_string(root.join("package.json")) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&package) {
            if value
                .get("scripts")
                .and_then(|scripts| scripts.get("typecheck"))
                .and_then(serde_json::Value::as_str)
                .is_some()
            {
                checks.push(check("npm typecheck", "npm run typecheck"));
            }
        }
    }
    if root.join("pytest.ini").is_file() || root.join("pyproject.toml").is_file() {
        checks.push(check("python compile", "python -m compileall -q ."));
    }
    ValidationPlan::new(checks)
}

fn package_name_for_path(root: &Path, path: &Path) -> Option<String> {
    let mut current = root.join(path);
    if !current.is_dir() {
        current.pop();
    }
    loop {
        let manifest = current.join("Cargo.toml");
        if let Ok(contents) = fs::read_to_string(&manifest) {
            for line in contents.lines() {
                let trimmed = line.trim();
                if let Some(value) = trimmed.strip_prefix("name = \"") {
                    return value.strip_suffix('"').map(ToOwned::to_owned);
                }
            }
        }
        if current == root || !current.pop() {
            return None;
        }
    }
}

fn find_manifest(root: &Path, name: &str) -> Option<String> {
    if root.join(name).is_file() {
        return Some(name.to_string());
    }

    let mut children = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    children.sort();
    children
        .into_iter()
        .find(|path| path.join(name).is_file())
        .map(|path| {
            path.strip_prefix(root)
                .expect("manifest child must be under root")
                .join(name)
                .display()
                .to_string()
        })
}

fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn initialize_validation_repository(root: &Path) -> io::Result<()> {
    run_git(root, &["init"])?;
    run_git(root, &["config", "user.name", "Claw Validator"])?;
    run_git(root, &["config", "user.email", "validator@claw.invalid"])?;
    run_git(root, &["add", "--all"])?;
    run_git(
        root,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "validation snapshot",
        ],
    )
}

fn run_git(root: &Path, args: &[&str]) -> io::Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
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
    fn validation_snapshot_has_isolated_git_context_and_candidate_content() {
        let source = std::env::temp_dir().join(format!("claw-validator-git-{}", unique_stamp()));
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("candidate.txt"), "candidate content").unwrap();
        let candidate = UntrustedCandidate {
            root: source.clone(),
        };

        let snapshot = ValidationSnapshot::create(&candidate, CandidateChangeSetId::zero())
            .expect("validation snapshot should be created");
        let top_level = Command::new("git")
            .args([
                "-C",
                snapshot.root.to_str().unwrap(),
                "rev-parse",
                "--show-toplevel",
            ])
            .output()
            .unwrap();
        assert!(top_level.status.success());
        assert_eq!(
            PathBuf::from(String::from_utf8_lossy(&top_level.stdout).trim())
                .canonicalize()
                .unwrap(),
            snapshot.root.canonicalize().unwrap()
        );
        let head = Command::new("git")
            .args(["-C", snapshot.root.to_str().unwrap(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(head.status.success());
        assert_eq!(
            fs::read_to_string(snapshot.root.join("candidate.txt")).unwrap(),
            "candidate content"
        );
        assert!(!source.join(".git").exists());
        drop(snapshot);
        fs::remove_dir_all(source).unwrap();
    }

    #[test]
    fn missing_validator_tool_is_infrastructure_blocked() {
        assert!(validator_startup_failure(
            Some(127),
            "/bin/sh: cargo: not found"
        ));
        assert!(!validator_startup_failure(Some(1), "test assertion failed"));
    }

    #[test]
    fn unreadable_validator_dependency_is_infrastructure_blocked() {
        assert!(validator_startup_failure(
            Some(1),
            "couldn't read /usr/local/cargo/registry/src/index/fnv/lib.rs: Permission denied"
        ));
        assert!(!validator_startup_failure(
            Some(1),
            "test reported Permission denied"
        ));
    }

    #[test]
    fn unavailable_cargo_dependency_is_infrastructure_blocked() {
        assert!(validator_startup_failure(
            Some(101),
            "error: no matching package named `fnv` found\nlocation searched: crates.io index\nAs a reminder, you're using offline mode (--offline)"
        ));
        assert!(validator_startup_failure(
            Some(101),
            "failed to download `serde` because attempting to make an HTTP request, but --offline was specified"
        ));
        assert!(!validator_startup_failure(
            Some(101),
            "error: test failed; assertion failed: expected state"
        ));
    }

    #[test]
    fn bounded_validator_output_retains_the_diagnostic_tail() {
        let mut input = vec![b'a'; MAX_OUTPUT_BYTES];
        input.extend_from_slice(b"\nfinal compiler diagnostic: expected item\n");

        let (output, truncated) = read_bounded(std::io::Cursor::new(input)).unwrap();

        assert!(truncated);
        assert!(output.contains("validator output truncated; tail retained"));
        assert!(output.contains("final compiler diagnostic: expected item"));
    }

    #[test]
    fn validation_identity_includes_validator_image() {
        let plan = ValidationPlan::new(Vec::new());
        let first = PodmanValidatorBackend {
            image: "validator:a".into(),
            ..Default::default()
        };
        let second = PodmanValidatorBackend {
            image: "validator:b".into(),
            ..Default::default()
        };
        assert_ne!(
            plan.identity(&first.identity()),
            plan.identity(&second.identity())
        );
    }

    #[test]
    fn nested_cargo_workspace_is_validated_with_its_manifest() {
        let root = std::env::temp_dir().join(format!("claw-validator-plan-{}", unique_stamp()));
        fs::create_dir_all(root.join("rust")).unwrap();
        fs::write(root.join("rust/Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();

        let plan = detect_validation_plan(&root);
        assert_eq!(plan.checks.len(), 3);
        assert!(plan
            .checks
            .iter()
            .all(|check| check.command.contains("--manifest-path 'rust/Cargo.toml'")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn development_plan_is_fast_and_keeps_trusted_checks_broad() {
        let root = std::env::temp_dir().join(format!("claw-development-plan-{}", unique_stamp()));
        fs::create_dir_all(root.join("rust")).unwrap();
        fs::write(root.join("rust/Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();

        let plan = detect_development_validation_plan(&root);
        assert_eq!(plan.checks.len(), 2);
        assert!(plan.checks.iter().any(|check| check.name == "cargo fmt"));
        assert!(plan.checks.iter().any(|check| check.name == "cargo check"));
        assert!(plan
            .checks
            .iter()
            .all(|check| !check.command.contains("cargo test")));
        assert!(plan
            .checks
            .iter()
            .all(|check| !check.command.contains("clippy")));

        let trusted = detect_validation_plan(&root);
        assert_eq!(trusted.checks.len(), 3);
        assert!(trusted
            .checks
            .iter()
            .any(|check| check.command.contains("cargo test")));
        assert!(trusted
            .checks
            .iter()
            .any(|check| check.command.contains("clippy")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn development_plan_scopes_cargo_check_to_changed_package() {
        let root = std::env::temp_dir().join(format!("claw-development-scope-{}", unique_stamp()));
        fs::create_dir_all(root.join("rust/crates/demo/src")).unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[workspace]\nmembers = [\"crates/demo\"]\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/crates/demo/Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();

        let plan = detect_development_validation_plan_for_changes(
            &root,
            &[PathBuf::from("rust/crates/demo/src/lib.rs")],
        );
        let check = plan
            .checks
            .iter()
            .find(|check| check.name == "cargo check")
            .expect("development plan should include cargo check");
        assert!(check.command.contains("-p 'demo'"));
        assert!(!check.command.contains("--workspace"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_validation_evidence_is_infrastructure_failure_and_cannot_apply() {
        let plan = ValidationPlan::new(Vec::new());
        let result = ValidationResult {
            candidate_identity: CandidateChangeSetId::zero(),
            validation_identity: plan.identity("podman-validator-v2"),
            checks: Vec::new(),
            duration: Duration::ZERO,
            comparison: None,
        };

        assert!(result.has_infrastructure_failure());
        assert!(!result.allows_apply(
            CandidateChangeSetId::zero(),
            ValidationPolicy::default(),
            false
        ));
    }

    #[test]
    fn candidate_and_validation_identities_must_match() {
        let plan = ValidationPlan::new(Vec::new());
        let result = ValidationResult {
            candidate_identity: CandidateChangeSetId::zero(),
            validation_identity: plan.identity("podman-validator-v1"),
            checks: Vec::new(),
            duration: Duration::ZERO,
            comparison: None,
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
                command: String::from("test"),
                required: true,
                status: ValidationStatus::Fail,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: String::new(),
                truncated: false,
            }],
            duration: Duration::ZERO,
            comparison: None,
        };
        assert!(!result.allows_apply(
            CandidateChangeSetId::zero(),
            ValidationPolicy::default(),
            false
        ));
    }

    fn comparison_check(status: ValidationStatus, output: &str) -> ValidationCheckResult {
        ValidationCheckResult {
            name: String::from("workspace tests"),
            command: String::from("cargo test"),
            required: true,
            status,
            exit_code: Some(i32::from(status != ValidationStatus::Pass)),
            stdout: output.to_string(),
            stderr: String::new(),
            truncated: false,
        }
    }

    #[test]
    fn unchanged_baseline_failure_does_not_block_apply() {
        let baseline = ValidationResult {
            candidate_identity: CandidateChangeSetId::zero(),
            validation_identity: ValidationIdentity([1; 32]),
            checks: vec![comparison_check(
                ValidationStatus::Fail,
                "test repository_cwd_identity_tests::root ... FAILED\ntest result: FAILED",
            )],
            duration: Duration::ZERO,
            comparison: None,
        };
        let candidate = ValidationResult {
            candidate_identity: CandidateChangeSetId::new([2; 32]),
            validation_identity: ValidationIdentity([1; 32]),
            checks: baseline.checks.clone(),
            duration: Duration::ZERO,
            comparison: None,
        };
        let comparison =
            compare_validation_results(&baseline, &candidate, CandidateChangeSetId::new([9; 32]));
        assert_eq!(
            comparison.disposition,
            ValidationDisposition::BaselineFailure
        );
        let candidate = ValidationResult {
            comparison: Some(comparison),
            ..candidate
        };
        assert!(candidate.allows_apply(
            CandidateChangeSetId::new([2; 32]),
            ValidationPolicy::default(),
            false
        ));
    }

    #[test]
    fn new_failure_blocks_and_fixed_baseline_failure_is_reported() {
        let baseline = ValidationResult {
            candidate_identity: CandidateChangeSetId::zero(),
            validation_identity: ValidationIdentity([1; 32]),
            checks: vec![comparison_check(
                ValidationStatus::Fail,
                "test existing_failure ... FAILED",
            )],
            duration: Duration::ZERO,
            comparison: None,
        };
        let introduced = ValidationResult {
            candidate_identity: CandidateChangeSetId::new([2; 32]),
            validation_identity: ValidationIdentity([1; 32]),
            checks: vec![comparison_check(
                ValidationStatus::Fail,
                "test existing_failure ... FAILED\ntest new_failure ... FAILED",
            )],
            duration: Duration::ZERO,
            comparison: None,
        };
        let comparison =
            compare_validation_results(&baseline, &introduced, CandidateChangeSetId::new([9; 32]));
        assert_eq!(
            comparison.disposition,
            ValidationDisposition::CandidateFailure
        );
        assert!(!ValidationResult {
            comparison: Some(comparison),
            ..introduced
        }
        .allows_apply(
            CandidateChangeSetId::new([2; 32]),
            ValidationPolicy::default(),
            false
        ));

        let fixed = ValidationResult {
            candidate_identity: CandidateChangeSetId::new([3; 32]),
            validation_identity: ValidationIdentity([1; 32]),
            checks: vec![comparison_check(ValidationStatus::Pass, "")],
            duration: Duration::ZERO,
            comparison: None,
        };
        let comparison =
            compare_validation_results(&baseline, &fixed, CandidateChangeSetId::new([9; 32]));
        assert_eq!(
            comparison.checks[0].disposition,
            ValidationCheckDisposition::CandidateFixedBaselineFailure
        );
        assert_eq!(
            comparison.disposition,
            ValidationDisposition::BaselineFailure
        );
    }

    #[test]
    fn validation_snapshot_excludes_git_and_is_independent() {
        let root = std::env::temp_dir().join(format!("claw-validator-test-{}", unique_stamp()));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("source.txt"), b"candidate").unwrap();
        fs::write(root.join("Cargo.lock"), b"baseline lockfile").unwrap();
        fs::write(root.join(".git/config"), b"hostile").unwrap();
        let candidate = UntrustedCandidate { root: root.clone() };
        let snapshot =
            ValidationSnapshot::create(&candidate, CandidateChangeSetId::zero()).unwrap();
        assert!(snapshot.root.join("source.txt").is_file());
        assert!(snapshot.root.join(".git/config").is_file());
        let head = Command::new("git")
            .args(["-C", snapshot.root.to_str().unwrap(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(head.status.success());
        fs::write(snapshot.root.join("source.txt"), b"validator artifact").unwrap();
        fs::write(
            snapshot.root.join("Cargo.lock"),
            b"validator lockfile churn",
        )
        .unwrap();
        assert_eq!(fs::read(root.join("source.txt")).unwrap(), b"candidate");
        assert_eq!(
            fs::read(root.join("Cargo.lock")).unwrap(),
            b"baseline lockfile"
        );
        drop(snapshot);
        let _ = fs::remove_dir_all(root);
    }
}
