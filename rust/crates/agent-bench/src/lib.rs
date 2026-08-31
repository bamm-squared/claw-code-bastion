//! Deterministic measurement infrastructure for the current Bastion runtime.
//!
//! The `run_mock` path is deliberately a harness validation path: it drives the
//! real `runtime::ConversationRuntime`, usage accounting, permission policy,
//! session tracing, tool loop, and result serialization with a deterministic
//! scripted provider. It is not presented as a model-quality baseline.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, PermissionMode, PermissionPolicy,
    ProjectContext, RuntimeError, Session, SystemPromptBuilder, ToolError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use telemetry::{MemoryTelemetrySink, SessionTracer, TelemetryEvent};

pub const BENCHMARK_SCHEMA_VERSION: u32 = 1;

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
    Ok(tasks)
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

pub fn compare_files(old: &Path, new: &Path) -> Result<String, String> {
    let old_records = read_jsonl(old)?;
    let new_records = read_jsonl(new)?;
    let old_pass = rate(&old_records, |record| {
        record.first_pass == "FIRST_PASS_PASS"
    });
    let new_pass = rate(&new_records, |record| {
        record.first_pass == "FIRST_PASS_PASS"
    });
    let old_tokens = mean(&old_records, |record| {
        record.usage.input_tokens + record.usage.output_tokens
    });
    let new_tokens = mean(&new_records, |record| {
        record.usage.input_tokens + record.usage.output_tokens
    });
    let old_time = mean_u128(&old_records, |record| record.timing.end_to_end_ms);
    let new_time = mean_u128(&new_records, |record| record.timing.end_to_end_ms);
    Ok(format!("# Agent benchmark comparison\n\n| Metric | Baseline | Current | Delta |\n|---|---:|---:|---:|\n| First-pass success | {old_pass:.3} | {new_pass:.3} | {:+.3} |\n| Mean tokens | {old_tokens:.1} | {new_tokens:.1} | {:+.1} |\n| Mean wall time ms | {old_time:.1} | {new_time:.1} | {:+.1} |\n", new_pass - old_pass, new_tokens - old_tokens, new_time - old_time))
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
fn mean(records: &[BenchmarkRecord], value: impl Fn(&BenchmarkRecord) -> u64) -> f64 {
    if records.is_empty() {
        0.0
    } else {
        records.iter().map(value).sum::<u64>() as f64 / records.len() as f64
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
}
