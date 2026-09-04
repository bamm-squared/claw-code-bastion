#!/usr/bin/env python3
"""Deterministic local Responses provider for benchmark fixtures.

This server only returns model responses. Candidate and canonical files are
changed by the normal Claw tool executor, never by this process.
"""

import argparse
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path


CONFIG = """pub const LIMIT: usize = 8;

pub fn accepts(value: usize) -> bool {
    value <= LIMIT
}
"""

TESTS = """use claw_acceptance_fixture::config::{accepts, LIMIT};

#[test]
fn limit_is_eight() {
    assert_eq!(LIMIT, 8);
}

#[test]
fn accepts_values_at_or_below_limit() {
    assert!(accepts(8));
}

#[test]
fn rejects_values_above_limit() {
    assert!(!accepts(9));
}
"""


RETRY_CONFIG = """#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 100,
            max_delay_ms: 500,
        }
    }
}
"""

RETRY = """use crate::config::RetryConfig;

pub fn backoff_delay_ms(config: &RetryConfig, retry_index: u32) -> u64 {
    let multiplier = 1u64.checked_shl(retry_index).unwrap_or(u64::MAX);
    config
        .base_delay_ms
        .saturating_mul(multiplier)
        .min(config.max_delay_ms)
}

pub fn retry_delays(config: &RetryConfig, retry_count: u32) -> Vec<u64> {
    (0..retry_count)
        .map(|index| backoff_delay_ms(config, index))
        .collect()
}
"""

RETRY_BROKEN = """use crate::config::RetryConfig;

pub fn backoff_delay_ms(config: &RetryConfig, retry_index: u32) -> u64 {
    let multiplier = 1u64.checked_shl(retry_index).unwrap_or(u64::MAX);
    config
        .base_delay_ms
        .saturating_mul(multiplier)
        .min(config.max_delay_m)
}

pub fn retry_delays(config: &RetryConfig, retry_count: u32) -> Vec<u64> {
    (0..retry_count)
        .map(|index| backoff_delay_ms(config, index))
        .collect()
}
"""

RETRY_TESTS = """use claw_retry_fixture::{config::RetryConfig, retry};

#[test]
fn default_policy_preserves_attempts_and_adds_a_cap() {
    let config = RetryConfig::default();
    assert_eq!(config.max_attempts, 3);
    assert_eq!(config.max_delay_ms, 500);
    assert_eq!(
        retry::retry_delays(&config, 5),
        vec![100, 200, 400, 500, 500]
    );
}

#[test]
fn custom_policy_caps_exponential_growth() {
    let config = RetryConfig {
        max_attempts: 5,
        base_delay_ms: 100,
        max_delay_ms: 250,
    };
    assert_eq!(retry::retry_delays(&config, 4), vec![100, 200, 250, 250]);
}

#[test]
fn zero_base_delay_remains_zero() {
    let config = RetryConfig {
        max_attempts: 5,
        base_delay_ms: 0,
        max_delay_ms: 250,
    };
    assert_eq!(retry::retry_delays(&config, 4), vec![0, 0, 0, 0]);
}
"""

RETRY_CLIENT_TESTS = """use claw_retry_fixture::{client::Client, config::RetryConfig};

#[test]
fn client_uses_the_capped_policy() {
    let client = Client::new(RetryConfig {
        max_attempts: 5,
        base_delay_ms: 50,
        max_delay_ms: 125,
    });
    assert_eq!(client.retry_delays(4), vec![50, 100, 125, 125]);
}
"""

RETRY_CLIENT_TESTS_UNCAPPED = """use claw_retry_fixture::{client::Client, config::RetryConfig};

#[test]
fn client_uses_configured_retry_delays() {
    let client = Client::new(RetryConfig {
        max_attempts: 5,
        base_delay_ms: 50,
        max_delay_ms: 500,
    });
    assert_eq!(client.retry_delays(3), vec![50, 100, 200]);
}
"""

