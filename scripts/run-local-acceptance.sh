#!/usr/bin/env bash
set -Eeuo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TASK_ID=config-threading
WORKER_IMAGE=${CLAW_ACCEPTANCE_WORKER_IMAGE:-ghcr.io/bamm-squared/claw-bastion-runtime:0.1.0-rc.2}
VALIDATOR_IMAGE=${CLAW_ACCEPTANCE_VALIDATOR_IMAGE:-claw-bastion-validator-rust:0.1.0-rc.2}
SETTINGS_PATH=${CLAW_ACCEPTANCE_SETTINGS_PATH:-"$ROOT/benchmarks/settings.local.json"}
ARTIFACTS_DIR=${CLAW_ACCEPTANCE_ARTIFACTS_DIR:-"$ROOT/artifacts/acceptance/$(date -u +%Y%m%dT%H%M%SZ)-$$"}

usage() {
    cat <<'EOF'
Usage: scripts/run-local-acceptance.sh [--artifacts-dir PATH]

Runs the single repository-owned config-threading acceptance against the
loopback deterministic Responses provider. It never contacts an external
provider.
EOF
}

while (($#)); do
    case "$1" in
        --artifacts-dir)
            (($# >= 2)) || { echo "--artifacts-dir requires a path" >&2; exit 2; }
            ARTIFACTS_DIR=$2
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

mkdir -p "$ARTIFACTS_DIR"
ARTIFACTS_DIR=$(cd "$ARTIFACTS_DIR" && pwd)
RESULT_PATH=$ARTIFACTS_DIR/result.jsonl
TELEMETRY_PATH=$ARTIFACTS_DIR/telemetry.json
PROVIDER_LOG=$ARTIFACTS_DIR/provider.log
RUNNER_LOG=$ARTIFACTS_DIR/runner.log
BUILD_LOG=$ARTIFACTS_DIR/build.log
PORT_FILE=$ARTIFACTS_DIR/provider.port
READY_FILE=$ARTIFACTS_DIR/provider.ready

provider_pid=
cleanup() {
    status=$?
    if [[ -n "${provider_pid:-}" ]] && kill -0 "$provider_pid" 2>/dev/null; then
        kill "$provider_pid" 2>/dev/null || true
        wait "$provider_pid" 2>/dev/null || true
    fi
    exit "$status"
}
trap cleanup EXIT INT TERM

if [[ "$WORKER_IMAGE" == "$VALIDATOR_IMAGE" ]]; then
    echo "worker and validator images must remain distinct" >&2
    exit 1
fi

command -v podman >/dev/null || { echo "podman is required" >&2; exit 1; }
podman info --format '{{.Host.Security.Rootless}}' | grep -qx true || {
    echo "rootless Podman is required" >&2
    exit 1
}
podman image exists "$WORKER_IMAGE" || {
    echo "worker image is not available locally: $WORKER_IMAGE" >&2
    exit 1
}
podman image exists "$VALIDATOR_IMAGE" || {
    echo "validator image is not available locally: $VALIDATOR_IMAGE" >&2
    exit 1
}

python3 - "$SETTINGS_PATH" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    settings = json.load(handle)
resources = settings.get("modelResources") or []
if len(resources) != 4:
    raise SystemExit(f"expected four modelResources, found {len(resources)}")
for resource in resources:
    endpoint = resource.get("endpoint")
    if endpoint and not endpoint.startswith(("http://127.0.0.1", "http://localhost")):
        raise SystemExit("local acceptance requires loopback model endpoints")
print(f"local settings: modelResources={len(resources)} routed=true")
PY

echo "building workspace" | tee "$BUILD_LOG"
(cd "$ROOT/rust" && cargo build --workspace) >>"$BUILD_LOG" 2>&1

echo "starting repository-owned fake Responses provider" | tee "$PROVIDER_LOG"
python3 "$ROOT/benchmarks/fake_responses_provider.py" \
    --host 127.0.0.1 --port 0 \
    --port-file "$PORT_FILE" --ready-file "$READY_FILE" >>"$PROVIDER_LOG" 2>&1 &
provider_pid=$!
for _ in {1..100}; do
    if [[ -f "$READY_FILE" && -s "$PORT_FILE" ]]; then
        break
    fi
    kill -0 "$provider_pid" 2>/dev/null || {
        echo "fake provider exited before readiness" >&2
        exit 1
    }
    sleep 0.1
done
[[ -s "$PORT_FILE" ]] || { echo "fake provider did not become ready" >&2; exit 1; }
provider_port=$(<"$PORT_FILE")
[[ "$provider_port" =~ ^[0-9]+$ ]] || { echo "invalid fake provider port" >&2; exit 1; }

echo "running deterministic acceptance in $ARTIFACTS_DIR" | tee "$RUNNER_LOG"
set +e
(
    cd "$ROOT"
    env -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY \
        OPENAI_API_KEY=local-acceptance-only \
        OPENAI_BASE_URL="http://127.0.0.1:$provider_port/v1" \
        NO_PROXY=127.0.0.1,localhost \
        CLAW_PROVIDER_TRACE=1 CLAW_ORCHESTRATION_TRACE=1 \
        CLAW_BENCH_PERMISSION_RESPONSE=deny \
        CLAW_BENCH_TELEMETRY="$TELEMETRY_PATH" \
        CLAW_VALIDATOR_IMAGE="$VALIDATOR_IMAGE" \
        "$ROOT/rust/target/debug/agent-bench" run \
        --execution production --interactive \
        --tasks "$ROOT/benchmarks/tasks.v1.json" --task "$TASK_ID" \
        --models "$ROOT/benchmarks/models.local.json" \
        --settings "$SETTINGS_PATH" \
        --binary "$ROOT/rust/target/debug/claw" \
        --runtime-image "$WORKER_IMAGE" \
        --validator-image "$VALIDATOR_IMAGE" \
        --task-timeout 180 --exploration-timeout 45 --repetitions 1 \
        --output "$RESULT_PATH"
) >>"$RUNNER_LOG" 2>&1
runner_status=$?
set -e

python3 - "$RESULT_PATH" "$TELEMETRY_PATH" "$TASK_ID" <<'PY'
import json
import os
import signal
import sys

result_path, telemetry_path, task_id = sys.argv[1:]
if not os.path.exists(result_path):
    raise SystemExit(f"missing benchmark result: {result_path}")
with open(result_path, encoding="utf-8") as handle:
    records = [json.loads(line) for line in handle if line.strip()]
if len(records) != 1:
    raise SystemExit(f"expected one benchmark record, found {len(records)}")
record = records[0]
actual_task = record.get("task_id", record.get("task"))
if actual_task != task_id:
    raise SystemExit(f"unexpected task in result: {actual_task}")
if record.get("final_correctness") != "PASS":
    raise SystemExit(f"hidden oracle result: {record.get('final_correctness')}")
if record.get("validation") not in ("completed", "PASS", "pass"):
    raise SystemExit(f"validation result: {record.get('validation')}")
if record.get("protocol") != "responses":
    raise SystemExit(f"provider protocol: {record.get('protocol')}")
if not record.get("executed_profile"):
    raise SystemExit("benchmark result has no executed profile")
if record.get("activity", {}).get("provider_calls", 0) < 1:
    raise SystemExit("benchmark result has no provider activity")
if not os.path.exists(telemetry_path):
    raise SystemExit(f"missing telemetry: {telemetry_path}")
with open(telemetry_path, encoding="utf-8") as handle:
    telemetry = json.load(handle)
if telemetry.get("terminal_status") != "completed":
    raise SystemExit(f"local acceptance terminal status: {telemetry.get('terminal_status')}")
print("ACCEPTANCE: PASS")
print(f"result: {result_path}")
print(f"telemetry: {telemetry_path}")
print(f"profile: {record['executed_profile']}")
print(f"provider: {record['provider']}")
print(f"protocol: {record['protocol']}")
print(f"validation: {record['validation']}")
print(f"hidden oracle: {record['final_correctness']}")
PY
checker_status=$?

if ((runner_status != 0 || checker_status != 0)); then
    echo "ACCEPTANCE: FAIL (artifacts retained in $ARTIFACTS_DIR)" >&2
    exit 1
fi
