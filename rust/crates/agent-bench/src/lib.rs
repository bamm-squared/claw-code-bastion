//! Deterministic measurement infrastructure for the current Bastion runtime.
//!
//! The `run_mock` path is deliberately a harness validation path: it drives the
//! real `runtime::ConversationRuntime`, usage accounting, permission policy,
//! session tracing, tool loop, and result serialization with a deterministic
//! scripted provider. It is not presented as a model-quality baseline.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, PermissionMode, PermissionPolicy,
    ProjectContext, RuntimeError, Session, SystemPromptBuilder, ToolError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use telemetry::{MemoryTelemetrySink, SessionTracer, TelemetryEvent};

pub const BENCHMARK_SCHEMA_VERSION: u32 = 1;
const PARTIAL_TELEMETRY_MARKER: &str = "\n__CLAW_BENCH_PARTIAL_TELEMETRY__";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkTask {
    pub id: String,
    pub version: u32,
    pub category: String,
    pub language: String,
    pub prompt: String,
    pub fixture: Fixture,
    #[serde(default)]
    pub visible_validation: Option<String>,
    pub hidden_oracle: HiddenOracle,
    #[serde(default)]
    pub expected_change_scope: Vec<String>,
    #[serde(default)]
    pub forbidden_changes: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_max_turns")]
    pub max_agent_turns: usize,
}