EVENT_LEDGER = """use std::collections::BTreeSet;

use crate::event::Event;

#[derive(Debug, PartialEq, Eq)]
pub enum LedgerError {
    InsufficientFunds { requested: u64, available: u64 },
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    Duplicate,
}

#[derive(Debug, Default)]
pub struct Ledger {
    balance: u64,
    applied_ids: BTreeSet<u64>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn balance(&self) -> u64 {
        self.balance
    }

    pub fn apply(&mut self, event: Event) -> Result<ApplyOutcome, LedgerError> {
        let event_id = event.id();
        if self.applied_ids.contains(&event_id) {
            return Ok(ApplyOutcome::Duplicate);
        }

        match event {
            Event::Deposit { amount, .. } => {
                self.balance += amount;
            }
            Event::Withdraw { amount, .. } => {
                if amount > self.balance {
                    return Err(LedgerError::InsufficientFunds {
                        requested: amount,
                        available: self.balance,
                    });
                }
                self.balance -= amount;
            }
        }
        self.applied_ids.insert(event_id);
        Ok(ApplyOutcome::Applied)
    }
}
"""

EVENT_LEDGER_TESTS = """use claw_event_ledger_fixture::{
    event::Event,
    ledger::{ApplyOutcome, Ledger, LedgerError},
};

#[test]
fn deposits_and_withdrawals_update_balance() {
    let mut ledger = Ledger::new();
    assert_eq!(
        ledger.apply(Event::Deposit { id: 1, amount: 100 }),
        Ok(ApplyOutcome::Applied)
    );
    assert_eq!(
        ledger.apply(Event::Withdraw { id: 2, amount: 40 }),
        Ok(ApplyOutcome::Applied)
    );
    assert_eq!(ledger.balance(), 60);
}

#[test]
fn duplicate_event_does_not_change_balance() {
    let mut ledger = Ledger::new();
    ledger.apply(Event::Deposit { id: 1, amount: 100 }).unwrap();
    assert_eq!(
        ledger.apply(Event::Deposit { id: 1, amount: 100 }),
        Ok(ApplyOutcome::Duplicate)
    );
    assert_eq!(ledger.balance(), 100);
}

#[test]
fn failed_withdrawal_is_not_recorded() {
    let mut ledger = Ledger::new();
    let error = ledger
        .apply(Event::Withdraw { id: 3, amount: 10 })
        .unwrap_err();
    assert_eq!(
        error,
        LedgerError::InsufficientFunds {
            requested: 10,
            available: 0,
        }
    );
    ledger.apply(Event::Deposit { id: 4, amount: 10 }).unwrap();
    assert_eq!(
        ledger.apply(Event::Withdraw { id: 3, amount: 10 }),
        Ok(ApplyOutcome::Applied)
    );
    assert_eq!(ledger.balance(), 0);
}
"""

API_REQUEST = """#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    EmptyPath,
    UnsupportedMethod,
}

pub fn build_request(method: &str, path: &str) -> Result<Request, RequestError> {
    if path.is_empty() {
        return Err(RequestError::EmptyPath);
    }
    if !matches!(method, "GET" | "POST") {
        return Err(RequestError::UnsupportedMethod);
    }
    Ok(Request {
        method: method.to_string(),
        path: path.to_string(),
    })
}

pub struct RequestBuilder {
    method: String,
    path: String,
    query: Vec<(String, String)>,
}

impl RequestBuilder {
    pub fn new(method: &str, path: &str) -> Result<Self, RequestError> {
        let request = build_request(method, path)?;
        Ok(Self {
            method: request.method,
            path: request.path,
            query: Vec::new(),
        })
    }

    pub fn query_param(mut self, key: &str, value: &str) -> Self {
        self.query.push((key.to_string(), value.to_string()));
        self
    }

    pub fn build(self) -> Result<Request, RequestError> {
        let mut path = self.path;
        if !self.query.is_empty() {
            path.push('?');
            for (index, (key, value)) in self.query.into_iter().enumerate() {
                if index > 0 {
                    path.push('&');
                }
                path.push_str(&encode_component(&key));
                path.push('=');
                path.push_str(&encode_component(&value));
            }
        }
        Ok(Request {
            method: self.method,
            path,
        })
    }
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
"""

