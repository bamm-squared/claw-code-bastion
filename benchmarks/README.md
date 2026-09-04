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
server for the repository-owned acceptance fixtures. It returns explorer
findings, the writer's read/write tool sequence, and a deterministic evaluator
result; all filesystem changes still occur through the real Claw tool
executor. Select the maintained multi-file retry-policy fixture with
`--task retry-policy` when exercising that task directly.

Run it directly for protocol debugging with:

```bash
python3 benchmarks/fake_responses_provider.py --port 18766
```

Point a zero-provider production run at `http://127.0.0.1:18766/v1` using the
existing `OPENAI_BASE_URL` test override. This fixture is intentionally not an
acceptance oracle: the hidden oracle remains in `tasks.v1.json` and is consumed
outside the candidate workspace by the benchmark parent.

## Canonical deterministic acceptance

The repository-owned local command is:

```bash
./scripts/run-local-acceptance.sh
```

It runs exactly one `config-threading` task through the production Claw child,
real candidate tools, the worker and dedicated Rust validator images, trusted
validation/evaluation, interactive Review/Apply, and the hidden oracle. The
provider is a loopback-only deterministic Responses server; the script removes
proxy variables and uses a dummy local credential, so it makes no external
model calls. It uses the secret-free repository-owned
`benchmarks/settings.local.json` resource pool, and worker and validator images
are required to be distinct.

Each run writes to a unique `artifacts/acceptance/<timestamp>-<pid>/` directory
by default. The directory contains `result.jsonl`, `telemetry.json`, provider
and runner logs, build output, and fake-provider readiness metadata. Failed
runs retain the same directory. Use `--artifacts-dir PATH` to select a stable
location for CI or local investigation.

This acceptance is intentionally heavier than normal Rust CI because it needs
rootless Podman, both container images, a PTY, and a disposable project. The
manual `Deterministic acceptance` workflow runs it on the labeled self-hosted
rootless-Podman runner. Ordinary pull-request CI continues to run the normal
format, check, test, clippy, build, and documentation gates.

## Explicit live acceptance

Live provider execution is a separate operator action. It must use an
authorized model manifest and credential environment, and should remain a
single-task invocation with an explicit output directory:

```bash
CLAW_PROVIDER_TRACE=1 \
CLAW_ORCHESTRATION_TRACE=1 \
CLAW_BENCH_TELEMETRY="$PWD/artifacts/acceptance/live/telemetry.json" \
CLAW_VALIDATOR_IMAGE=claw-bastion-validator-rust:0.1.0-rc.2 \
rust/target/debug/agent-bench run \
  --execution production --interactive \
  --tasks benchmarks/tasks.v1.json --task config-threading \
  --models /absolute/path/to/authorized-models.json \
  --settings /absolute/path/to/.claw/settings.json \
  --binary "$PWD/rust/target/debug/claw" \
  --runtime-image ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.2 \
  --validator-image claw-bastion-validator-rust:0.1.0-rc.2 \
  --task-timeout 180 --exploration-timeout 45 --repetitions 1 \
  --output "$PWD/artifacts/acceptance/live/result.jsonl"
```

Do not pass `--model` for routed acceptance; omitting it preserves the full
configured resource pool. A pinned model is a separate deliberate experiment.
The live command is not called by tests or CI, and its profile manifest,
credential source, task count, images, and artifact paths are visible in the
command before execution. Never place credentials in settings, manifests, or
artifacts.

## Realistic multi-file capability task

`tasks.realistic.v1.json` contains the `retry-policy` fixture, which requires a
bounded exponential retry policy to be added across configuration, retry
calculation, client integration, and visible tests. Its hidden oracle runs an
independent Rust test against the candidate and checks default, custom-cap,
zero-base, client-integration, and preservation behavior. The fixture is
deliberately separate from the canonical `config-threading` acceptance so the
small lifecycle gate and the multi-file capability probe remain reproducible
and independently selectable.