fn default_timeout() -> u64 {
    120
}
fn default_max_turns() -> usize {
    24
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub mock_actions: Vec<MockAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MockAction {
    Write { path: String, content: String },
    Read { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HiddenOracle {
    pub expected_files: BTreeMap<String, String>,
    #[serde(default)]
    pub forbidden_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub alias: String,
    pub provider_profile: String,
    pub model: String,
    #[serde(default = "default_reasoning")]
    pub reasoning: String,
    #[serde(default)]
    pub authorized: bool,
    #[serde(default)]
    pub local: bool,
    #[serde(default)]
    pub actual_input_usd_per_million: f64,
    #[serde(default)]
    pub actual_output_usd_per_million: f64,
    #[serde(default)]
    pub cached_input_usd_per_million: f64,
    #[serde(default)]
    pub cache_write_usd_per_million: f64,
}

fn default_reasoning() -> String {
    "provider-default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    pub schema_version: u32,
    pub models: Vec<ModelProfile>,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            models: vec![ModelProfile {
                alias: "local-mock".to_string(),
                provider_profile: "deterministic-mock".to_string(),
                model: "mock-agent-v1".to_string(),
                reasoning: "not_applicable".to_string(),
                authorized: true,
                local: true,
                actual_input_usd_per_million: 0.0,
                actual_output_usd_per_million: 0.0,
                cached_input_usd_per_million: 0.0,
                cache_write_usd_per_million: 0.0,
            }],
        }
    }
}

impl BenchmarkConfig {
    pub fn from_path(path: Option<&Path>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let config = serde_json::from_str::<Self>(&text).map_err(|error| error.to_string())?;
        if config.schema_version != BENCHMARK_SCHEMA_VERSION {
            return Err(format!(
                "unsupported benchmark config schema {}",
                config.schema_version
            ));
        }
        Ok(config)
    }

    fn profile(&self, alias: &str) -> Result<&ModelProfile, String> {
        self.models
            .iter()
            .find(|profile| profile.alias == alias)
            .ok_or_else(|| format!("model profile {alias:?} is not configured"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextAttribution {
    pub system_instructions_bytes: u64,
    pub conversation_bytes: u64,
    pub tool_result_bytes: u64,
    pub explicit_at_bytes: u64,
    pub retrieval_bytes: u64,
    pub git_context_bytes: u64,
    pub attachment_bytes: u64,
    pub other_bytes: u64,
    pub exact_provider_tokens: Option<u32>,
    pub estimated_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityMetrics {
    pub provider_calls: u64,
    pub model_turns: u64,
    pub tool_bearing_turns: u64,
    pub final_response_turns: u64,
    pub compaction_events: u64,
    pub provider_retries: u64,
    pub tool_calls: BTreeMap<String, u64>,
    pub unique_files_read: u64,
    pub total_file_reads: u64,
    pub repeated_file_reads: u64,
    pub context_search_calls: u64,
    pub tool_calls_before_first_candidate_write: u64,
    pub time_to_first_candidate_mutation_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CandidateMetrics {
    pub files_added: u64,
    pub files_modified: u64,
    pub files_deleted: u64,
    pub bytes_changed: u64,
    pub mutation_operations: u64,
    pub forbidden_changes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TimingMetrics {
    pub end_to_end_ms: u128,
    pub provider_ms: u128,
    pub tool_execution_ms: u128,
    pub context_construction_ms: u128,
    pub validation_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageMetrics {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub actual_input_cost_usd: f64,
    pub actual_output_cost_usd: f64,
    pub actual_cache_cost_usd: f64,
    pub actual_total_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRecord {
    pub benchmark_schema_version: u32,
    pub task_corpus_version: u32,
    pub task_id: String,
    pub category: String,
    pub language: String,
    pub model_alias: String,
    #[serde(default)]
    pub requested_profile: Option<String>,
    #[serde(default)]
    pub executed_profile: Option<String>,
    pub provider: String,
    pub model: String,
    pub reasoning: String,
    pub repetition: u32,
    pub started_at_ms: u128,
    pub finished_at_ms: u128,
    pub first_pass: String,
    pub final_correctness: String,
    pub rework_cycles: u32,
    pub usage: UsageMetrics,
    pub timing: TimingMetrics,
    pub activity: ActivityMetrics,
    pub context: ContextAttribution,
    pub candidate: CandidateMetrics,
    pub validation: String,
    pub validation_duration_ms: u128,
    pub hidden_oracle_checked_outside_workspace: bool,
    pub mock_execution: bool,
    #[serde(default)]
    pub execution: String,
    #[serde(default)]
    pub runtime_image: Option<String>,
    #[serde(default)]
    pub validator_image: Option<String>,
    #[serde(default)]
    pub isolated_execution: Option<bool>,
    #[serde(default)]
    pub error_class: Option<String>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub production_telemetry: Option<Value>,
}

struct ScriptedApi {
    actions: Vec<MockAction>,
    calls: u64,
}

impl ApiClient for ScriptedApi {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let index = usize::try_from(self.calls).unwrap_or(usize::MAX);
        self.calls += 1;
        if let Some(action) = self.actions.get(index) {
            let (name, input) = match action {
                MockAction::Write { path, content } => (
                    "write_file",
                    json!({"path": path, "content": content}).to_string(),
                ),
                MockAction::Read { path } => ("read_file", json!({"path": path}).to_string()),
            };
            return Ok(vec![
                AssistantEvent::ToolUse {
                    id: format!("bench-tool-{}", self.calls),
                    name: name.to_string(),
                    input,
                },
                AssistantEvent::Usage(runtime::TokenUsage {
                    input_tokens: 100,
                    output_tokens: 20,
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                }),
                AssistantEvent::MessageStop,
            ]);
        }
        Ok(vec![
            AssistantEvent::TextDelta("deterministic mock completion".to_string()),
            AssistantEvent::Usage(runtime::TokenUsage {
                input_tokens: 100,
                output_tokens: 20,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            }),
            AssistantEvent::MessageStop,
        ])
    }
}

#[derive(Default, Clone)]
struct ExecutorMetrics {
    read_files: BTreeSet<String>,
    total_file_reads: u64,
    tool_calls: BTreeMap<String, u64>,
    tool_result_bytes: u64,
    first_write: Option<Instant>,
    candidate_writes: u64,
}

struct MeasuredExecutor {
    root: PathBuf,
    metrics: Arc<std::sync::Mutex<ExecutorMetrics>>,
    started: Instant,
}

impl MeasuredExecutor {
    fn new(root: PathBuf, started: Instant) -> Self {
        Self {
            root,
            metrics: Arc::new(std::sync::Mutex::new(ExecutorMetrics::default())),
            started,
        }
    }

    fn safe_path(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let path = Path::new(raw);
        if path.is_absolute()
            || path
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(ToolError::new("benchmark path escapes workspace"));
        }
        Ok(self.root.join(path))
    }

    fn metrics(&self) -> Arc<std::sync::Mutex<ExecutorMetrics>> {
        self.metrics.clone()
    }
}

impl runtime::ToolExecutor for MeasuredExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        let mut metrics = self
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *metrics.tool_calls.entry(tool_name.to_string()).or_default() += 1;
        let value: Value =
            serde_json::from_str(input).map_err(|error| ToolError::new(error.to_string()))?;
        let raw_path = value
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::new("missing path"))?;
        let path = self.safe_path(raw_path)?;
        let result = match tool_name {
            "read_file" => {
                metrics.total_file_reads += 1;
                metrics.read_files.insert(raw_path.to_string());
                fs::read_to_string(&path).map_err(|error| ToolError::new(error.to_string()))?
            }
            "write_file" => {
                let content = value
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::new("missing content"))?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| ToolError::new(error.to_string()))?;
                }
                fs::write(&path, content).map_err(|error| ToolError::new(error.to_string()))?;
                metrics.candidate_writes += 1;
                metrics.first_write.get_or_insert(self.started);
                format!("wrote {} bytes", content.len())
            }
            _ => {
                return Err(ToolError::new(format!(
                    "unsupported benchmark tool {tool_name}"
                )))
            }
        };
        metrics.tool_result_bytes += result.len() as u64;
        Ok(result)
    }
}

pub fn load_tasks(path: &Path) -> Result<Vec<BenchmarkTask>, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let tasks =
        serde_json::from_str::<Vec<BenchmarkTask>>(&text).map_err(|error| error.to_string())?;
    if tasks.is_empty() {
        return Err("task corpus is empty".to_string());
    }
    let mut ids = BTreeSet::new();
    for task in &tasks {
        if !ids.insert(task.id.as_str()) {
            return Err(format!("duplicate task id {:?}", task.id));
        }
    }
    Ok(tasks)
}

pub fn select_tasks(
    tasks: &[BenchmarkTask],
    task_id: Option<&str>,
) -> Result<Vec<BenchmarkTask>, String> {
    match task_id {
        None => Ok(tasks.to_vec()),
        Some(task_id) => tasks
            .iter()
            .find(|task| task.id == task_id)
            .cloned()
            .map(|task| vec![task])
            .ok_or_else(|| format!("task {task_id:?} is not present in the corpus")),
    }
}

pub fn run_mock(
    tasks: &[BenchmarkTask],
    config: &BenchmarkConfig,
    model_alias: &str,
    repetitions: u32,
) -> Result<Vec<BenchmarkRecord>, String> {
    let profile = config.profile(model_alias)?.clone();
    if !profile.authorized || !profile.local {
        return Err(
            "mock execution requires an explicitly authorized local model profile".to_string(),
        );
    }
    let mut records = Vec::new();
    for repetition in 1..=repetitions {
        for task in tasks {
            records.push(run_one(task, &profile, repetition)?);
        }
    }
    Ok(records)
}

/// Run the actual packaged/current `claw` CLI through its normal isolated
/// execution path. This adapter is intentionally opt-in and refuses the
/// deterministic mock profile so mock records cannot be mistaken for a
/// production baseline.
#[allow(clippy::too_many_arguments)]
pub fn run_production(
    tasks: &[BenchmarkTask],
    config: &BenchmarkConfig,
    model_alias: Option<&str>,
    repetitions: u32,
    binary: Option<&Path>,
    runtime_image: Option<&str>,
    settings_path: Option<&Path>,
    validator_image: Option<&str>,
    task_timeout: Option<u64>,
    exploration_timeout: Option<u64>,
    dry_run: bool,
    interactive: bool,
) -> Result<Vec<BenchmarkRecord>, String> {
    let profile = select_production_profile(config, model_alias)?.clone();
    let binary = binary
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CLAW_BENCH_BINARY").map(PathBuf::from))
        .ok_or_else(|| {
            "production execution requires --binary PATH or CLAW_BENCH_BINARY".to_string()
        })?
        .canonicalize()
        .map_err(|error| format!("production benchmark binary is unavailable: {error}"))?;
    let runtime_image = runtime_image
        .map(str::to_string)
        .or_else(|| std::env::var("CLAW_WORKER_IMAGE").ok())
        .ok_or_else(|| {
            "production execution requires --runtime-image REF or CLAW_WORKER_IMAGE".to_string()
        })?;
    let validator_image = validator_image
        .map(str::to_string)
        .or_else(|| std::env::var("CLAW_VALIDATOR_IMAGE").ok())
        .ok_or_else(|| {
            "production execution requires --validator-image REF or CLAW_VALIDATOR_IMAGE; refusing to use the worker image for validation".to_string()
        })?;
    let settings_path = settings_path
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CLAW_BENCH_SETTINGS").map(PathBuf::from))
        .ok_or_else(|| {
            "production execution requires --settings PATH or CLAW_BENCH_SETTINGS".to_string()
        })?;
    let settings = load_child_settings(&settings_path)?;
    if !profile.authorized {
        return Err(format!(
            "model profile {:?} is not benchmark-authorized; set authorized: true only after explicit operator approval",
            profile.alias
        ));
    }
    eprintln!(
        "production preflight: tasks={} task_ids={} profile_mode={} worker_image={} validator_image={} settings_model_resources={} task_timeout={} exploration_budget={}",
        tasks.len(),
        tasks.iter().map(|task| task.id.as_str()).collect::<Vec<_>>().join(","),
        model_alias.unwrap_or("routed"),
        runtime_image,
        validator_image,
        settings
            .get("modelResources")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        task_timeout.map_or_else(|| "task-default".to_string(), |value| format!("{value}s")),
        exploration_timeout.map_or_else(|| "task-derived".to_string(), |value| format!("{value}s")),
    );
    if dry_run {
        eprintln!("production preflight: dry-run; no child process launched");
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for repetition in 1..=repetitions {
        for task in tasks {
            match run_production_one(
                task,
                &profile,
                model_alias,
                repetition,
                &binary,
                &runtime_image,
                &validator_image,
                &settings,
                task_timeout,
                exploration_timeout,
                interactive,
            ) {
                Ok(record) => records.push(record),
                Err(error) => records.push(production_failure_record(
                    task,
                    &profile,
                    model_alias,
                    repetition,
                    &runtime_image,
                    &validator_image,
                    &error,
                )),
            }
        }
    }
    Ok(records)
}

fn load_child_settings(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read Claw settings {}: {error}", path.display()))?;
    let settings = serde_json::from_str::<Value>(&text)
        .map_err(|error| format!("failed to parse Claw settings {}: {error}", path.display()))?;
    if !settings.is_object() {
        return Err("Claw settings must be a JSON object".to_string());
    }
    let resources = settings
        .get("modelResources")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "Claw settings must contain modelResources for production acceptance".to_string()
        })?;
    if resources.is_empty() {
        return Err("Claw settings modelResources must not be empty".to_string());
    }
    Ok(settings)
}

