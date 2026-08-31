# Agent benchmark harness

Phase 0 measures the current Bastion agent without changing its behavior.

The runner uses the production `runtime::ConversationRuntime`, typed usage
accounting, permission policy, session tracing, and tool-loop events. The
default `local-mock` profile is only a deterministic harness-validation path;
its results must not be interpreted as model-quality results.

## Run the harness validation path

From the repository root:

```bash
cargo run -p agent-bench -- run \
  --tasks benchmarks/tasks.v1.json \
  --models benchmarks/models.example.json \
  --model local-mock \
  --repetitions 1 \
  --output /tmp/bastion-benchmark.jsonl
```

Compare two JSONL result sets:

```bash
cargo run -p agent-bench -- compare baseline.jsonl current.jsonl
```

## Schema and data hygiene

- Task schema version is `1`.
- Hidden oracle data is consumed by the parent benchmark process and is never
  copied into the synthetic project.
- Results contain structured metrics by default; full transcripts are not
  retained by default.
- Model profiles contain pricing metadata only, never credentials.
- Paid profiles must be explicitly marked `authorized: true` by the operator.
- Local profiles should use actual zero-cost metadata when appropriate.

Real/local provider execution is intentionally not claimed by this initial
runner. A future adapter must invoke the packaged production CLI/runtime and
retain the same manifest and metric schema.
