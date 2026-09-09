use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Default)]
pub struct Snapshot {
    pub schema_version: u32,
    pub run_id: String,
    pub provider_calls: u64,
    pub provider_request_ids: Vec<String>,
    pub provider_empty_response_recoveries: u64,
    pub provider_transient_recoveries: u64,
    pub provider_rate_limit_pacing_events: u64,
    pub provider_rate_limit_pacing_seconds: u64,
    pub writer_checkpoint_candidate_checks: u64,
    pub writer_checkpoint_before_context_tokens: Option<u64>,
    pub writer_checkpoint_after_context_tokens: Option<u64>,
    pub writer_checkpoint_context_reduction_tokens: Option<u64>,
    pub writer_checkpoint_compacted_messages: Option<u64>,
    pub writer_checkpoint_reason: Option<String>,
    pub writer_request_estimate_tokens: Option<u64>,
    pub writer_provider_token_limit: Option<u64>,
    pub writer_provider_token_remaining: Option<u64>,
    pub writer_provider_token_reset_after_seconds: Option<u64>,
    pub work_units_total: u64,
    pub work_units_completed: u64,
    pub work_unit_transitions: u64,
    pub current_work_unit: Option<String>,
    pub work_unit_replans: u64,
    pub work_unit_writer_turns: u64,
    pub work_unit_turn_allowance: u64,
    pub work_unit_continuation_grants: u64,
    pub work_unit_completion_rejections: u64,
    pub work_unit_no_change_attempts: u64,
    pub work_unit_terminal_reason: Option<String>,
    pub candidate_check_evidence: Vec<CandidateCheckEvidence>,
    pub work_unit_checkpoints: Vec<WorkUnitCheckpoint>,
    pub model_turns: u64,
    pub tool_bearing_turns: u64,
    pub tool_calls: BTreeMap<String, u64>,
    pub model_request_bytes: u64,
    pub repository_intelligence_attempted: Option<bool>,
    pub repository_intelligence_seed_count: Option<u64>,
    pub repository_intelligence_context_used: Option<bool>,
    pub repository_intelligence_context_bytes: Option<u64>,
    pub repository_intelligence_nodes_used: u64,
    pub repository_intelligence_edges_used: u64,
    pub impact_query_count: u64,
    pub total_file_reads: u64,
    pub unique_files_read: u64,
    pub repeated_file_reads: u64,
    pub grep_calls: u64,
    pub context_search_calls: u64,
    pub repository_intelligence_enabled: Option<bool>,
    pub time_to_first_tool_call_ms: Option<u128>,
    pub time_to_first_candidate_mutation_ms: Option<u128>,
    pub model_turns_before_first_candidate_mutation: Option<u64>,
    pub tool_calls_before_first_candidate_mutation: Option<u64>,
    pub file_reads_before_first_candidate_mutation: Option<u64>,
    pub unique_files_read_before_first_candidate_mutation: Option<u64>,
    pub repeated_file_reads_before_first_candidate_mutation: Option<u64>,
    pub grep_calls_before_first_candidate_mutation: Option<u64>,
    pub context_search_calls_before_first_candidate_mutation: Option<u64>,
    pub input_tokens_before_first_candidate_mutation: Option<u64>,
    pub output_tokens_before_first_candidate_mutation: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub thinking_present: bool,
    pub content_present: bool,
    pub candidate_mutations: u64,
    pub validation_attempts: u64,
    pub validation_result: Option<String>,
    pub validation_candidate_identity: Option<String>,
    pub validation_identity: Option<String>,
    pub validation_checks: Vec<ValidationDiagnostic>,
    pub validation_history: Vec<ValidationAttempt>,
    pub rework_cycles: u64,
    pub validation_repair_cycles: u64,
    pub evaluator_rework_cycles: u64,
    pub evaluator_selected_profile: Option<String>,
    pub evaluator_route_reason: Option<String>,
    pub evaluator_route_rejections: Vec<RoutingRejection>,
    pub writer_selected_profile: Option<String>,
    pub writer_route_reason: Option<String>,
    pub writer_route_rejections: Vec<RoutingRejection>,
    pub writer_route_estimate: Option<RoutingEstimate>,
    pub writer_profile_events: Vec<WriterProfileEvent>,
    pub planning_artifact: Option<Value>,
    pub writer_packet_events: Vec<WriterPacketEvent>,
    pub blocked_checkpoint_events: Vec<BlockedCheckpointEvent>,
    pub candidate_artifact: Option<CandidateArtifact>,
    pub evaluation_blocked_reason: Option<String>,
    pub requirement_coverage: Vec<RequirementCoverage>,
    pub started_at_ms: u128,
    pub elapsed_ms: u128,
    pub terminal_status: String,
    pub lifecycle_events: Vec<String>,
    pub provider_call_records: Vec<ProviderCallRecord>,
    pub execution_stages: Vec<ExecutionStage>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ProviderCallRecord {
    pub sequence: u64,
    pub cycle_id: String,
    pub started_at_ms: u128,
    pub finished_at_ms: Option<u128>,
    pub status: String,
    pub terminal_error: Option<String>,
    pub usage_known: bool,
    pub request_bytes: u64,
    pub role: Option<String>,
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub protocol: Option<String>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_policy: Option<String>,
    pub tools_supported: Option<bool>,
    pub context_window: Option<u32>,
    pub rate_limit: Option<api::RateLimitState>,
    pub request_ids: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub estimated_cost_usd: Option<f64>,
    pub price_source: Option<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ExecutionStage {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub cycle_id: Option<String>,
    pub stage: String,
    pub tool: Option<String>,
    pub request_bytes: Option<u64>,
    pub result_bytes: Option<u64>,
    pub terminal_status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct WriterProfileEvent {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub work_unit: Option<String>,
    pub role: String,
    pub previous_profile: Option<String>,
    pub profile: String,
    pub provider: Option<String>,
    pub model: String,
    pub protocol: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_policy: Option<String>,
    pub selection_source: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct CandidateArtifact {
    pub candidate_identity: String,
    pub changed_paths: Vec<String>,
    pub diff: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ValidationDiagnostic {
    pub name: String,
    pub command: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ValidationAttempt {
    pub candidate_identity: String,
    pub validation_identity: String,
    pub checks: Vec<ValidationDiagnostic>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct CandidateCheckEvidence {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub work_unit: Option<String>,
    pub check: String,
    pub command: String,
    pub classification: String,
    pub code_failure: bool,
    pub status: String,
    pub exit_code: Option<i32>,
    pub diagnostic: String,
    pub candidate_identity: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct WorkUnitCheckpoint {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub work_unit: Option<String>,
    pub writer_turns: u64,
    pub turn_allowance: u64,
    pub productive_writer_turns: u64,
    pub checkpoint_turn: bool,
    pub remaining_turns: u64,
    pub continuation_grants: u64,
    pub candidate_changed: Option<bool>,
    pub candidate_identity: Option<String>,
    pub candidate_check_evidence_ids: Vec<u64>,
    pub requested_status: String,
    pub reconciliation_outcome: Option<String>,
    pub unresolved_completion: Option<String>,
    pub terminal_reason: Option<String>,
    pub blocker_category: Option<String>,
    pub blocker_reason: Option<String>,
    pub declared_contract_ids: Vec<String>,
    pub continuation_eligible: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct WriterPacketEvent {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub task_id: String,
    pub work_unit: Option<String>,
    pub profile: String,
    pub model: String,
    pub protocol: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_policy: String,
    pub instruction_version: String,
    pub tool_schema_version: String,
    pub contract_ids: Vec<String>,
    pub owned_contract_ids: Vec<String>,
    pub downstream_contract_ids: Vec<String>,
    pub global_invariant_ids: Vec<String>,
    pub repository_fact_ids: Vec<String>,
    pub candidate_identity: String,
    pub context_message_count: u64,
    pub context_bytes: u64,
    pub packet_bytes: u64,
    pub plan_hash: String,
    pub packet_hash: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct BlockedCheckpointEvent {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub work_unit: Option<String>,
    pub writer_turns: u64,
    pub productive_writer_turns: u64,
    pub turn_allowance: u64,
    pub checkpoint_turn: bool,
    pub turns_remaining: u64,
    pub continuation_grants: u64,
    pub candidate_identity: String,
    pub category: String,
    pub reason: String,
    pub declared_contract_ids: Vec<String>,
    pub owned_contract_ids: Vec<String>,
    pub downstream_contract_ids: Vec<String>,
    pub global_invariant_ids: Vec<String>,
    pub latest_evidence_ids: Vec<u64>,
    pub continuation_eligible: bool,
    pub terminal_reason: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct RequirementCoverage {
    pub id: String,
    pub status: String,
    pub evidence: String,
    pub boundary: String,
    pub basis: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct RoutingRejection {
    pub profile_id: String,
    pub reason: String,
    pub capability: crate::model_router::Capability,
    pub required: crate::model_router::CapabilityRequirement,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct RoutingEstimate {
    pub coding: u8,
    pub reasoning: u8,
    pub agent_tool_use: u8,
    pub planning: u8,
    pub evaluation: u8,
    pub context_window: u32,
    pub safety_margin: u8,
    pub ambiguity: u8,
    pub scope: u8,
    pub risk: u8,
    pub unresolved: u8,
}

#[derive(Clone, Debug, Default)]
struct ProviderContext {
    role: Option<String>,
    profile: Option<String>,
    provider: Option<String>,
    protocol: Option<String>,
    model: Option<String>,
    endpoint: Option<String>,
    reasoning_effort: Option<String>,
    reasoning_policy: Option<String>,
    tools_supported: Option<bool>,
    context_window: Option<u32>,
    rate_limit: Option<api::RateLimitState>,
    input_rate: Option<f64>,
    output_rate: Option<f64>,
    price_source: Option<String>,
}

struct State {
    path: PathBuf,
    started: Instant,
    snapshot: Snapshot,
    read_identities: HashSet<u64>,
    first_tool_at: Option<Instant>,
    first_mutation_recorded: bool,
    provider_context: ProviderContext,
    active_provider_record: Option<usize>,
}

static STATE: OnceLock<Mutex<Option<State>>> = OnceLock::new();

pub fn init() {
    let Some(path) = std::env::var_os("CLAW_BENCH_TELEMETRY") else {
        return;
    };
    let _ = STATE.set(Mutex::new(Some(State {
        path: path.into(),
        started: Instant::now(),
        snapshot: Snapshot {
            schema_version: 1,
            run_id: format!("claw-{}", std::process::id()),
            started_at_ms: now_ms(),
            repository_intelligence_enabled: Some(graph_context_enabled()),
            ..Snapshot::default()
        },
        read_identities: HashSet::new(),
        first_tool_at: None,
        first_mutation_recorded: false,
        provider_context: ProviderContext::default(),
        active_provider_record: None,
    })));
}

fn graph_context_enabled_value(value: Option<&str>) -> bool {
    value != Some("off")
}

pub fn graph_context_enabled() -> bool {
    graph_context_enabled_value(std::env::var("CLAW_BENCH_GRAPH_CONTEXT").ok().as_deref())
}

fn with_state(f: impl FnOnce(&mut State)) {
    let Some(lock) = STATE.get() else { return };
    let Ok(mut guard) = lock.lock() else { return };
    if let Some(state) = guard.as_mut() {
        f(state);
    }
}

pub fn provider_call() {
    with_state(|s| {
        s.snapshot.provider_calls += 1;
        let context = &s.provider_context;
        let sequence = s.snapshot.provider_calls;
        let cycle_id = format!("provider-call-{sequence}");
        s.snapshot.provider_call_records.push(ProviderCallRecord {
            sequence,
            cycle_id: cycle_id.clone(),
            started_at_ms: now_ms(),
            status: String::from("started"),
            role: context.role.clone(),
            profile: context.profile.clone(),
            provider: context.provider.clone(),
            protocol: context.protocol.clone(),
            model: context.model.clone(),
            endpoint: context.endpoint.clone(),
            reasoning_effort: context.reasoning_effort.clone(),
            reasoning_policy: context.reasoning_policy.clone(),
            tools_supported: context.tools_supported,
            context_window: context.context_window,
            rate_limit: context.rate_limit.clone(),
            price_source: context.price_source.clone(),
            ..ProviderCallRecord::default()
        });
        s.active_provider_record = Some(s.snapshot.provider_call_records.len() - 1);
        push_execution_stage(s, "provider_request_started", None, None, None, None);
    });
    persist_snapshot();
}

pub fn provider_call_finished(status: &str, error: Option<&str>) {
    with_state(|s| {
        if let Some(index) = s.active_provider_record {
            {
                let record = &mut s.snapshot.provider_call_records[index];
                record.status = status.chars().take(64).collect();
                record.finished_at_ms = Some(now_ms());
                record.terminal_error = error.map(bounded_diagnostic);
            }
            let stage = if status == "completed" {
                "provider_response_completed"
            } else {
                "provider_attempt_finished"
            };
            push_execution_stage(s, stage, None, None, None, Some(status));
        }
    });
    persist_snapshot();
}

pub fn execution_stage(
    stage: &str,
    tool: Option<&str>,
    request_bytes: Option<u64>,
    result_bytes: Option<u64>,
    terminal_status: Option<&str>,
) {
    with_state(|s| {
        push_execution_stage(s, stage, tool, request_bytes, result_bytes, terminal_status);
    });
    persist_snapshot();
}

fn push_execution_stage(
    telemetry_state: &mut State,
    stage_name: &str,
    tool: Option<&str>,
    request_bytes: Option<u64>,
    result_bytes: Option<u64>,
    terminal_status: Option<&str>,
) {
    if telemetry_state.snapshot.execution_stages.len() >= 256 {
        return;
    }
    let cycle_id = telemetry_state
        .active_provider_record
        .and_then(|index| telemetry_state.snapshot.provider_call_records.get(index))
        .map(|record| record.cycle_id.clone());
    telemetry_state
        .snapshot
        .execution_stages
        .push(ExecutionStage {
            sequence: telemetry_state.snapshot.execution_stages.len() as u64 + 1,
            timestamp_ms: now_ms(),
            cycle_id,
            stage: stage_name.chars().take(64).collect(),
            tool: tool.map(|value| value.chars().take(64).collect()),
            request_bytes,
            result_bytes,
            terminal_status: terminal_status.map(|value| value.chars().take(64).collect()),
        });
}

pub fn set_provider_context(
    role: &str,
    profile: Option<&str>,
    provider: Option<&str>,
    protocol: Option<&str>,
    input_rate: Option<f64>,
    output_rate: Option<f64>,
    price_source: Option<&str>,
) {
    with_state(|s| {
        s.provider_context = ProviderContext {
            role: Some(role.to_string()),
            profile: profile.map(str::to_string),
            provider: provider.map(str::to_string),
            protocol: protocol.map(str::to_string),
            input_rate,
            output_rate,
            price_source: price_source.map(str::to_string),
            ..ProviderContext::default()
        };
    });
}

pub fn set_provider_protocol(protocol: &str) {
    with_state(|s| {
        s.provider_context.protocol = Some(protocol.to_string());
    });
}

pub fn set_provider_reasoning(effort: &str, policy: &str) {
    with_state(|s| {
        s.provider_context.reasoning_effort = Some(effort.to_string());
        s.provider_context.reasoning_policy = Some(policy.to_string());
    });
}

pub fn provider_protocol() -> Option<String> {
    STATE
        .get()
        .and_then(|lock| lock.lock().ok())
        .and_then(|guard| guard.as_ref()?.provider_context.protocol.clone())
}

/// Attach the effective, non-secret execution configuration to subsequent
/// provider-call records. The endpoint is expected to be redacted by the
/// caller before it reaches telemetry.
pub fn set_provider_execution(
    model: Option<&str>,
    endpoint: Option<&str>,
    reasoning_effort: Option<&str>,
    reasoning_policy: Option<&str>,
    tools_supported: Option<bool>,
    context_window: Option<u32>,
) {
    with_state(|s| {
        s.provider_context.model = model.map(str::to_string);
        s.provider_context.endpoint = endpoint.map(str::to_string);
        s.provider_context.reasoning_effort = reasoning_effort.map(str::to_string);
        s.provider_context.reasoning_policy = reasoning_policy.map(str::to_string);
        s.provider_context.tools_supported = tools_supported;
        s.provider_context.context_window = context_window;
    });
}

pub fn set_provider_rate_limit(state: Option<&api::RateLimitState>) {
    with_state(|s| {
        s.provider_context.rate_limit = state.cloned();
        if let Some(index) = s.active_provider_record {
            s.snapshot.provider_call_records[index].rate_limit = state.cloned();
        }
    });
}

pub fn provider_request_id(request_id: &str) {
    with_state(|s| {
        if !request_id.is_empty()
            && !s
                .snapshot
                .provider_request_ids
                .iter()
                .any(|id| id == request_id)
            && s.snapshot.provider_request_ids.len() < 64
        {
            s.snapshot.provider_request_ids.push(request_id.to_string());
        }
        if let Some(index) = s.active_provider_record {
            let record = &mut s.snapshot.provider_call_records[index];
            if !request_id.is_empty() && !record.request_ids.iter().any(|id| id == request_id) {
                record.request_ids.push(request_id.to_string());
            }
        }
    });
}

pub fn provider_empty_response_recovery() {
    with_state(|s| {
        s.snapshot.provider_empty_response_recoveries = s
            .snapshot
            .provider_empty_response_recoveries
            .saturating_add(1);
    });
    lifecycle_event("provider_empty_response_recovery");
}

pub fn provider_transient_recovery() {
    with_state(|s| {
        s.snapshot.provider_transient_recoveries =
            s.snapshot.provider_transient_recoveries.saturating_add(1);
    });
    lifecycle_event("provider_transient_recovery");
}

pub fn provider_rate_limit_pacing(seconds: u64) {
    with_state(|s| {
        s.snapshot.provider_rate_limit_pacing_events = s
            .snapshot
            .provider_rate_limit_pacing_events
            .saturating_add(1);
        s.snapshot.provider_rate_limit_pacing_seconds = s
            .snapshot
            .provider_rate_limit_pacing_seconds
            .saturating_add(seconds);
    });
    lifecycle_event("provider_rate_limit_pacing");
}

pub fn writer_checkpoint_candidate_check() {
    with_state(|s| {
        s.snapshot.writer_checkpoint_candidate_checks = s
            .snapshot
            .writer_checkpoint_candidate_checks
            .saturating_add(1);
    });
    lifecycle_event("writer_checkpoint_candidate_check");
}

pub fn writer_checkpoint_context(
    before_tokens: usize,
    after_tokens: usize,
    removed_messages: usize,
) {
    with_state(|s| {
        s.snapshot.writer_checkpoint_before_context_tokens = Some(before_tokens as u64);
        s.snapshot.writer_checkpoint_after_context_tokens = Some(after_tokens as u64);
        s.snapshot.writer_checkpoint_context_reduction_tokens =
            Some(before_tokens.saturating_sub(after_tokens) as u64);
        s.snapshot.writer_checkpoint_compacted_messages = Some(removed_messages as u64);
    });
    lifecycle_event("writer_checkpoint_context_compacted");
}

pub fn writer_checkpoint_reason(reason: &str) {
    with_state(|s| s.snapshot.writer_checkpoint_reason = Some(reason.to_string()));
    lifecycle_event("writer_checkpoint_resource_triggered");
}

pub fn writer_request_resource_estimate(
    estimated_tokens: u64,
    token_limit: Option<u64>,
    token_remaining: Option<u64>,
    reset_after_seconds: Option<u64>,
) {
    with_state(|s| {
        s.snapshot.writer_request_estimate_tokens = Some(estimated_tokens);
        s.snapshot.writer_provider_token_limit = token_limit;
        s.snapshot.writer_provider_token_remaining = token_remaining;
        s.snapshot.writer_provider_token_reset_after_seconds = reset_after_seconds;
    });
}

pub fn work_unit_state(total: usize, completed: usize, current: Option<&str>) {
    with_state(|s| {
        s.snapshot.work_units_total = total as u64;
        s.snapshot.work_units_completed = completed as u64;
        s.snapshot.current_work_unit = current.map(str::to_string);
    });
}

pub fn work_unit_transition(completed: &str, next: Option<&str>) {
    with_state(|s| {
        s.snapshot.work_unit_transitions = s.snapshot.work_unit_transitions.saturating_add(1);
        s.snapshot.work_units_completed = s.snapshot.work_units_completed.saturating_add(1);
        s.snapshot.current_work_unit = next.map(str::to_string);
    });
    lifecycle_event(&format!("work_unit_completed:{completed}"));
    lifecycle_event(next.map_or("work_unit_plan_complete", |_| "work_unit_advanced"));
}

pub fn work_unit_replanned() {
    with_state(|s| {
        s.snapshot.work_unit_replans = s.snapshot.work_unit_replans.saturating_add(1);
    });
    lifecycle_event("work_unit_replanned");
}

pub fn work_unit_budget(used: usize, allowance: usize, continuation_grants: u8) {
    with_state(|s| {
        s.snapshot.work_unit_writer_turns = used as u64;
        s.snapshot.work_unit_turn_allowance = allowance as u64;
        s.snapshot.work_unit_continuation_grants = u64::from(continuation_grants);
    });
}

pub fn work_unit_checkpoint_runtime_state(used: usize, allowance: usize, continuation_grants: u8) {
    with_state(|s| {
        if let Some(record) = s
            .snapshot
            .work_unit_checkpoints
            .last_mut()
            .filter(|record| record.reconciliation_outcome.is_none())
        {
            record.writer_turns = used as u64;
            record.turn_allowance = allowance as u64;
            record.productive_writer_turns = used.min(allowance) as u64;
            record.checkpoint_turn = used > allowance;
            record.remaining_turns = allowance.saturating_sub(used) as u64;
            record.continuation_grants = u64::from(continuation_grants);
            if record.work_unit.is_none() {
                record.work_unit.clone_from(&s.snapshot.current_work_unit);
            }
        }
    });
    persist_snapshot();
}

pub fn work_unit_reconciliation_counters(rejections: u8, no_change_attempts: u8) {
    with_state(|s| {
        s.snapshot.work_unit_completion_rejections = u64::from(rejections);
        s.snapshot.work_unit_no_change_attempts = u64::from(no_change_attempts);
    });
    persist_snapshot();
}

pub fn work_unit_terminal(reason: &str) {
    with_state(|s| {
        s.snapshot.work_unit_terminal_reason = Some(reason.chars().take(256).collect());
    });
    lifecycle_event(&format!("work_unit_terminal:{reason}"));
}

#[allow(clippy::too_many_lines)]
pub fn candidate_check_result(raw: &str) {
    with_state(|s| {
        let value = serde_json::from_str::<Value>(raw).ok();
        let work_unit = s.snapshot.current_work_unit.clone();
        let default_classification = value
            .as_ref()
            .and_then(|value| value.get("classification"))
            .and_then(Value::as_str)
            .unwrap_or("malformed")
            .to_string();
        let candidate_identity = value
            .as_ref()
            .and_then(|value| value.get("candidate_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let checks = value
            .as_ref()
            .and_then(|value| value.get("checks"))
            .and_then(Value::as_array);
        let mut records = Vec::new();
        if let Some(checks) = checks {
            for check in checks.iter().take(32) {
                let status = check
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("malformed")
                    .to_string();
                let (classification, code_failure) = check_classification(check, &status);
                let diagnostic = check
                    .get("stderr")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .or_else(|| check.get("stdout").and_then(Value::as_str))
                    .unwrap_or_default();
                records.push(CandidateCheckEvidence {
                    sequence: s.snapshot.candidate_check_evidence.len() as u64
                        + records.len() as u64
                        + 1,
                    timestamp_ms: now_ms(),
                    work_unit: work_unit.clone(),
                    check: check
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("candidate_check")
                        .to_string(),
                    command: check
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("candidate_check")
                        .to_string(),
                    classification,
                    code_failure,
                    status,
                    exit_code: check
                        .get("exit_code")
                        .and_then(Value::as_i64)
                        .and_then(|code| i32::try_from(code).ok()),
                    diagnostic: bounded_diagnostic(diagnostic),
                    candidate_identity: candidate_identity.clone(),
                    truncated: check
                        .get("truncated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
        }
        if records.is_empty() {
            records.push(CandidateCheckEvidence {
                sequence: s.snapshot.candidate_check_evidence.len() as u64 + 1,
                timestamp_ms: now_ms(),
                work_unit,
                check: value
                    .as_ref()
                    .and_then(|value| value.get("kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("candidate_check")
                    .to_string(),
                command: "candidate_check".to_string(),
                classification: default_classification.clone(),
                code_failure: default_classification == "candidate_failure",
                status: value
                    .as_ref()
                    .and_then(|value| value.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("malformed")
                    .to_string(),
                exit_code: None,
                diagnostic: value
                    .as_ref()
                    .and_then(|value| value.get("error").or_else(|| value.get("diagnostic")))
                    .and_then(Value::as_str)
                    .map_or_else(|| bounded_diagnostic(raw), bounded_diagnostic),
                candidate_identity,
                truncated: raw.len() > 8_000,
            });
        }
        s.snapshot.candidate_check_evidence.extend(records);
        if s.snapshot.candidate_check_evidence.len() > 128 {
            let excess = s.snapshot.candidate_check_evidence.len() - 128;
            s.snapshot.candidate_check_evidence.drain(0..excess);
        }
    });
    lifecycle_event("candidate_check_evidence_recorded");
}

pub fn candidate_check_error(error: &str) {
    candidate_check_result(
        &serde_json::json!({
            "kind": "candidate_development_check",
            "status": "infrastructure_error",
            "classification": "infrastructure_failure",
            "code_failure": false,
            "error": error,
        })
        .to_string(),
    );
}

pub fn work_unit_checkpoint_requested(requested_status: &str, candidate_changed: Option<bool>) {
    with_state(|s| {
        let evidence = s
            .snapshot
            .candidate_check_evidence
            .iter()
            .rev()
            .take(8)
            .map(|item| item.sequence)
            .collect::<Vec<_>>();
        let candidate_identity = s
            .snapshot
            .candidate_check_evidence
            .iter()
            .rev()
            .find_map(|item| item.candidate_identity.clone());
        let record = WorkUnitCheckpoint {
            sequence: s.snapshot.work_unit_checkpoints.len() as u64 + 1,
            timestamp_ms: now_ms(),
            work_unit: s.snapshot.current_work_unit.clone(),
            writer_turns: s.snapshot.work_unit_writer_turns,
            turn_allowance: s.snapshot.work_unit_turn_allowance,
            productive_writer_turns: s
                .snapshot
                .work_unit_writer_turns
                .min(s.snapshot.work_unit_turn_allowance),
            checkpoint_turn: s.snapshot.work_unit_writer_turns
                > s.snapshot.work_unit_turn_allowance,
            remaining_turns: s
                .snapshot
                .work_unit_turn_allowance
                .saturating_sub(s.snapshot.work_unit_writer_turns),
            continuation_grants: s.snapshot.work_unit_continuation_grants,
            candidate_changed,
            candidate_identity,
            candidate_check_evidence_ids: evidence,
            requested_status: requested_status.chars().take(64).collect(),
            ..WorkUnitCheckpoint::default()
        };
        s.snapshot.work_unit_checkpoints.push(record);
        if s.snapshot.work_unit_checkpoints.len() > 128 {
            let excess = s.snapshot.work_unit_checkpoints.len() - 128;
            s.snapshot.work_unit_checkpoints.drain(0..excess);
        }
    });
    lifecycle_event("work_unit_checkpoint_recorded");
}

pub fn blocked_checkpoint_requested(reason: &str) {
    with_state(|s| {
        let writer_turns = s.snapshot.work_unit_writer_turns;
        let turn_allowance = s.snapshot.work_unit_turn_allowance;
        let latest_evidence_ids = s
            .snapshot
            .candidate_check_evidence
            .iter()
            .rev()
            .take(8)
            .map(|item| item.sequence)
            .collect::<Vec<_>>();
        let candidate_identity = s
            .snapshot
            .candidate_check_evidence
            .iter()
            .rev()
            .find_map(|item| item.candidate_identity.clone())
            .unwrap_or_else(|| "candidate-unknown".to_string());
        record_blocked_checkpoint_event(
            s,
            BlockedCheckpointEvent {
                work_unit: s.snapshot.current_work_unit.clone(),
                writer_turns,
                productive_writer_turns: writer_turns.min(turn_allowance),
                turn_allowance,
                checkpoint_turn: writer_turns > turn_allowance,
                turns_remaining: turn_allowance.saturating_sub(writer_turns),
                continuation_grants: s.snapshot.work_unit_continuation_grants,
                candidate_identity,
                category: "writer_reported_blocked".to_string(),
                reason: reason.to_string(),
                latest_evidence_ids,
                continuation_eligible: false,
                terminal_reason: "checkpoint_requested".to_string(),
                ..BlockedCheckpointEvent::default()
            },
        );
    });
    persist_snapshot();
}

pub fn work_unit_checkpoint_reconciled(
    outcome: &str,
    unresolved_completion: Option<&str>,
    terminal_reason: Option<&str>,
) {
    with_state(|s| {
        if let Some(record) = s.snapshot.work_unit_checkpoints.last_mut() {
            record.reconciliation_outcome = Some(outcome.chars().take(64).collect());
            record.unresolved_completion = unresolved_completion.map(bounded_diagnostic);
            record.terminal_reason = terminal_reason.map(|value| value.chars().take(256).collect());
        }
    });
    persist_snapshot();
}

pub fn work_unit_checkpoint_candidate_state(changed: bool) {
    with_state(|s| {
        if let Some(record) = s.snapshot.work_unit_checkpoints.last_mut() {
            record.candidate_changed = Some(changed);
        }
    });
    persist_snapshot();
}

pub fn work_unit_checkpoint_refresh_evidence() {
    with_state(|s| {
        let evidence = s
            .snapshot
            .candidate_check_evidence
            .iter()
            .rev()
            .take(8)
            .map(|item| item.sequence)
            .collect::<Vec<_>>();
        let candidate_identity = s
            .snapshot
            .candidate_check_evidence
            .iter()
            .rev()
            .find_map(|item| item.candidate_identity.clone());
        if let Some(record) = s.snapshot.work_unit_checkpoints.last_mut() {
            record.candidate_check_evidence_ids = evidence;
            record.candidate_identity = candidate_identity;
        }
    });
    persist_snapshot();
}

pub fn model_turn() {
    with_state(|s| s.snapshot.model_turns += 1);
}
pub fn tool(name: &str) {
    tool_event(name, None);
}
pub fn tool_event(name: &str, input: Option<&str>) {
    with_state(|s| record_tool_event(s, name, input));
}
pub fn tool_turn() {
    with_state(|s| s.snapshot.tool_bearing_turns += 1);
}
pub fn thinking() {
    with_state(|s| s.snapshot.thinking_present = true);
}
pub fn content() {
    with_state(|s| s.snapshot.content_present = true);
}
#[allow(clippy::cast_precision_loss)]
pub fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) {
    with_state(|s| {
        add_usage(&mut s.snapshot.input_tokens, input);
        add_usage(&mut s.snapshot.output_tokens, output);
        add_usage(&mut s.snapshot.cache_read_tokens, cache_read);
        add_usage(&mut s.snapshot.cache_write_tokens, cache_write);
        if let Some(index) = s.active_provider_record {
            let record = &mut s.snapshot.provider_call_records[index];
            record.usage_known = true;
            record.input_tokens = record.input_tokens.saturating_add(input);
            record.output_tokens = record.output_tokens.saturating_add(output);
            record.cache_read_tokens = record.cache_read_tokens.saturating_add(cache_read);
            record.cache_write_tokens = record.cache_write_tokens.saturating_add(cache_write);
            if let (Some(input_rate), Some(output_rate)) = (
                s.provider_context.input_rate,
                s.provider_context.output_rate,
            ) {
                record.estimated_cost_usd = Some(
                    (record.input_tokens as f64 * input_rate
                        + record.output_tokens as f64 * output_rate)
                        / 1_000_000.0,
                );
            }
        }
    });
}

pub fn model_request_bytes(bytes: u64) {
    with_state(|s| {
        s.snapshot.model_request_bytes = s.snapshot.model_request_bytes.saturating_add(bytes);
        if let Some(index) = s.active_provider_record {
            let record = &mut s.snapshot.provider_call_records[index];
            record.request_bytes = record.request_bytes.saturating_add(bytes);
        }
    });
}

pub fn repository_intelligence_attempted() {
    with_state(|s| s.snapshot.repository_intelligence_attempted = Some(true));
    persist_snapshot();
}

pub fn repository_intelligence_selection(seed_count: usize, injected: bool) {
    with_state(|s| {
        s.snapshot.repository_intelligence_attempted = Some(true);
        s.snapshot.repository_intelligence_seed_count = Some(seed_count as u64);
        s.snapshot.repository_intelligence_context_used = Some(injected);
        if !injected {
            s.snapshot.repository_intelligence_context_bytes = Some(0);
        }
    });
    persist_snapshot();
}

pub fn repository_intelligence_context(bytes: u64, nodes: usize, edges: usize) {
    with_state(|s| {
        s.snapshot.repository_intelligence_attempted = Some(true);
        s.snapshot.repository_intelligence_context_used = Some(true);
        s.snapshot.repository_intelligence_context_bytes = Some(
            s.snapshot
                .repository_intelligence_context_bytes
                .unwrap_or(0)
                .saturating_add(bytes),
        );
        s.snapshot.repository_intelligence_nodes_used = s
            .snapshot
            .repository_intelligence_nodes_used
            .saturating_add(nodes as u64);
        s.snapshot.repository_intelligence_edges_used = s
            .snapshot
            .repository_intelligence_edges_used
            .saturating_add(edges as u64);
        s.snapshot.impact_query_count = s.snapshot.impact_query_count.saturating_add(1);
    });
    persist_snapshot();
}

fn add_usage(slot: &mut Option<u64>, value: u64) {
    if value > 0 {
        *slot = Some(slot.unwrap_or(0).saturating_add(value));
    }
}

fn bounded_diagnostic(value: &str) -> String {
    value.chars().take(8_000).collect()
}

fn classification_for_status(status: &str) -> &'static str {
    match status {
        "pass" => "success",
        "fail" => "candidate_failure",
        "timeout" => "timeout",
        "blocked" | "error" | "infrastructure_error" => "infrastructure_failure",
        "skipped" | "unavailable" => "unavailable",
        _ => "malformed",
    }
}

fn check_classification(check: &Value, status: &str) -> (String, bool) {
    let classification = check
        .get("classification")
        .and_then(Value::as_str)
        .unwrap_or_else(|| classification_for_status(status))
        .to_string();
    let code_failure = classification == "candidate_failure";
    (classification, code_failure)
}
pub fn candidate_mutation() {
    with_state(record_candidate_mutation);
    lifecycle_event("candidate_mutated");
}

pub fn lifecycle_event(event: &str) {
    with_state(|state| {
        if state.snapshot.lifecycle_events.len() < 64 {
            state.snapshot.lifecycle_events.push(event.to_string());
        }
    });
    persist_snapshot();
}

fn record_tool_event(state: &mut State, name: &str, input: Option<&str>) {
    *state.snapshot.tool_calls.entry(name.into()).or_default() += 1;
    if state.first_tool_at.is_none() {
        state.first_tool_at = Some(Instant::now());
        state.snapshot.time_to_first_tool_call_ms = Some(state.started.elapsed().as_millis());
    }
    let normalized = name.to_ascii_lowercase();
    if matches!(normalized.as_str(), "read" | "read_file" | "readfile") {
        state.snapshot.total_file_reads += 1;
        let identity = input
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
            .and_then(|value| {
                value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .map(|path| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                std::hash::Hash::hash(&path.replace('\\', "/"), &mut hasher);
                std::hash::Hasher::finish(&hasher)
            });
        if let Some(identity) = identity {
            state.read_identities.insert(identity);
            state.snapshot.unique_files_read = state.read_identities.len() as u64;
            state.snapshot.repeated_file_reads = state
                .snapshot
                .total_file_reads
                .saturating_sub(state.snapshot.unique_files_read);
        }
    } else if normalized == "grep" {
        state.snapshot.grep_calls += 1;
    } else if normalized == "contextsearch" || normalized == "context_search" {
        state.snapshot.context_search_calls += 1;
    }
}

fn record_candidate_mutation(state: &mut State) {
    state.snapshot.candidate_mutations += 1;
    if state.first_mutation_recorded {
        return;
    }
    state.first_mutation_recorded = true;
    state.snapshot.time_to_first_candidate_mutation_ms = Some(state.started.elapsed().as_millis());
    state.snapshot.model_turns_before_first_candidate_mutation = Some(state.snapshot.model_turns);
    state.snapshot.tool_calls_before_first_candidate_mutation =
        Some(state.snapshot.tool_calls.values().copied().sum());
    state.snapshot.file_reads_before_first_candidate_mutation =
        Some(state.snapshot.total_file_reads);
    state
        .snapshot
        .unique_files_read_before_first_candidate_mutation = Some(state.snapshot.unique_files_read);
    state
        .snapshot
        .repeated_file_reads_before_first_candidate_mutation =
        Some(state.snapshot.repeated_file_reads);
    state.snapshot.grep_calls_before_first_candidate_mutation = Some(state.snapshot.grep_calls);
    state
        .snapshot
        .context_search_calls_before_first_candidate_mutation =
        Some(state.snapshot.context_search_calls);
    state.snapshot.input_tokens_before_first_candidate_mutation = state.snapshot.input_tokens;
    state.snapshot.output_tokens_before_first_candidate_mutation = state.snapshot.output_tokens;
}
pub fn validation(result: &str) {
    with_state(|s| {
        s.snapshot.validation_attempts += 1;
        s.snapshot.validation_result = Some(result.into());
    });
    persist_snapshot();
}

pub fn validation_details(
    candidate_identity: &str,
    validation_identity: &str,
    checks: Vec<ValidationDiagnostic>,
) {
    with_state(|s| {
        s.snapshot.validation_candidate_identity = Some(candidate_identity.to_string());
        s.snapshot.validation_identity = Some(validation_identity.to_string());
        s.snapshot.validation_checks.clone_from(&checks);
        s.snapshot.validation_history.push(ValidationAttempt {
            candidate_identity: candidate_identity.to_string(),
            validation_identity: validation_identity.to_string(),
            checks,
        });
    });
    persist_snapshot();
}

pub fn validation_repair_cycle() {
    with_state(|s| {
        s.snapshot.rework_cycles += 1;
        s.snapshot.validation_repair_cycles += 1;
    });
    persist_snapshot();
}

pub fn evaluator_rework_cycle() {
    with_state(|s| {
        s.snapshot.rework_cycles += 1;
        s.snapshot.evaluator_rework_cycles += 1;
    });
    persist_snapshot();
}

pub fn evaluator_routing(
    selected_profile: Option<&str>,
    reason: &str,
    rejections: Vec<RoutingRejection>,
) {
    with_state(|s| {
        s.snapshot.evaluator_selected_profile = selected_profile.map(str::to_string);
        s.snapshot.evaluator_route_reason = Some(reason.to_string());
        s.snapshot.evaluator_route_rejections = rejections;
    });
    persist_snapshot();
}

pub fn writer_routing(
    selected_profile: Option<&str>,
    reason: &str,
    rejections: Vec<RoutingRejection>,
    estimate: RoutingEstimate,
) {
    with_state(|s| {
        s.snapshot.writer_selected_profile = selected_profile.map(str::to_string);
        s.snapshot.writer_route_reason = Some(reason.to_string());
        s.snapshot.writer_route_rejections = rejections;
        s.snapshot.writer_route_estimate = Some(estimate);
    });
    persist_snapshot();
}

pub fn writer_profile_event(mut event: WriterProfileEvent) {
    with_state(|s| {
        event.sequence = s.snapshot.writer_profile_events.len() as u64 + 1;
        event.timestamp_ms = now_ms();
        event.work_unit.clone_from(&s.snapshot.current_work_unit);
        event.reason = bounded_diagnostic(&event.reason);
        s.snapshot.writer_profile_events.push(event);
        if s.snapshot.writer_profile_events.len() > 128 {
            let excess = s.snapshot.writer_profile_events.len() - 128;
            s.snapshot.writer_profile_events.drain(0..excess);
        }
    });
    lifecycle_event("writer_profile_event_recorded");
}

pub fn planning_state(plan: &crate::task_plan::TaskPlan) {
    let value = planning_artifact_value(plan);
    with_state(|s| s.snapshot.planning_artifact = Some(value));
    persist_snapshot();
}

fn planning_artifact_value(plan: &crate::task_plan::TaskPlan) -> Value {
    let mut value =
        serde_json::to_value(plan).unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    if let Some(object) = value.as_object_mut() {
        if let Some(request) = object
            .get_mut("full_request")
            .and_then(|value| value.as_str())
        {
            const MAX_REQUEST_CHARS: usize = 16_384;
            if request.chars().count() > MAX_REQUEST_CHARS {
                let bounded = request.chars().take(MAX_REQUEST_CHARS).collect::<String>();
                object.insert("full_request".to_string(), Value::String(bounded));
                object.insert("full_request_truncated".to_string(), Value::Bool(true));
            }
        }
    }
    value
}

pub fn writer_packet(event: WriterPacketEvent) {
    with_state(|s| {
        let mut event = event;
        event.sequence = s.snapshot.writer_packet_events.len() as u64 + 1;
        event.timestamp_ms = now_ms();
        s.snapshot.writer_packet_events.push(event);
        if s.snapshot.writer_packet_events.len() > 64 {
            let excess = s.snapshot.writer_packet_events.len() - 64;
            s.snapshot.writer_packet_events.drain(0..excess);
        }
    });
    persist_snapshot();
}

pub fn blocked_checkpoint(event: BlockedCheckpointEvent) {
    with_state(|s| {
        record_blocked_checkpoint_event(s, event);
    });
    persist_snapshot();
}

fn record_blocked_checkpoint_event(state: &mut State, mut event: BlockedCheckpointEvent) {
    event.reason = bounded_diagnostic(&event.reason);
    update_checkpoint_from_blocked_event(state, &event);
    if let Some(existing) = state
        .snapshot
        .blocked_checkpoint_events
        .iter_mut()
        .rev()
        .find(|existing| {
            existing.work_unit == event.work_unit
                && existing.category == event.category
                && existing.reason == event.reason
        })
    {
        let sequence = existing.sequence;
        let timestamp_ms = existing.timestamp_ms;
        *existing = event;
        existing.sequence = sequence;
        existing.timestamp_ms = timestamp_ms;
        return;
    }
    event.sequence = state.snapshot.blocked_checkpoint_events.len() as u64 + 1;
    event.timestamp_ms = now_ms();
    state.snapshot.blocked_checkpoint_events.push(event);
    if state.snapshot.blocked_checkpoint_events.len() > 64 {
        let excess = state.snapshot.blocked_checkpoint_events.len() - 64;
        state.snapshot.blocked_checkpoint_events.drain(0..excess);
    }
}

fn update_checkpoint_from_blocked_event(state: &mut State, event: &BlockedCheckpointEvent) {
    if let Some(record) = state.snapshot.work_unit_checkpoints.last_mut() {
        record.candidate_identity =
            (!event.candidate_identity.is_empty()).then(|| event.candidate_identity.clone());
        record.writer_turns = event.writer_turns;
        record.productive_writer_turns = event.productive_writer_turns;
        record.turn_allowance = event.turn_allowance;
        record.checkpoint_turn = event.checkpoint_turn;
        record.remaining_turns = event.turns_remaining;
        record.continuation_grants = event.continuation_grants;
        record.blocker_category = Some(event.category.clone());
        record.blocker_reason = Some(event.reason.clone());
        record
            .declared_contract_ids
            .clone_from(&event.declared_contract_ids);
        record.continuation_eligible = Some(event.continuation_eligible);
        record
            .candidate_check_evidence_ids
            .clone_from(&event.latest_evidence_ids);
    }
}

pub fn candidate_artifact(candidate_identity: &str, changed_paths: &[String], diff: &str) {
    const MAX_DIFF_CHARS: usize = 32_000;
    let bounded_diff = diff.chars().take(MAX_DIFF_CHARS).collect::<String>();
    with_state(|s| {
        s.snapshot.candidate_artifact = Some(CandidateArtifact {
            candidate_identity: candidate_identity.to_string(),
            changed_paths: changed_paths.iter().take(128).cloned().collect(),
            truncated: bounded_diff.len() < diff.len(),
            diff: bounded_diff,
        });
    });
    persist_snapshot();
}

pub fn evaluation_blocked(reason: &str) {
    with_state(|s| {
        s.snapshot.evaluation_blocked_reason = Some(reason.chars().take(1_000).collect());
    });
    lifecycle_event("evaluation_blocked");
    persist_snapshot();
}

pub fn requirement_coverage(coverage: Vec<RequirementCoverage>) {
    with_state(|s| {
        s.snapshot.requirement_coverage = coverage;
    });
    persist_snapshot();
}

pub fn flush(status: &str) {
    let Some(lock) = STATE.get() else { return };
    let Ok(mut guard) = lock.lock() else { return };
    let Some(mut state) = guard.take() else {
        return;
    };
    state.snapshot.elapsed_ms = state.started.elapsed().as_millis();
    state.snapshot.terminal_status = status.into();
    write_snapshot(&state.path, &state.snapshot);
}

fn persist_snapshot() {
    with_state(|state| write_snapshot(&state.path, &state.snapshot));
}

fn write_snapshot(path: &PathBuf, snapshot: &Snapshot) {
    let mut persisted = snapshot.clone();
    if persisted.terminal_status.is_empty() {
        persisted.terminal_status = "in_progress".into();
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&persisted) else {
        return;
    };
    let temporary = path.with_extension("tmp");
    if fs::write(&temporary, bytes).is_ok() {
        let _ = fs::rename(temporary, path);
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            path: PathBuf::from("/tmp/telemetry-test.json"),
            started: Instant::now(),
            snapshot: Snapshot {
                schema_version: 1,
                ..Snapshot::default()
            },
            read_identities: HashSet::new(),
            first_tool_at: None,
            first_mutation_recorded: false,
            provider_context: ProviderContext::default(),
            active_provider_record: None,
        }
    }

    #[test]
    fn usage_and_request_bytes_accumulate_without_fabricating_zeroes() {
        let mut state = state();
        add_usage(&mut state.snapshot.input_tokens, 10);
        add_usage(&mut state.snapshot.input_tokens, 20);
        add_usage(&mut state.snapshot.output_tokens, 7);
        state.snapshot.model_request_bytes += 100;
        state.snapshot.model_request_bytes += 50;
        assert_eq!(state.snapshot.input_tokens, Some(30));
        assert_eq!(state.snapshot.output_tokens, Some(7));
        assert_eq!(state.snapshot.cache_read_tokens, None);
        assert_eq!(state.snapshot.model_request_bytes, 150);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn provider_record_uses_profile_rates_for_authoritative_usage() {
        let mut state = state();
        state.provider_context = ProviderContext {
            role: Some("writer".into()),
            profile: Some("gpt-5.4-mini".into()),
            provider: Some("openai".into()),
            protocol: Some("responses".into()),
            input_rate: Some(0.75),
            output_rate: Some(4.50),
            price_source: Some("explicit_profile".into()),
            ..ProviderContext::default()
        };
        state.snapshot.provider_calls = 1;
        state
            .snapshot
            .provider_call_records
            .push(ProviderCallRecord {
                sequence: 1,
                role: state.provider_context.role.clone(),
                profile: state.provider_context.profile.clone(),
                provider: state.provider_context.provider.clone(),
                protocol: state.provider_context.protocol.clone(),
                price_source: state.provider_context.price_source.clone(),
                ..ProviderCallRecord::default()
            });
        state.active_provider_record = Some(0);
        let input_rate = state.provider_context.input_rate.unwrap();
        let output_rate = state.provider_context.output_rate.unwrap();
        let record = &mut state.snapshot.provider_call_records[0];
        record.input_tokens = 1_000_000;
        record.output_tokens = 1_000_000;
        record.estimated_cost_usd = Some(
            (record.input_tokens as f64 * input_rate + record.output_tokens as f64 * output_rate)
                / 1_000_000.0,
        );
        assert_eq!(record.profile.as_deref(), Some("gpt-5.4-mini"));
        assert_eq!(record.price_source.as_deref(), Some("explicit_profile"));
        assert_eq!(record.estimated_cost_usd, Some(5.25));
    }

    #[test]
    fn read_and_exploration_counts_track_unique_and_repeated_events() {
        let mut state = state();
        record_tool_event(&mut state, "read_file", Some(r#"{"path":"a.rs"}"#));
        record_tool_event(&mut state, "read_file", Some(r#"{"path":"b.rs"}"#));
        record_tool_event(&mut state, "read_file", Some(r#"{"path":"a.rs"}"#));
        record_tool_event(&mut state, "grep", None);
        record_tool_event(&mut state, "ContextSearch", None);
        assert_eq!(state.snapshot.total_file_reads, 3);
        assert_eq!(state.snapshot.unique_files_read, 2);
        assert_eq!(state.snapshot.repeated_file_reads, 1);
        assert_eq!(state.snapshot.grep_calls, 1);
        assert_eq!(state.snapshot.context_search_calls, 1);
    }

    #[test]
    fn treatment_fields_distinguish_unreached_from_definitive_non_match() {
        let mut state = state();
        assert_eq!(state.snapshot.repository_intelligence_attempted, None);
        assert_eq!(state.snapshot.repository_intelligence_context_used, None);
        state.snapshot.repository_intelligence_attempted = Some(true);
        state.snapshot.repository_intelligence_seed_count = Some(0);
        state.snapshot.repository_intelligence_context_used = Some(false);
        state.snapshot.repository_intelligence_context_bytes = Some(0);
        assert_eq!(
            state.snapshot.repository_intelligence_context_used,
            Some(false)
        );
        assert_eq!(
            state.snapshot.repository_intelligence_context_bytes,
            Some(0)
        );
    }

    #[test]
    fn graph_context_switch_is_off_only_when_explicitly_disabled() {
        assert!(graph_context_enabled_value(None));
        assert!(graph_context_enabled_value(Some("on")));
        assert!(!graph_context_enabled_value(Some("off")));
    }

    #[test]
    fn first_mutation_snapshots_only_prior_activity() {
        let mut state = state();
        state.snapshot.model_turns = 2;
        record_tool_event(&mut state, "read_file", Some(r#"{"path":"a.rs"}"#));
        record_candidate_mutation(&mut state);
        state.snapshot.model_turns = 3;
        record_tool_event(&mut state, "read_file", Some(r#"{"path":"b.rs"}"#));
        assert_eq!(
            state.snapshot.model_turns_before_first_candidate_mutation,
            Some(2)
        );
        assert_eq!(
            state.snapshot.file_reads_before_first_candidate_mutation,
            Some(1)
        );
        assert_eq!(
            state
                .snapshot
                .unique_files_read_before_first_candidate_mutation,
            Some(1)
        );
        assert_eq!(
            state.snapshot.tool_calls_before_first_candidate_mutation,
            Some(1)
        );
        assert_eq!(
            state.snapshot.file_reads_before_first_candidate_mutation,
            Some(1)
        );
        assert!(state.snapshot.time_to_first_candidate_mutation_ms.is_some());
    }

    #[test]
    fn per_check_status_does_not_inherit_mixed_aggregate_classification() {
        assert_eq!(classification_for_status("pass"), "success");
        assert_eq!(classification_for_status("fail"), "candidate_failure");
        assert_eq!(classification_for_status("timeout"), "timeout");
        assert_eq!(
            classification_for_status("infrastructure_error"),
            "infrastructure_failure"
        );
        assert_eq!(classification_for_status("skipped"), "unavailable");

        let formatter = serde_json::json!({"status":"pass"});
        let tests = serde_json::json!({"status":"fail"});
        let clippy = serde_json::json!({"status":"fail"});
        assert_eq!(
            check_classification(&formatter, "pass"),
            ("success".to_string(), false)
        );
        assert_eq!(
            check_classification(&tests, "fail"),
            ("candidate_failure".to_string(), true)
        );
        assert_eq!(
            check_classification(&clippy, "fail"),
            ("candidate_failure".to_string(), true)
        );
    }

    #[test]
    fn checkpoint_telemetry_distinguishes_allowance_from_checkpoint_turn() {
        let mut state = state();
        state.snapshot.work_unit_writer_turns = 11;
        state.snapshot.work_unit_turn_allowance = 10;
        let record = WorkUnitCheckpoint {
            writer_turns: state.snapshot.work_unit_writer_turns,
            turn_allowance: state.snapshot.work_unit_turn_allowance,
            productive_writer_turns: state
                .snapshot
                .work_unit_writer_turns
                .min(state.snapshot.work_unit_turn_allowance),
            checkpoint_turn: state.snapshot.work_unit_writer_turns
                > state.snapshot.work_unit_turn_allowance,
            ..WorkUnitCheckpoint::default()
        };

        assert_eq!(record.productive_writer_turns, 10);
        assert!(record.checkpoint_turn);
        assert_eq!(record.writer_turns, 11);
    }

    #[test]
    fn planning_artifact_contains_contracts_and_work_units() {
        let plan = crate::task_plan::TaskPlan::from_request(
            "Validate merged configuration and render safe diagnostics.",
            Some("doctor command; configuration loader; JSON output"),
        );
        let value = planning_artifact_value(&plan);

        assert!(value.get("full_request").is_some());
        assert!(value.get("contracts").and_then(Value::as_array).is_some());
        assert!(value.get("work_units").and_then(Value::as_array).is_some());
        assert!(value.get("repository_files").is_some());
    }

    #[test]
    fn packet_and_blocked_checkpoint_records_preserve_safe_semantic_identity() {
        let packet = WriterPacketEvent {
            task_id: "task-1".to_string(),
            work_unit: Some("WU-1".to_string()),
            profile: "profile-a".to_string(),
            model: "opaque-model".to_string(),
            protocol: Some("responses".to_string()),
            reasoning_policy: "omitted_provider_default".to_string(),
            contract_ids: vec!["contract-1".to_string()],
            owned_contract_ids: vec!["contract-1".to_string()],
            downstream_contract_ids: vec!["contract-2".to_string()],
            global_invariant_ids: vec!["contract-3".to_string()],
            repository_fact_ids: vec!["fact-1".to_string()],
            candidate_identity: "candidate-1".to_string(),
            packet_hash: "packet-hash".to_string(),
            ..WriterPacketEvent::default()
        };
        let blocked = BlockedCheckpointEvent {
            work_unit: Some("WU-1".to_string()),
            writer_turns: 12,
            productive_writer_turns: 10,
            turn_allowance: 10,
            checkpoint_turn: true,
            turns_remaining: 0,
            continuation_grants: 1,
            candidate_identity: "candidate-1".to_string(),
            category: "writer_reported_blocked".to_string(),
            reason: "missing implementation surface".to_string(),
            declared_contract_ids: vec!["contract-1".to_string()],
            owned_contract_ids: vec!["contract-1".to_string()],
            downstream_contract_ids: vec!["contract-2".to_string()],
            global_invariant_ids: vec!["contract-3".to_string()],
            latest_evidence_ids: vec![7],
            terminal_reason: "writer_checkpoint_blocked".to_string(),
            ..BlockedCheckpointEvent::default()
        };

        let packet_json = serde_json::to_value(packet).expect("packet telemetry should serialize");
        let blocked_json =
            serde_json::to_value(blocked).expect("blocked telemetry should serialize");
        assert_eq!(packet_json["packet_hash"], "packet-hash");
        assert_eq!(packet_json["protocol"], "responses");
        assert_eq!(packet_json["reasoning_policy"], "omitted_provider_default");
        assert_eq!(packet_json["owned_contract_ids"][0], "contract-1");
        assert_eq!(packet_json["downstream_contract_ids"][0], "contract-2");
        assert_eq!(blocked_json["category"], "writer_reported_blocked");
        assert_eq!(blocked_json["declared_contract_ids"][0], "contract-1");
        assert_eq!(blocked_json["productive_writer_turns"], 10);
        assert_eq!(blocked_json["latest_evidence_ids"][0], 7);
    }
}