fn select_production_profile<'a>(
    config: &'a BenchmarkConfig,
    model_alias: Option<&str>,
) -> Result<&'a ModelProfile, String> {
    if let Some(alias) = model_alias.filter(|alias| *alias != "local-mock") {
        return config.profile(alias);
    }
    let candidates = config
        .models
        .iter()
        .filter(|profile| profile.authorized && profile.provider_profile != "deterministic-mock")
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [profile] => Ok(profile),
        [] => Err(
            "no explicitly authorized real/local model profile is configured; add one to the models file with authorized: true, then rerun with --execution production --model ALIAS".to_string(),
        ),
        _ => candidates
            .iter()
            .find(|profile| profile.local)
            .copied()
            .or_else(|| candidates.first().copied())
            .ok_or_else(|| "unable to select an authorized production profile".to_string()),
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_production_one(
    task: &BenchmarkTask,
    profile: &ModelProfile,
    model_alias: Option<&str>,
    repetition: u32,
    binary: &Path,
    runtime_image: &str,
    validator_image: &str,
    settings: &Value,
    task_timeout: Option<u64>,
    exploration_timeout: Option<u64>,
    interactive: bool,
) -> Result<BenchmarkRecord, String> {
    let started_at = epoch_ms();
    let wall = Instant::now();
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let root = temp.path().join("project");
    let home = temp.path().join("home");
    let config = temp.path().join("config");
    let cache = temp.path().join("cache");
    let state = temp.path().join("state");
    let telemetry = std::env::var_os("CLAW_BENCH_TELEMETRY").map_or_else(
        || temp.path().join("production-telemetry.json"),
        PathBuf::from,
    );
    let podman_data_home = std::env::var_os("XDG_DATA_HOME").unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map_or_else(|| PathBuf::from("/home"), PathBuf::from)
            .join(".local")
            .join("share")
            .into_os_string()
    });
    for directory in [&root, &home, &config, &cache, &state] {
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    }
    if let Some(parent) = telemetry.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    for (path, content) in &task.fixture.files {
        let destination = root.join(path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(destination, content).map_err(|error| error.to_string())?;
    }
    let git_status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&root)
        .status()
        .map_err(|error| format!("benchmark git fixture failed to start: {error}"))?;
    if !git_status.success() {
        return Err("benchmark git fixture initialization failed".to_string());
    }
    fs::write(
        config.join("settings.json"),
        serde_json::to_vec(settings).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let mut cli_args = vec!["--permission-mode", "workspace-write"];
    if !interactive {
        cli_args.push("--print");
    }
    cli_args.extend(["-p", &task.prompt]);
    if model_alias.is_some_and(|alias| alias != "local-mock") {
        cli_args.splice(0..0, ["--model", &profile.model]);
    }
    let interactive_transcript = interactive.then(|| {
        PathBuf::from(format!(
            "/tmp/claw-pty-{}-{}.log",
            std::process::id(),
            task.id
        ))
    });
    let mut command = if interactive {
        Command::new("script")
    } else {
        #[cfg(unix)]
        {
            // `setsid` makes the CLI the process-group leader; descendants such
            // as Podman workers can then be terminated as one acceptance job.
            let mut command = Command::new("setsid");
            command.arg(binary);
            command
        }
        #[cfg(not(unix))]
        Command::new(binary)
    };
    if interactive {
        let command_line = std::iter::once(binary.to_string_lossy().into_owned())
            .chain(cli_args.iter().map(|arg| (*arg).to_string()))
            .map(|arg| shell_quote(&arg))
            .collect::<Vec<_>>()
            .join(" ");
        let transcript = interactive_transcript
            .as_ref()
            .expect("interactive transcript path exists")
            .to_string_lossy();
        command.args(["-qefc", &command_line, &transcript]);
    }
    let effective_task_timeout = task_timeout.unwrap_or(task.timeout_seconds);
    let effective_exploration_timeout =
        exploration_timeout.unwrap_or_else(|| (effective_task_timeout / 3).max(1));
    command.current_dir(&root);
    if !interactive {
        command.args(cli_args);
    }
    let mut child = command
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_CACHE_HOME", &cache)
        .env("XDG_STATE_HOME", &state)
        .env("CLAW_CONFIG_HOME", &config)
        .env("CLAW_BENCH_TELEMETRY", &telemetry)
        // Headless Claw treats a local OpenAI-compatible provider as
        // configured only when its explicit Ollama endpoint is present.
        .env("OLLAMA_HOST", "http://127.0.0.1:11434")
        // Rootless Podman stores images independently of Bastion's HOME.
        // Preserve that storage location so the explicitly supplied runtime
        // image resolves locally instead of triggering a registry pull.
        .env("XDG_DATA_HOME", podman_data_home)
        .env("CLAW_WORKER_IMAGE", runtime_image)
        .env("CLAW_VALIDATOR_IMAGE", validator_image)
        .env("CLAW_EXECUTION_MODE", "isolated")
        .env(
            "CLAW_EXPLORATION_BUDGET_MS",
            effective_exploration_timeout
                .saturating_mul(1_000)
                .to_string(),
        )
        .stdin(if interactive {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("production CLI failed to start: {error}"))?;
    let mut interactive_input = child.stdin.take();
    let mut apply_sent = false;
    let permission_response = std::env::var("CLAW_BENCH_PERMISSION_RESPONSE")
        .ok()
        .and_then(
            |response| match response.trim().to_ascii_lowercase().as_str() {
                "allow" | "yes" | "y" => Some("y\n"),
                "deny" | "no" | "n" => Some("n\n"),
                _ => None,
            },
        );
    let mut permission_response_sent = false;
    let deadline = Instant::now() + Duration::from_secs(effective_task_timeout);
    let output = loop {
        if interactive
            && !permission_response_sent
            && permission_response.is_some()
            && telemetry_has_event(&telemetry, "permission_requested")
        {
            if let (Some(input), Some(response)) = (interactive_input.as_mut(), permission_response)
            {
                input
                    .write_all(response.as_bytes())
                    .and_then(|()| input.flush())
                    .map_err(|error| format!("failed to send permission decision: {error}"))?;
                permission_response_sent = true;
            }
        }
        if interactive && !apply_sent && telemetry_has_event(&telemetry, "review_ready") {
            if let Some(input) = interactive_input.as_mut() {
                input
                    .write_all(b"a\n")
                    .and_then(|()| input.flush())
                    .map_err(|error| format!("failed to send Review Apply decision: {error}"))?;
                apply_sent = true;
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("production CLI wait failed: {error}"))?
        {
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| "production CLI stdout unavailable".to_string())?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| "production CLI stderr unavailable".to_string())?;
            let mut output = Vec::new();
            stdout
                .take(8 * 1024 * 1024)
                .read_to_end(&mut output)
                .map_err(|error| error.to_string())?;
            let mut error_output = Vec::new();
            stderr
                .take(256 * 1024)
                .read_to_end(&mut error_output)
                .map_err(|error| error.to_string())?;
            break std::process::Output {
                status,
                stdout: output,
                stderr: error_output,
            };
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-KILL", &format!("-{}", child.id())])
                    .status();
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(with_timeout_telemetry(
                &format!(
                    "production task {} timed out after {} seconds",
                    task.id, effective_task_timeout
                ),
                &telemetry,
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if !output.status.success() {
        finalize_telemetry_status(&telemetry, "failed");
        return Err(with_partial_telemetry(
            &format!(
                "production task {} exited with {}; stderr: {}",
                task.id,
                output.status,
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(4_000)
                    .collect::<String>()
            ),
            &telemetry,
        ));
    }
    let oracle_ok = evaluate_oracle(&root, task)?;
    let production_telemetry = fs::read_to_string(&telemetry)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let executed_profile = production_telemetry
        .as_ref()
        .and_then(first_telemetry_profile);
    let candidate = candidate_metrics(&root, task);
    Ok(BenchmarkRecord {
        benchmark_schema_version: BENCHMARK_SCHEMA_VERSION,
        task_corpus_version: task.version,
        task_id: task.id.clone(),
        category: task.category.clone(),
        language: task.language.clone(),
        model_alias: executed_profile
            .clone()
            .or_else(|| model_alias.map(str::to_string))
            .unwrap_or_else(|| "routed".to_string()),
        requested_profile: model_alias.map(str::to_string),
        executed_profile,
        provider: model_alias.map_or_else(
            || "routed".to_string(),
            |_| profile.provider_profile.clone(),
        ),
        model: model_alias.map_or_else(|| "routed".to_string(), |_| profile.model.clone()),
        reasoning: model_alias.map_or_else(|| "routed".to_string(), |_| profile.reasoning.clone()),
        repetition,
        started_at_ms: started_at,
        finished_at_ms: epoch_ms(),
        first_pass: if oracle_ok {
            "FIRST_PASS_PASS"
        } else {
            "FIRST_PASS_FAIL"
        }
        .to_string(),
        final_correctness: if oracle_ok { "PASS" } else { "FAIL" }.to_string(),
        rework_cycles: 0,
        usage: zero_usage(profile),
        timing: TimingMetrics {
            end_to_end_ms: wall.elapsed().as_millis(),
            ..TimingMetrics::default()
        },
        activity: ActivityMetrics::default(),
        context: ContextAttribution::default(),
        candidate,
        validation: "NOT_REPORTED".to_string(),
        validation_duration_ms: 0,
        hidden_oracle_checked_outside_workspace: true,
        mock_execution: false,
        execution: "production_cli".to_string(),
        runtime_image: Some(runtime_image.to_string()),
        validator_image: Some(validator_image.to_string()),
        isolated_execution: Some(true),
        error_class: None,
        error_message: None,
        production_telemetry,
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn telemetry_has_event(path: &Path, event: &str) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("lifecycle_events").cloned())
        .and_then(|events| events.as_array().cloned())
        .is_some_and(|events| events.iter().any(|value| value.as_str() == Some(event)))
}

fn production_failure_record(
    task: &BenchmarkTask,
    profile: &ModelProfile,
    requested_profile: Option<&str>,
    repetition: u32,
    runtime_image: &str,
    validator_image: &str,
    error: &str,
) -> BenchmarkRecord {
    let (error_message, production_telemetry) = split_partial_telemetry(error);
    let class = if error.contains("timed out") {
        "timeout"
    } else {
        "agent_or_provider_failure"
    };
    BenchmarkRecord {
        benchmark_schema_version: BENCHMARK_SCHEMA_VERSION,
        task_corpus_version: task.version,
        task_id: task.id.clone(),
        category: task.category.clone(),
        language: task.language.clone(),
        model_alias: requested_profile.unwrap_or("routed").to_string(),
        requested_profile: requested_profile.map(str::to_string),
        executed_profile: None,
        provider: requested_profile.map_or_else(
            || "unselected".to_string(),
            |_| profile.provider_profile.clone(),
        ),
        model: requested_profile
            .map_or_else(|| "unselected".to_string(), |_| profile.model.clone()),
        reasoning: requested_profile
            .map_or_else(|| "unselected".to_string(), |_| profile.reasoning.clone()),
        repetition,
        started_at_ms: epoch_ms(),
        finished_at_ms: epoch_ms(),
        first_pass: "FIRST_PASS_FAIL".to_string(),
        final_correctness: "FAIL".to_string(),
        rework_cycles: 0,
        usage: zero_usage(profile),
        timing: TimingMetrics::default(),
        activity: ActivityMetrics::default(),
        context: ContextAttribution::default(),
        candidate: CandidateMetrics::default(),
        validation: "NOT_RUN".to_string(),
        validation_duration_ms: 0,
        hidden_oracle_checked_outside_workspace: true,
        mock_execution: false,
        execution: "production_cli".to_string(),
        runtime_image: Some(runtime_image.to_string()),
        validator_image: Some(validator_image.to_string()),
        isolated_execution: Some(true),
        error_class: Some(class.to_string()),
        error_message: Some(error_message),
        production_telemetry,
    }
}

fn with_partial_telemetry(message: &str, path: &Path) -> String {
    let Some(mut value) = read_telemetry(path) else {
        return message.to_string();
    };
    finalize_value_status(&mut value, "failed");
    let _ = fs::write(path, serde_json::to_vec(&value).unwrap_or_default());
    format!("{message}{PARTIAL_TELEMETRY_MARKER}{value}")
}

fn with_timeout_telemetry(message: &str, path: &Path) -> String {
    let Some(mut value) = read_telemetry(path) else {
        return with_partial_telemetry(message, path);
    };
    finalize_value_status(&mut value, "timeout");
    let _ = fs::write(path, serde_json::to_vec(&value).unwrap_or_default());
    format!("{message}{PARTIAL_TELEMETRY_MARKER}{value}")
}

fn read_telemetry(path: &Path) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
}

fn finalize_value_status(value: &mut Value, status: &str) {
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "terminal_status".to_string(),
            Value::String(status.to_string()),
        );
    }
}

fn finalize_telemetry_status(path: &Path, status: &str) {
    let Some(mut value) = read_telemetry(path) else {
        return;
    };
    finalize_value_status(&mut value, status);
    let _ = fs::write(path, serde_json::to_vec(&value).unwrap_or_default());
}

fn first_telemetry_profile(telemetry: &Value) -> Option<String> {
    telemetry
        .get("provider_call_records")
        .and_then(Value::as_array)
        .and_then(|records| {
            records.iter().find_map(|record| {
                record
                    .get("profile")
                    .and_then(Value::as_str)
                    .filter(|profile| !profile.is_empty())
                    .map(str::to_string)
            })
        })
}

fn split_partial_telemetry(error: &str) -> (String, Option<Value>) {
    let Some((message, telemetry)) = error.split_once(PARTIAL_TELEMETRY_MARKER) else {
        return (error.to_string(), None);
    };
    (
        message.to_string(),
        serde_json::from_str::<Value>(telemetry).ok(),
    )
}

fn zero_usage(profile: &ModelProfile) -> UsageMetrics {
    UsageMetrics {
        input_tokens: 0,
        output_tokens: 0,
        cached_input_tokens: 0,
        cache_write_tokens: 0,
        actual_input_cost_usd: 0.0 * profile.actual_input_usd_per_million,
        actual_output_cost_usd: 0.0 * profile.actual_output_usd_per_million,
        actual_cache_cost_usd: 0.0,
        actual_total_cost_usd: 0.0,
    }
}

#[allow(clippy::too_many_lines)]
fn run_one(
    task: &BenchmarkTask,
    profile: &ModelProfile,
    repetition: u32,
) -> Result<BenchmarkRecord, String> {
    let started_at = epoch_ms();
    let wall = Instant::now();
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let root = temp.path().join("project");
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    for (path, content) in &task.fixture.files {
        let destination = root.join(path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(destination, content).map_err(|error| error.to_string())?;
    }
    let started = Instant::now();
    let executor = MeasuredExecutor::new(root.clone(), started);
    let metrics_handle = executor.metrics();
    let api = ScriptedApi {
        actions: task.fixture.mock_actions.clone(),
        calls: 0,
    };
    let sink = Arc::new(MemoryTelemetrySink::default());
    let tracer = SessionTracer::new(format!("bench-{}-{}", task.id, repetition), sink.clone());
    let system_prompt = SystemPromptBuilder::new()
        .with_project_context(ProjectContext {
            cwd: root.clone(),
            current_date: "benchmark".to_string(),
            git_status: None,
            git_diff: None,
            git_context: None,
            instruction_files: Vec::new(),
        })
        .build();
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        api,
        executor,
        PermissionPolicy::new(PermissionMode::WorkspaceWrite),
        system_prompt.clone(),
    )
    .with_max_iterations(task.max_agent_turns)
    .with_session_tracer(tracer);
    let context_start = Instant::now();
    let _summary = runtime
        .run_turn(&task.prompt, None)
        .map_err(|error| error.to_string())?;
    let context_construction_ms = context_start.elapsed().as_millis();
    let oracle_ok = evaluate_oracle(&root, task)?;
    let usage = runtime.usage().cumulative_usage();
    let calls = runtime.api_client_mut().calls;
    let events = sink.events();
    let metrics = metrics_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let activity = activity_from_events(&events, &metrics, calls, started);
    let timing = TimingMetrics {
        end_to_end_ms: wall.elapsed().as_millis(),
        provider_ms: 0,
        tool_execution_ms: 0,
        context_construction_ms,
        validation_ms: 0,
    };
    let usage_metrics = UsageMetrics {
        input_tokens: u64::from(usage.input_tokens),
        output_tokens: u64::from(usage.output_tokens),
        cached_input_tokens: u64::from(usage.cache_read_input_tokens),
        cache_write_tokens: u64::from(usage.cache_creation_input_tokens),
        actual_input_cost_usd: cost(usage.input_tokens, profile.actual_input_usd_per_million),
        actual_output_cost_usd: cost(usage.output_tokens, profile.actual_output_usd_per_million),
        actual_cache_cost_usd: cost(
            usage.cache_read_input_tokens,
            profile.cached_input_usd_per_million,
        ) + cost(
            usage.cache_creation_input_tokens,
            profile.cache_write_usd_per_million,
        ),
        actual_total_cost_usd: cost(usage.input_tokens, profile.actual_input_usd_per_million)
            + cost(usage.output_tokens, profile.actual_output_usd_per_million)
            + cost(
                usage.cache_read_input_tokens,
                profile.cached_input_usd_per_million,
            )
            + cost(
                usage.cache_creation_input_tokens,
                profile.cache_write_usd_per_million,
            ),
    };
    let candidate = candidate_metrics(&root, task);
    Ok(BenchmarkRecord {
        benchmark_schema_version: BENCHMARK_SCHEMA_VERSION,
        task_corpus_version: task.version,
        task_id: task.id.clone(),
        category: task.category.clone(),
        language: task.language.clone(),
        model_alias: profile.alias.clone(),
        requested_profile: Some(profile.alias.clone()),
        executed_profile: Some(profile.alias.clone()),
        provider: profile.provider_profile.clone(),
        model: profile.model.clone(),
        reasoning: profile.reasoning.clone(),
        repetition,
        started_at_ms: started_at,
        finished_at_ms: epoch_ms(),
        first_pass: if oracle_ok {
            "FIRST_PASS_PASS"
        } else {
            "FIRST_PASS_FAIL"
        }
        .to_string(),
        final_correctness: if oracle_ok { "PASS" } else { "FAIL" }.to_string(),
        rework_cycles: 0,
        usage: usage_metrics,
        timing,
        activity,
        context: ContextAttribution {
            system_instructions_bytes: system_prompt.iter().map(String::len).sum::<usize>() as u64,
            conversation_bytes: task.prompt.len() as u64,
            tool_result_bytes: metrics.tool_result_bytes,
            ..ContextAttribution::default()
        },
        candidate,
        validation: "NOT_RUN".to_string(),
        validation_duration_ms: 0,
        hidden_oracle_checked_outside_workspace: true,
        mock_execution: true,
        execution: "mock_runtime".to_string(),
        runtime_image: None,
        validator_image: None,
        isolated_execution: Some(false),
        error_class: None,
        error_message: None,
        production_telemetry: None,
    })
}

fn cost(tokens: u32, per_million: f64) -> f64 {
    f64::from(tokens) / 1_000_000.0 * per_million
}

fn evaluate_oracle(root: &Path, task: &BenchmarkTask) -> Result<bool, String> {
    let outside = root
        .parent()
        .ok_or_else(|| "project has no parent".to_string())?;
    if task
        .hidden_oracle
        .expected_files
        .keys()
        .any(|path| outside.join(path).exists())
    {
        return Err("hidden oracle unexpectedly visible outside project boundary".to_string());
    }
    for forbidden in &task.hidden_oracle.forbidden_paths {
        if root.join(forbidden).exists() {
            return Ok(false);
        }
    }
    let mut actual = BTreeMap::new();
    collect_files(root, root, &mut actual)?;
    Ok(actual == task.hidden_oracle.expected_files)
}

fn collect_files(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for entry in fs::read_dir(current).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if matches!(entry.file_name().to_str(), Some(".git" | ".claw")) {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else {
            let key = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(
                key,
                fs::read_to_string(path).map_err(|error| error.to_string())?,
            );
        }
    }
    Ok(())
}

fn candidate_metrics(root: &Path, task: &BenchmarkTask) -> CandidateMetrics {
    let mut actual = BTreeMap::new();
    let _ = collect_files(root, root, &mut actual);
    let keys = task
        .fixture
        .files
        .keys()
        .chain(actual.keys())
        .collect::<BTreeSet<_>>();
    let mut metrics = CandidateMetrics::default();
    for key in keys {
        match (task.fixture.files.get(key), actual.get(key)) {
            (None, Some(value)) => {
                metrics.files_added += 1;
                metrics.bytes_changed += value.len() as u64;
            }
            (Some(old), None) => {
                metrics.files_deleted += 1;
                metrics.bytes_changed += old.len() as u64;
            }
            (Some(old), Some(new)) if old != new => {
                metrics.files_modified += 1;
                metrics.bytes_changed += old.len().abs_diff(new.len()) as u64;
            }
            _ => {}
        }
    }
    metrics
}

fn activity_from_events(
    events: &[TelemetryEvent],
    metrics: &ExecutorMetrics,
    calls: u64,
    started: Instant,
) -> ActivityMetrics {
    let mut activity = ActivityMetrics {
        provider_calls: calls,
        tool_calls: metrics.tool_calls.clone(),
        ..ActivityMetrics::default()
    };
    for event in events {
        if let TelemetryEvent::SessionTrace(trace) = event {
            match trace.name.as_str() {
                "assistant_iteration_completed" => {
                    activity.model_turns += 1;
                    if trace
                        .attributes
                        .get("pending_tool_use_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        > 0
                    {
                        activity.tool_bearing_turns += 1;
                    }
                }
                "turn_completed" => activity.final_response_turns += 1,
                _ => {}
            }
        }
    }
    activity.unique_files_read = metrics.read_files.len() as u64;
    activity.total_file_reads = metrics.total_file_reads;
    activity.repeated_file_reads = metrics
        .total_file_reads
        .saturating_sub(activity.unique_files_read);
    activity.tool_calls_before_first_candidate_write = metrics.candidate_writes.saturating_sub(1);
    activity.time_to_first_candidate_mutation_ms = metrics
        .first_write
        .map(|time| time.duration_since(started).as_millis());
    activity
}

pub fn write_jsonl(path: &Path, records: &[BenchmarkRecord]) -> Result<(), String> {
    let mut output = String::new();
    for record in records {
        output.push_str(&serde_json::to_string(record).map_err(|error| error.to_string())?);
        output.push('\n');
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(path, output).map_err(|error| error.to_string())
}

#[allow(clippy::too_many_lines)]
pub fn compare_files(old: &Path, new: &Path) -> Result<String, String> {
    let old_records = read_jsonl(old)?;
    let new_records = read_jsonl(new)?;
    let old_pass = rate(&old_records, |record| {
        record.first_pass == "FIRST_PASS_PASS"
    });
    let new_pass = rate(&new_records, |record| {
        record.first_pass == "FIRST_PASS_PASS"
    });
    let old_tokens = mean_telemetry(&old_records, "input_tokens")
        .zip(mean_telemetry(&old_records, "output_tokens"))
        .map(|(input, output)| input + output);
    let new_tokens = mean_telemetry(&new_records, "input_tokens")
        .zip(mean_telemetry(&new_records, "output_tokens"))
        .map(|(input, output)| input + output);
    let old_time = mean_u128(&old_records, |record| record.timing.end_to_end_ms);
    let new_time = mean_u128(&new_records, |record| record.timing.end_to_end_ms);
    let mut output = String::from("# Agent benchmark comparison\n\n| Metric | Baseline | Current | Delta |\n|---|---:|---:|---:|\n");
    push_comparison_row(
        &mut output,
        "First-pass success",
        Some(old_pass),
        Some(new_pass),
    );
    push_comparison_row(
        &mut output,
        "Mean input tokens",
        mean_telemetry(&old_records, "input_tokens"),
        mean_telemetry(&new_records, "input_tokens"),
    );
    push_comparison_row(
        &mut output,
        "Mean output tokens",
        mean_telemetry(&old_records, "output_tokens"),
        mean_telemetry(&new_records, "output_tokens"),
    );
    push_comparison_row(
        &mut output,
        "Mean total model request bytes",
        mean_telemetry(&old_records, "model_request_bytes"),
        mean_telemetry(&new_records, "model_request_bytes"),
    );
    push_comparison_row(
        &mut output,
        "Mean provider calls",
        mean_telemetry(&old_records, "provider_calls"),
        mean_telemetry(&new_records, "provider_calls"),
    );
    push_comparison_row(
        &mut output,
        "Mean model turns",
        mean_telemetry(&old_records, "model_turns"),
        mean_telemetry(&new_records, "model_turns"),
    );
    push_comparison_row(
        &mut output,
        "Mean total file reads",
        mean_telemetry(&old_records, "total_file_reads"),
        mean_telemetry(&new_records, "total_file_reads"),
    );
    push_comparison_row(
        &mut output,
        "Mean unique file reads",
        mean_telemetry(&old_records, "unique_files_read"),
        mean_telemetry(&new_records, "unique_files_read"),
    );
    push_comparison_row(
        &mut output,
        "Mean repeated file reads",
        mean_telemetry(&old_records, "repeated_file_reads"),
        mean_telemetry(&new_records, "repeated_file_reads"),
    );
    push_comparison_row(
        &mut output,
        "Mean Grep calls",
        mean_telemetry(&old_records, "grep_calls"),
        mean_telemetry(&new_records, "grep_calls"),
    );
    push_comparison_row(
        &mut output,
        "Mean ContextSearch calls",
        mean_telemetry(&old_records, "context_search_calls"),
        mean_telemetry(&new_records, "context_search_calls"),
    );
    push_comparison_row(
        &mut output,
        "Mean tool calls before first mutation",
        mean_telemetry(&old_records, "tool_calls_before_first_candidate_mutation"),
        mean_telemetry(&new_records, "tool_calls_before_first_candidate_mutation"),
    );
    push_comparison_row(
        &mut output,
        "Mean model turns before first mutation",
        mean_telemetry(&old_records, "model_turns_before_first_candidate_mutation"),
        mean_telemetry(&new_records, "model_turns_before_first_candidate_mutation"),
    );
    push_comparison_row(
        &mut output,
        "Mean reads before first mutation",
        mean_telemetry(&old_records, "file_reads_before_first_candidate_mutation"),
        mean_telemetry(&new_records, "file_reads_before_first_candidate_mutation"),
    );
    push_comparison_row(
        &mut output,
        "Mean time to first mutation ms",
        mean_telemetry(&old_records, "time_to_first_candidate_mutation_ms"),
        mean_telemetry(&new_records, "time_to_first_candidate_mutation_ms"),
    );
    push_comparison_row(
        &mut output,
        "Mean wall time ms",
        Some(old_time),
        Some(new_time),
    );
    let _ = writeln!(
        output,
        "\nMean total tokens | Baseline: {} | Current: {}",
        format_optional(old_tokens),
        format_optional(new_tokens)
    );
    Ok(output)
}

fn telemetry_number(record: &BenchmarkRecord, key: &str) -> Option<f64> {
    record
        .production_telemetry
        .as_ref()
        .and_then(|telemetry| telemetry.get(key))
        .and_then(Value::as_f64)
}

#[allow(clippy::cast_precision_loss)]
fn mean_telemetry(records: &[BenchmarkRecord], key: &str) -> Option<f64> {
    let values: Vec<f64> = records
        .iter()
        .filter_map(|record| telemetry_number(record, key))
        .collect();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn format_optional(value: Option<f64>) -> String {
    value.map_or_else(|| "N/A".into(), |value| format!("{value:.1}"))
}

fn push_comparison_row(output: &mut String, name: &str, old: Option<f64>, new: Option<f64>) {
    let delta = old
        .zip(new)
        .map_or_else(|| "N/A".into(), |(old, new)| format!("{:+.1}", new - old));
    let _ = writeln!(
        output,
        "| {name} | {} | {} | {delta} |",
        format_optional(old),
        format_optional(new)
    );
}

fn read_jsonl(path: &Path) -> Result<Vec<BenchmarkRecord>, String> {
    fs::read_to_string(path)
        .map_err(|error| error.to_string())?
        .lines()
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect()
}
#[allow(clippy::cast_precision_loss)]
fn rate(records: &[BenchmarkRecord], predicate: impl Fn(&BenchmarkRecord) -> bool) -> f64 {
    if records.is_empty() {
        0.0
    } else {
        records.iter().filter(|record| predicate(record)).count() as f64 / records.len() as f64
    }
}
#[allow(clippy::cast_precision_loss)]
fn mean_u128(records: &[BenchmarkRecord], value: impl Fn(&BenchmarkRecord) -> u128) -> f64 {
    if records.is_empty() {
        0.0
    } else {
        records.iter().map(value).sum::<u128>() as f64 / records.len() as f64
    }
}
fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> BenchmarkTask {
        BenchmarkTask {
            id: id.into(),
            version: 1,
            category: "test".into(),
            language: "text".into(),
            prompt: "test".into(),
            fixture: Fixture {
                files: BTreeMap::new(),
                mock_actions: Vec::new(),
            },
            visible_validation: None,
            hidden_oracle: HiddenOracle {
                expected_files: BTreeMap::new(),
                forbidden_paths: Vec::new(),
            },
            expected_change_scope: Vec::new(),
            forbidden_changes: Vec::new(),
            timeout_seconds: 1,
            max_agent_turns: 1,
        }
    }

    #[test]
    fn explicit_task_selection_preserves_the_complete_task() {
        let tasks = vec![task("first"), task("config-threading"), task("last")];
        let selected = select_tasks(&tasks, Some("config-threading")).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "config-threading");
        assert_eq!(
            select_tasks(&tasks, Some("missing")).unwrap_err(),
            "task \"missing\" is not present in the corpus"
        );
        assert_eq!(select_tasks(&tasks, None).unwrap().len(), 3);
    }

    #[test]
    fn child_settings_require_and_preserve_model_resources() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(
            &path,
            r#"{"modelResources":[{"id":"profile-b","provider":"provider-b","model":"shared-model","endpoint":"http://b","protocolCapabilities":{"responses":false,"chatCompletions":true}}],"routing":{"allowRemote":true}}"#,
        )
        .unwrap();
        let settings = load_child_settings(&path).unwrap();
        assert_eq!(settings["modelResources"][0]["id"], "profile-b");
        assert_eq!(settings["modelResources"][0]["endpoint"], "http://b");
        assert_eq!(settings["routing"]["allowRemote"], true);
    }

    #[test]
    fn routed_attribution_does_not_claim_the_authorization_seed_executed() {
        let telemetry = json!({
            "provider_call_records": [{"profile": "gpt-5.4-mini"}]
        });
        assert_eq!(
            first_telemetry_profile(&telemetry).as_deref(),
            Some("gpt-5.4-mini")
        );
        assert_eq!(first_telemetry_profile(&json!({})), None);
    }

    #[test]
    fn local_profile_cost_is_zero() {
        let profile = ModelProfile {
            alias: "local".into(),
            provider_profile: "local".into(),
            model: "m".into(),
            reasoning: "not_applicable".into(),
            authorized: true,
            local: true,
            actual_input_usd_per_million: 0.0,
            actual_output_usd_per_million: 0.0,
            cached_input_usd_per_million: 0.0,
            cache_write_usd_per_million: 0.0,
        };
        assert!(cost(1_000_000, profile.actual_input_usd_per_million).abs() < f64::EPSILON);
    }

    #[test]
    fn hidden_oracle_is_not_inside_project() {
        let task = BenchmarkTask {
            id: "t".into(),
            version: 1,
            category: "mechanical".into(),
            language: "text".into(),
            prompt: "write".into(),
            fixture: Fixture {
                files: BTreeMap::new(),
                mock_actions: Vec::new(),
            },
            visible_validation: None,
            hidden_oracle: HiddenOracle {
                expected_files: BTreeMap::new(),
                forbidden_paths: vec![],
            },
            expected_change_scope: vec![],
            forbidden_changes: vec![],
            timeout_seconds: 1,
            max_agent_turns: 1,
        };
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        assert!(evaluate_oracle(&project, &task).unwrap());
    }

    #[test]
    fn hidden_oracle_ignores_claw_runtime_state() {
        let task = BenchmarkTask {
            id: "t".into(),
            version: 1,
            category: "mechanical".into(),
            language: "text".into(),
            prompt: "write".into(),
            fixture: Fixture {
                files: BTreeMap::new(),
                mock_actions: Vec::new(),
            },
            visible_validation: None,
            hidden_oracle: HiddenOracle {
                expected_files: BTreeMap::from([(String::from("answer.txt"), String::from("ok"))]),
                forbidden_paths: vec![],
            },
            expected_change_scope: vec![],
            forbidden_changes: vec![],
            timeout_seconds: 1,
            max_agent_turns: 1,
        };
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(project.join(".claw/sessions")).unwrap();
        fs::write(project.join("answer.txt"), "ok").unwrap();
        fs::write(
            project.join(".claw/sessions/session.jsonl"),
            "runtime state",
        )
        .unwrap();
        assert!(evaluate_oracle(&project, &task).unwrap());
    }

    #[test]
    fn production_selection_excludes_mock_and_requires_authorization() {
        let config = BenchmarkConfig::default();
        let error = select_production_profile(&config, None).unwrap_err();
        assert!(error.contains("no explicitly authorized real/local model profile"));
    }

    #[test]
    fn production_profile_selection_prefers_the_single_authorized_profile() {
        let config = BenchmarkConfig {
            schema_version: BENCHMARK_SCHEMA_VERSION,
            models: vec![ModelProfile {
                alias: "local-real".into(),
                provider_profile: "ollama".into(),
                model: "qwen".into(),
                reasoning: "not_applicable".into(),
                authorized: true,
                local: true,
                actual_input_usd_per_million: 0.0,
                actual_output_usd_per_million: 0.0,
                cached_input_usd_per_million: 0.0,
                cache_write_usd_per_million: 0.0,
            }],
        };
        assert_eq!(
            select_production_profile(&config, None).unwrap().alias,
            "local-real"
        );
    }

    #[test]
    fn partial_telemetry_is_recovered_without_leaking_marker_into_error() {
        let error = with_partial_telemetry(
            "production task timed out",
            Path::new("/definitely/missing/telemetry.json"),
        );
        assert_eq!(split_partial_telemetry(&error), (error, None));

        let value = json!({
            "repository_intelligence_attempted": true,
            "repository_intelligence_seed_count": 2,
            "repository_intelligence_context_used": true
        });
        let encoded = format!("failure{PARTIAL_TELEMETRY_MARKER}{value}");
        let (message, telemetry) = split_partial_telemetry(&encoded);
        assert_eq!(message, "failure");
        assert_eq!(telemetry, Some(value));
    }

    #[test]
    fn timeout_finalizes_partial_telemetry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("telemetry.json");
        fs::write(
            &path,
            r#"{"terminal_status":"in_progress","provider_calls":1}"#,
        )
        .unwrap();
        let error = with_timeout_telemetry("timed out", &path);
        let (_, telemetry) = split_partial_telemetry(&error);
        assert_eq!(telemetry.unwrap()["terminal_status"], "timeout");
        let persisted = fs::read_to_string(path).unwrap();
        assert!(persisted.contains(r#""terminal_status":"timeout""#));
    }

    #[test]
    fn failure_finalizes_partial_telemetry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("telemetry.json");
        fs::write(
            &path,
            r#"{"terminal_status":"in_progress","provider_calls":1}"#,
        )
        .unwrap();
        let error = with_partial_telemetry("failed", &path);
        let (_, telemetry) = split_partial_telemetry(&error);
        assert_eq!(telemetry.unwrap()["terminal_status"], "failed");
        let persisted = fs::read_to_string(path).unwrap();
        assert!(persisted.contains(r#""terminal_status":"failed""#));
    }
}
