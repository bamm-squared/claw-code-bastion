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
runner. The opt-in production adapter invokes the real `claw` binary and pins
the configured worker and validator images:

```bash
cargo run -p agent-bench -- run \
  --execution production \
  --tasks benchmarks/tasks.v1.json \
  --task config-threading \
  --models /path/to/authorized-models.json \
  --settings /path/to/.claw/settings.json \
  --binary /path/to/claw \
  --runtime-image localhost/claw-bastion-runtime:gate-REVISION \
  --validator-image localhost/claw-bastion-validator-rust:gate-REVISION \
  --repetitions 1 \
  --output /tmp/bastion-production-benchmark.jsonl
```

Production execution requires an operator-authored profile with
`authorized: true`; the deterministic `local-mock` profile is never accepted
as a production baseline. `--model ALIAS` pins one profile; omitting it keeps
the complete settings `modelResources` pool available for normal routing.
`--task ID` selects one existing task without rebuilding its definition.
Production requires complete Claw settings through `--settings` or
`CLAW_BENCH_SETTINGS`; the adapter never reduces them to a model string.
Worker and validator images are independent and both must be supplied for
production execution. `--dry-run` performs the same task/profile/settings
preflight without launching the child process.

## Local production-style provider

`fake_responses_provider.py` is a dependency-free deterministic Responses
server for the `config-threading` task. It returns explorer findings, the
writer's read/write/bash tool sequence, and a deterministic evaluator result;
all filesystem changes still occur through the real Claw tool executor.

Run it locally with:

```bash
python3 benchmarks/fake_responses_provider.py --port 18766
```

Point a zero-provider production run at `http://127.0.0.1:18766/v1` using the
existing `OPENAI_BASE_URL` test override. This fixture is intentionally not an
acceptance oracle: the hidden oracle remains in `tasks.v1.json` and is consumed
outside the candidate workspace by the benchmark parent.