API_CLIENT = """use crate::request::{Request, RequestBuilder, RequestError};

#[derive(Default)]
pub struct Client;

impl Client {
    pub fn new() -> Self {
        Self
    }

    pub fn send(&self, request: Request) -> Result<String, RequestError> {
        Ok(format!("{} {}", request.method, request.path))
    }

    pub fn builder(method: &str, path: &str) -> Result<RequestBuilder, RequestError> {
        RequestBuilder::new(method, path)
    }
}
"""

API_TESTS = """use claw_api_compat_fixture::{
    client::Client,
    request::{build_request, RequestError},
};

#[test]
fn builds_a_get_request() {
    let request = build_request("GET", "/health").unwrap();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/health");
}

#[test]
fn legacy_client_sends_request() {
    let request = build_request("POST", "/events").unwrap();
    assert_eq!(
        Client::new().send(request),
        Ok(String::from("POST /events"))
    );
}

#[test]
fn preserves_existing_validation_errors() {
    assert_eq!(build_request("GET", ""), Err(RequestError::EmptyPath));
    assert_eq!(
        build_request("TRACE", "/health"),
        Err(RequestError::UnsupportedMethod)
    );
}

#[test]
fn builder_adds_encoded_query_parameters() {
    let request = Client::builder("GET", "/search")
        .unwrap()
        .query_param("q", "a b")
        .query_param("next", "x&y=z?")
        .build()
        .unwrap();
    assert_eq!(request.path, "/search?q=a%20b&next=x%26y%3Dz%3F");
}

#[test]
fn builder_without_query_matches_legacy_request() {
    let legacy = build_request("POST", "/events").unwrap();
    let built = Client::builder("POST", "/events").unwrap().build().unwrap();
    assert_eq!(built, legacy);
}
"""


def actions_for(
    task,
    repair=False,
    inject_validation_failure=False,
    inject_evaluator_gap=False,
):
    if task == "retry-policy":
        return [
            ("read_file", {"path": "Cargo.toml"}),
            ("read_file", {"path": "src/config.rs"}),
            ("read_file", {"path": "src/retry.rs"}),
            ("read_file", {"path": "src/client.rs"}),
            ("read_file", {"path": "tests/retry.rs"}),
            ("read_file", {"path": "tests/client.rs"}),
            ("write_file", {"path": "src/config.rs", "content": RETRY_CONFIG}),
            (
                "write_file",
                {
                    "path": "src/retry.rs",
                    "content": RETRY_BROKEN
                    if inject_validation_failure and not repair
                    else RETRY,
                },
            ),
            ("write_file", {"path": "tests/retry.rs", "content": RETRY_TESTS}),
            (
                "write_file",
                {
                    "path": "tests/client.rs",
                    "content": RETRY_CLIENT_TESTS_UNCAPPED
                    if inject_evaluator_gap and not repair
                    else RETRY_CLIENT_TESTS,
                },
            ),
        ]
    if task == "event-ledger":
        return [
            ("read_file", {"path": "Cargo.toml"}),
            ("read_file", {"path": "src/event.rs"}),
            ("read_file", {"path": "src/ledger.rs"}),
            ("read_file", {"path": "tests/ledger.rs"}),
            ("write_file", {"path": "src/ledger.rs", "content": EVENT_LEDGER}),
            ("write_file", {"path": "tests/ledger.rs", "content": EVENT_LEDGER_TESTS}),
        ]
    if task == "api-compat":
        return [
            ("read_file", {"path": "Cargo.toml"}),
            ("read_file", {"path": "src/lib.rs"}),
            ("read_file", {"path": "src/request.rs"}),
            ("read_file", {"path": "src/client.rs"}),
            ("read_file", {"path": "tests/request.rs"}),
            ("write_file", {"path": "src/request.rs", "content": API_REQUEST}),
            ("write_file", {"path": "src/client.rs", "content": API_CLIENT}),
            ("write_file", {"path": "tests/request.rs", "content": API_TESTS}),
        ]
    return [
        ("read_file", {"path": "src/config.rs"}),
        ("read_file", {"path": "tests/config.rs"}),
        ("write_file", {"path": "src/config.rs", "content": CONFIG}),
        ("write_file", {"path": "tests/config.rs", "content": TESTS}),
        ("bash", {"command": "cargo test"}),
        ("bash", {"command": "cargo test"}),
    ]


