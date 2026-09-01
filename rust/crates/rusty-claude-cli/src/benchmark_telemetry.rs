use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Serialize, Default)]
pub struct Snapshot {
    pub schema_version: u32,
    pub run_id: String,
    pub provider_calls: u64,
    pub model_turns: u64,
    pub tool_bearing_turns: u64,
    pub tool_calls: BTreeMap<String, u64>,
    pub model_request_bytes: u64,
    pub repository_intelligence_context_used: bool,
    pub repository_intelligence_context_bytes: u64,
    pub repository_intelligence_nodes_used: u64,
    pub repository_intelligence_edges_used: u64,
    pub impact_query_count: u64,
    pub total_file_reads: u64,
    pub unique_files_read: u64,
    pub repeated_file_reads: u64,
    pub grep_calls: u64,
    pub context_search_calls: u64,
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
    pub started_at_ms: u128,
    pub elapsed_ms: u128,
    pub terminal_status: String,
}

struct State {
    path: PathBuf,
    started: Instant,
    snapshot: Snapshot,
    read_identities: HashSet<u64>,
    first_tool_at: Option<Instant>,
    first_mutation_recorded: bool,
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
            ..Snapshot::default()
        },
        read_identities: HashSet::new(),
        first_tool_at: None,
        first_mutation_recorded: false,
    })));
}

fn with_state(f: impl FnOnce(&mut State)) {
    let Some(lock) = STATE.get() else { return };
    let Ok(mut guard) = lock.lock() else { return };
    if let Some(state) = guard.as_mut() {
        f(state);
    }
}

pub fn provider_call() {
    with_state(|s| s.snapshot.provider_calls += 1);
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
pub fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) {
    with_state(|s| {
        add_usage(&mut s.snapshot.input_tokens, input);
        add_usage(&mut s.snapshot.output_tokens, output);
        add_usage(&mut s.snapshot.cache_read_tokens, cache_read);
        add_usage(&mut s.snapshot.cache_write_tokens, cache_write);
    });
}

pub fn model_request_bytes(bytes: u64) {
    with_state(|s| {
        s.snapshot.model_request_bytes = s.snapshot.model_request_bytes.saturating_add(bytes);
    });
}

pub fn repository_intelligence_context(bytes: u64, nodes: usize, edges: usize) {
    with_state(|s| {
        s.snapshot.repository_intelligence_context_used = true;
        s.snapshot.repository_intelligence_context_bytes = s
            .snapshot
            .repository_intelligence_context_bytes
            .saturating_add(bytes);
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
}

fn add_usage(slot: &mut Option<u64>, value: u64) {
    if value > 0 {
        *slot = Some(slot.unwrap_or(0).saturating_add(value));
    }
}
pub fn candidate_mutation() {
    with_state(record_candidate_mutation);
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
}

pub fn flush(status: &str) {
    let Some(lock) = STATE.get() else { return };
    let Ok(mut guard) = lock.lock() else { return };
    let Some(mut state) = guard.take() else {
        return;
    };
    state.snapshot.elapsed_ms = state.started.elapsed().as_millis();
    state.snapshot.terminal_status = status.into();
    let Ok(bytes) = serde_json::to_vec_pretty(&state.snapshot) else {
        return;
    };
    let temporary = state.path.with_extension("tmp");
    if fs::write(&temporary, bytes).is_ok() {
        let _ = fs::rename(temporary, state.path);
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
