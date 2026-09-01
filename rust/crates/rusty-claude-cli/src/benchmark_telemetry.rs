use serde::Serialize;
use std::collections::BTreeMap;
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
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
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
    with_state(|s| *s.snapshot.tool_calls.entry(name.into()).or_default() += 1);
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
        s.snapshot.input_tokens += input;
        s.snapshot.output_tokens += output;
        s.snapshot.cache_read_tokens += cache_read;
        s.snapshot.cache_write_tokens += cache_write;
    });
}
pub fn candidate_mutation() {
    with_state(|s| s.snapshot.candidate_mutations += 1);
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