def tool_for(
    index,
    task,
    repair=False,
    inject_validation_failure=False,
    inject_evaluator_gap=False,
):
    return actions_for(
        task, repair, inject_validation_failure, inject_evaluator_gap
    )[index]


def response(
    number,
    stream,
    task,
    rework_test=False,
    evaluator_rework_test=False,
    evaluator_unavailable=False,
    is_evaluator=False,
    evaluator_call=0,
    writer_index=0,
):
    response_id = f"local-{number}"
    if number <= 3:
        finding = json.dumps(
            {
                "subject": "fixture",
                "claim": "The fixture contains the requested Rust behavior.",
                "evidence": "local fixture",
            }
        )
        return {
            "id": response_id,
            "object": "response",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": finding}],
                }
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5},
        }
    if is_evaluator:
        if evaluator_unavailable:
            evaluation = "The evaluator is unavailable for this deterministic regression."
        elif (rework_test or evaluator_rework_test) and evaluator_call == 1:
            if evaluator_rework_test:
                finding = "Client integration does not verify the configured delay ceiling."
                evidence = "tests/client.rs only exercises an uncapped retry schedule."
            else:
                finding = "Cross-module validation failed; repair using the trusted diagnostic."
                evidence = "The candidate did not pass trusted validation."
            evaluation = json.dumps(
                {
                    "requirements": [
                        {
                            "requirement_id": "task",
                            "state": "gap_found",
                            "finding": finding,
                            "evidence": evidence,
                            "confidence": "high",
                            "rework_recommended": True,
                        }
                    ]
                }
            )
        else:
            evaluation = json.dumps(
                {
                    "requirements": [
                        {
                            "requirement_id": "task",
                            "state": "satisfied",
                            "finding": "All requested behavior is present.",
                            "evidence": "Trusted validation passed.",
                            "confidence": "high",
                            "rework_recommended": False,
                        }
                    ]
                }
            )
        return {
            "id": response_id,
            "object": "response",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": evaluation}],
                }
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5},
        }

    repair = (rework_test or evaluator_rework_test) and evaluator_call > 0
    inject_validation_failure = rework_test and evaluator_call == 0
    inject_evaluator_gap = evaluator_rework_test and evaluator_call == 0
    actions = actions_for(
        task, repair, inject_validation_failure, inject_evaluator_gap
    )
    if writer_index < len(actions):
        name, arguments = tool_for(
            writer_index,
            task,
            repair,
            inject_validation_failure,
            inject_evaluator_gap,
        )
        item_id = f"item-{number}"
        call_id = f"call-{number}"
        encoded = json.dumps(arguments, separators=(",", ":"))
        return [
            {
                "type": "response.output_item.added",
                "response": {"id": response_id},
                "item": {
                    "type": "function_call",
                    "id": item_id,
                    "call_id": call_id,
                    "name": name,
                    "arguments": "",
                },
            },
            {
                "type": "response.function_call_arguments.delta",
                "response": {"id": response_id},
                "item_id": item_id,
                "delta": encoded,
            },
            {
                "type": "response.function_call_arguments.done",
                "response": {"id": response_id},
                "item_id": item_id,
                "arguments": encoded,
            },
            {
                "type": "response.output_item.done",
                "response": {"id": response_id},
                "item": {
                    "type": "function_call",
                    "id": item_id,
                    "call_id": call_id,
                    "name": name,
                    "arguments": encoded,
                },
            },
            {
                "type": "response.completed",
                "response": {
                    "id": response_id,
                    "status": "completed",
                    "usage": {"input_tokens": 10, "output_tokens": 5},
                },
            },
        ]

    return [
        {
            "type": "response.output_text.delta",
            "response": {"id": response_id},
            "delta": "Implemented the requested change.",
        },
        {
            "type": "response.completed",
            "response": {
                "id": response_id,
                "status": "completed",
                "usage": {"input_tokens": 10, "output_tokens": 5},
            },
        },
    ]


