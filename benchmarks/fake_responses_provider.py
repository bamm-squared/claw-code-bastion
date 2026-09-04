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
        "--task", choices=["config-threading", "retry-policy"], default="config-threading"
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
