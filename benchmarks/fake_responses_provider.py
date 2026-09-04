#!/usr/bin/env python3
"""Deterministic local Responses provider for the config-threading task.

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


def tool_for(index):
    return [
        ("read_file", {"path": "src/config.rs"}),
        ("read_file", {"path": "tests/config.rs"}),
        ("write_file", {"path": "src/config.rs", "content": CONFIG}),
        ("write_file", {"path": "tests/config.rs", "content": TESTS}),
        ("bash", {"command": "cargo test"}),
    ][index]


def response(number, stream):
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
    if not stream:
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

    index = number - 4
    if index < 5:
        name, arguments = tool_for(index)
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

    def log_message(self, format_string, *args):
        print(f"fake-responses request={self.request_number} path={self.path}", flush=True)

    def do_POST(self):
        ResponsesHandler.request_number += 1
        length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(length))
        stream = bool(request.get("stream", True))
        payload = response(ResponsesHandler.request_number, stream)
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
    args = parser.parse_args()
    server = HTTPServer((args.host, args.port), ResponsesHandler)
    if args.port_file:
        args.port_file.write_text(str(server.server_address[1]) + "\n")
    if args.ready_file:
        args.ready_file.touch()
    server.serve_forever()


if __name__ == "__main__":
    main()
