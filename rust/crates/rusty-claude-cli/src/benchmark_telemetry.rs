use serde::Serialize;
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
    pub evaluator_selected_profile: Option<String>,
    pub evaluator_route_reason: Option<String>,
    pub evaluator_route_rejections: Vec<RoutingRejection>,
    pub evaluation_blocked_reason: Option<String>,
    pub started_at_ms: u128,
    pub elapsed_ms: u128,
    pub terminal_status: String,
    pub lifecycle_events: Vec<String>,
    pub provider_call_records: Vec<ProviderCallRecord>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ProviderCallRecord {
    pub sequence: u64,
    pub role: Option<String>,
    pub profile: Option<String>,
    pub provider: Option<String>,
    pub protocol: Option<String>,
    pub request_ids: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub estimated_cost_usd: Option<f64>,
    pub price_source: Option<String>,
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
pub struct RoutingRejection {
    pub profile_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
struct ProviderContext {
    role: Option<String>,
    profile: Option<String>,
    provider: Option<String>,
    protocol: Option<String>,
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
        s.snapshot.provider_call_records.push(ProviderCallRecord {
            sequence,
            role: context.role.clone(),
            profile: context.profile.clone(),
            provider: context.provider.clone(),
            protocol: context.protocol.clone(),
            price_source: context.price_source.clone(),
            ..ProviderCallRecord::default()
        });
        s.active_provider_record = Some(s.snapshot.provider_call_records.len() - 1);
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
        };
    });
}

pub fn set_provider_protocol(protocol: &str) {
    with_state(|s| {
        s.provider_context.protocol = Some(protocol.to_string());
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

pub fn rework_cycle() {
    with_state(|s| s.snapshot.rework_cycles += 1);
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

pub fn evaluation_blocked(reason: &str) {
    with_state(|s| {
        s.snapshot.evaluation_blocked_reason = Some(reason.chars().take(1_000).collect());
    });
    lifecycle_event("evaluation_blocked");
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
}