class ResponsesHandler(BaseHTTPRequestHandler):
    request_number = 0
    evaluator_calls = 0
    writer_calls_since_evaluator = 0
    task = "config-threading"
    rework_test = False
    evaluator_rework_test = False
    evaluator_unavailable = False

    def log_message(self, format_string, *args):
        print(f"fake-responses request={self.request_number} path={self.path}", flush=True)

    def do_POST(self):
        ResponsesHandler.request_number += 1
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length))
        stream = bool(request.get("stream", True))
        is_evaluator = "Independent Requirement Evaluation" in json.dumps(request)
        if is_evaluator:
            ResponsesHandler.evaluator_calls += 1
            ResponsesHandler.writer_calls_since_evaluator = 0
            evaluator_call = ResponsesHandler.evaluator_calls
            writer_index = 0
        elif ResponsesHandler.request_number > 3:
            evaluator_call = ResponsesHandler.evaluator_calls
            writer_index = ResponsesHandler.writer_calls_since_evaluator
            ResponsesHandler.writer_calls_since_evaluator += 1
        else:
            evaluator_call = ResponsesHandler.evaluator_calls
            writer_index = 0
        payload = response(
            ResponsesHandler.request_number,
            stream,
            ResponsesHandler.task,
            ResponsesHandler.rework_test,
            ResponsesHandler.evaluator_rework_test,
            ResponsesHandler.evaluator_unavailable,
            is_evaluator,
            evaluator_call,
            writer_index,
        )
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream" if stream else "application/json")
        self.send_header("x-request-id", f"local-req-{ResponsesHandler.request_number}")
        if stream:
            body = b"".join(
                b"data: " + json.dumps(event).encode() + b"\n\n" for event in payload
            )
        else:
            body = json.dumps(payload).encode()
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=18766)
    parser.add_argument("--port-file", type=Path)
    parser.add_argument("--ready-file", type=Path)
    parser.add_argument(
        "--task", choices=["config-threading", "retry-policy", "event-ledger", "api-compat"], default="config-threading"
    )
    parser.add_argument(
        "--rework-test",
        action="store_true",
        help="fail the first retry-policy candidate, then provide a repair sequence",
    )
    parser.add_argument(
        "--evaluator-unavailable",
        action="store_true",
        help="return an invalid evaluator response so Review must fail closed",
    )
    parser.add_argument(
        "--evaluator-rework-test",
        action="store_true",
        help="start with valid code but incomplete Client coverage, then provide evaluator rework",
    )
    args = parser.parse_args()
    ResponsesHandler.task = args.task
    ResponsesHandler.rework_test = args.rework_test
    ResponsesHandler.evaluator_rework_test = args.evaluator_rework_test
    ResponsesHandler.evaluator_unavailable = args.evaluator_unavailable
    server = HTTPServer((args.host, args.port), ResponsesHandler)
    if args.port_file:
        args.port_file.write_text(str(server.server_address[1]) + "\n")
    if args.ready_file:
        args.ready_file.touch()
    server.serve_forever()


if __name__ == "__main__":
    main()
