#!/usr/bin/env bash
set -u
set -o pipefail

root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
image="${CLAW_REAL_PODMAN_IMAGE:-claw-exec:security}"
artifact_dir="$root/artifacts"
artifact="$artifact_dir/security-verification.json"
log_dir="$(mktemp -d "${TMPDIR:-/tmp}/claw-real-isolation.XXXXXX")"
trap 'rm -rf "$log_dir"' EXIT

capabilities=(worker_runtime worker_filesystem_isolation worker_outside_write_isolation worker_canonical_isolation worker_credential_isolation worker_network_isolation worker_socket_isolation worker_symlink_isolation worker_git_metadata_isolation worker_process_isolation worker_resource_limits worker_output_bounds worker_crash_recovery validator_runtime validator_filesystem_isolation validator_canonical_isolation validator_credential_isolation validator_network_isolation validator_socket_isolation validator_candidate_independence validator_timeout_cleanup validator_descendant_cleanup validator_output_bounds candidate_canonical_boundary validation_identity_binding whole_change_set_apply full_authoritative_lifecycle mcp_real_execution mcp_canonical_isolation mcp_outside_host_isolation mcp_credential_isolation mcp_socket_isolation mcp_network_isolation mcp_cleanup mcp_no_host_fallback hook_real_execution hook_canonical_isolation hook_credential_isolation hook_network_isolation hook_descendant_cleanup hook_no_host_fallback plugin_real_execution plugin_canonical_isolation plugin_credential_isolation plugin_network_isolation plugin_cleanup plugin_no_host_fallback private_isolation_mandatory private_no_session_persistence private_no_resume private_webfetch_denied private_websearch_denied private_provider_fallback_denied private_provider_policy private_review_apply webfetch_http_https_validation webfetch_loopback_denial webfetch_private_address_denial webfetch_link_local_denial webfetch_metadata_denial webfetch_ipv6_denial webfetch_ipv4_mapped_ipv6_denial webfetch_redirect_revalidation webfetch_timeout_bound webfetch_body_bound webfetch_header_credential_isolation webfetch_dns_rebinding_toctou custom_runtime_trusted_selection custom_runtime_project_override_denied custom_runtime_network_none custom_runtime_mount_restrictions custom_runtime_credentials_sockets_unavailable custom_runtime_no_host_fallback combined_canonical_unchanged_pre_apply combined_outside_canaries_unchanged combined_credentials_not_leaked combined_network_not_reached combined_stale_validation_rejected combined_only_reviewed_changes_apply combined_cleanup_complete provider_worker_credential_isolation provider_validator_credential_isolation provider_mcp_credential_isolation provider_host_secret_redaction trusted_git_boundary retrieval_boundary trusted_context_boundary terminal_rendering_boundary trusted_attachment_boundary multimodal_image_boundary)
declare -A status TEST_CAPABILITIES TEST_PACKAGES TEST_TARGETS TEST_BIN TEST_LIB TEST_IGNORED TEST_EVIDENCE capability_test capability_evidence
for capability in "${capabilities[@]}"; do status["$capability"]="not_tested"; done

map_test() {
    local name="$1"; shift
    TEST_CAPABILITIES["$name"]="${TEST_CAPABILITIES[$name]:-} $*"
    TEST_PACKAGES["$name"]="runtime"
    TEST_TARGETS["$name"]="podman_isolation"
    TEST_IGNORED["$name"]=1
    TEST_EVIDENCE["$name"]="test process must pass every mapped assertion"
    local capability
    for capability in $*; do
        capability_test["$capability"]="$name"
        capability_evidence["$capability"]="${TEST_EVIDENCE[$name]}"
    done
}
map_test_runtime() { local name="$1"; shift; map_test "$name" "$@"; TEST_PACKAGES["$name"]="runtime"; TEST_TARGETS["$name"]="podman_isolation"; }
map_deterministic() { local name="$1"; shift; map_test "$name" "$@"; TEST_IGNORED["$name"]=0; }
map_cli_bin() { local name="$1"; shift; map_deterministic "$name" "$@"; TEST_PACKAGES["$name"]="rusty-claude-cli"; TEST_TARGETS["$name"]="claw"; TEST_BIN["$name"]="claw"; }
map_test real_worker_boundary_blocks_host_state_and_allows_candidate_edits "worker_runtime worker_filesystem_isolation worker_canonical_isolation worker_credential_isolation worker_network_isolation worker_socket_isolation"
map_test real_worker_network_and_mount_policy_denies_host_state "worker_network_isolation worker_socket_isolation worker_credential_isolation"
map_test real_worker_edit_positive_control "worker_runtime worker_filesystem_isolation"
map_test real_worker_outside_write_is_denied "worker_outside_write_isolation"
map_test real_worker_external_symlink_isolation "worker_symlink_isolation"
map_test real_worker_candidate_git_metadata_is_harmless "worker_git_metadata_isolation"
map_test real_worker_process_limit_and_output_bounds "worker_process_isolation worker_resource_limits worker_output_bounds"
map_test real_worker_crash_is_reported_without_fallback "worker_crash_recovery"
map_test real_validator_is_fresh_networkless_and_does_not_mutate_candidate "validator_runtime validator_credential_isolation validator_network_isolation validator_candidate_independence"
map_test real_validator_is_fresh_networkless_and_does_not_mutate_candidate "provider_validator_credential_isolation"
map_test real_validator_host_and_socket_isolation "validator_filesystem_isolation validator_canonical_isolation validator_socket_isolation"
map_test real_validator_output_bounds_and_timeout_cleanup "validator_output_bounds validator_timeout_cleanup validator_descendant_cleanup"
map_test podman_full_hostile_authoritative_lifecycle "candidate_canonical_boundary validation_identity_binding whole_change_set_apply full_authoritative_lifecycle"
map_test_runtime real_mcp_stdio_isolated_boundary_and_cleanup "mcp_real_execution mcp_canonical_isolation mcp_outside_host_isolation mcp_credential_isolation mcp_socket_isolation mcp_network_isolation mcp_cleanup mcp_no_host_fallback provider_mcp_credential_isolation"
map_test_runtime real_hook_execution_uses_isolated_candidate_and_no_host_fallback "hook_real_execution hook_canonical_isolation hook_credential_isolation hook_network_isolation hook_descendant_cleanup hook_no_host_fallback"
TEST_PACKAGES[real_hook_execution_uses_isolated_candidate_and_no_host_fallback]="tools"
TEST_TARGETS[real_hook_execution_uses_isolated_candidate_and_no_host_fallback]="podman_surfaces"
map_test_runtime real_plugin_tool_uses_isolated_backend_and_preserves_apply_boundary "plugin_real_execution plugin_canonical_isolation plugin_credential_isolation plugin_network_isolation plugin_cleanup plugin_no_host_fallback"
TEST_PACKAGES[real_plugin_tool_uses_isolated_backend_and_preserves_apply_boundary]="tools"
TEST_TARGETS[real_plugin_tool_uses_isolated_backend_and_preserves_apply_boundary]="podman_surfaces"
map_test_runtime real_private_mode_lifecycle_preserves_isolation_and_apply_boundary "private_isolation_mandatory private_review_apply"
map_cli_bin private_provider_security_assertions "private_no_session_persistence private_no_resume private_webfetch_denied private_websearch_denied private_provider_policy provider_host_secret_redaction"
TEST_IGNORED[private_provider_security_assertions]=1
map_deterministic provider_runtime_client_private_fallback_denied "private_provider_fallback_denied"
TEST_PACKAGES[provider_runtime_client_private_fallback_denied]="tools"
TEST_TARGETS[provider_runtime_client_private_fallback_denied]="tools"
TEST_LIB[provider_runtime_client_private_fallback_denied]=1
map_test real_worker_network_and_mount_policy_denies_host_state "provider_worker_credential_isolation"
map_test_runtime real_custom_runtime_preserves_outer_security_policy "custom_runtime_trusted_selection custom_runtime_project_override_denied custom_runtime_network_none custom_runtime_mount_restrictions custom_runtime_credentials_sockets_unavailable custom_runtime_no_host_fallback"
map_test_runtime real_combined_hostile_lifecycle_keeps_canonical_authoritative "combined_canonical_unchanged_pre_apply combined_outside_canaries_unchanged combined_credentials_not_leaked combined_network_not_reached combined_stale_validation_rejected combined_only_reviewed_changes_apply combined_cleanup_complete"
TEST_PACKAGES[real_combined_hostile_lifecycle_keeps_canonical_authoritative]="tools"
TEST_TARGETS[real_combined_hostile_lifecycle_keeps_canonical_authoritative]="podman_surfaces"
map_deterministic web_broker_security_assertions "webfetch_http_https_validation webfetch_loopback_denial webfetch_private_address_denial webfetch_link_local_denial webfetch_metadata_denial webfetch_ipv6_denial webfetch_ipv4_mapped_ipv6_denial webfetch_redirect_revalidation webfetch_timeout_bound webfetch_body_bound webfetch_header_credential_isolation webfetch_dns_rebinding_toctou"
TEST_PACKAGES[web_broker_security_assertions]="tools"
TEST_TARGETS[web_broker_security_assertions]="tools"
TEST_LIB[web_broker_security_assertions]=1
map_deterministic post_rc_security_boundaries "trusted_git_boundary retrieval_boundary trusted_context_boundary terminal_rendering_boundary trusted_attachment_boundary multimodal_image_boundary"
TEST_EVIDENCE[real_worker_boundary_blocks_host_state_and_allows_candidate_edits]="candidate marker is present while outside, canonical, credential, socket, and network probes fail"
TEST_EVIDENCE[real_worker_network_and_mount_policy_denies_host_state]="network, socket, credential, and protected mount probes return denial"
TEST_EVIDENCE[real_worker_edit_positive_control]="candidate edit succeeds in the isolated worker"
TEST_EVIDENCE[real_worker_outside_write_is_denied]="outside-host write probe fails"
TEST_EVIDENCE[real_worker_external_symlink_isolation]="external symlink traversal probe fails"
TEST_EVIDENCE[real_worker_candidate_git_metadata_is_harmless]="candidate git metadata cannot alter trusted behavior"
TEST_EVIDENCE[real_worker_process_limit_and_output_bounds]="bounded process and output probes complete within configured limits"
TEST_EVIDENCE[real_worker_crash_is_reported_without_fallback]="worker crash returns an error without local execution"
TEST_EVIDENCE[real_validator_is_fresh_networkless_and_does_not_mutate_candidate]="fresh validator observes candidate while credential/network probes fail and candidate is unchanged"
TEST_EVIDENCE[real_validator_host_and_socket_isolation]="validator protected-path and socket probes fail"
TEST_EVIDENCE[real_validator_output_bounds_and_timeout_cleanup]="validator output, timeout, and descendant cleanup assertions pass"
TEST_EVIDENCE[podman_full_hostile_authoritative_lifecycle]="candidate identity, review, validation, and explicit Apply assertions protect canonical state"
TEST_EVIDENCE[real_mcp_stdio_isolated_boundary_and_cleanup]="MCP candidate probe succeeds while host, canonical, credential, socket, network, and cleanup probes fail"
TEST_EVIDENCE[real_hook_execution_uses_isolated_candidate_and_no_host_fallback]="hook candidate marker succeeds; canonical write, host, credential, network, and descendant cleanup probes fail"
TEST_EVIDENCE[real_plugin_tool_uses_isolated_backend_and_preserves_apply_boundary]="plugin candidate mutation succeeds; canonical write and protected resource probes fail; cleanup probe succeeds"
TEST_EVIDENCE[real_private_mode_lifecycle_preserves_isolation_and_apply_boundary]="private lifecycle asserts isolation and candidate-only Apply behavior"
TEST_EVIDENCE[provider_runtime_client_private_fallback_denied]="primary local provider receives and fails a request while independently reachable fallback receives zero requests"
TEST_EVIDENCE[real_custom_runtime_preserves_outer_security_policy]="selected custom runtime asserts outer network, mount, credential, and socket restrictions"
TEST_EVIDENCE[real_combined_hostile_lifecycle_keeps_canonical_authoritative]="combined lifecycle asserts canonical, outside-canary, credential, network, stale-review, Apply, and cleanup outcomes"
TEST_EVIDENCE[real_web_broker_policy_matrix]="each URL policy case asserts denial before a request is made"
for test_name in "${!TEST_CAPABILITIES[@]}"; do
    for capability in ${TEST_CAPABILITIES[$test_name]}; do
        capability_test["$capability"]="$test_name"
        capability_evidence["$capability"]="${TEST_EVIDENCE[$test_name]}"
    done
done

write_artifact() {
    mkdir -p "$artifact_dir"
    local overall="incomplete" capability first=1 any_tested=0 any_failed=0 all_pass=1
    local pass_count=0 fail_count=0 not_tested_count=0
    for capability in "${capabilities[@]}"; do
        case "${status[$capability]}" in
            pass) any_tested=1; pass_count=$((pass_count + 1));;
            fail) any_tested=1; any_failed=1; fail_count=$((fail_count + 1));;
            *) all_pass=0; not_tested_count=$((not_tested_count + 1));;
        esac
    done
    if [ "$any_failed" -eq 1 ]; then overall=fail; elif [ "$any_tested" -eq 0 ]; then overall=not_run; elif [ "$all_pass" -eq 1 ]; then overall=pass; fi
    local digest="$(podman image inspect --format '{{.Id}}' "$image" 2>/dev/null || true)"
    local source_revision="$(podman image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image" 2>/dev/null || true)"
    local version="$(podman --version 2>/dev/null || true)"
    local commit="${GITHUB_SHA:-$(git -C "$root" rev-parse HEAD 2>/dev/null || printf unknown)}"
    local os_kernel="$(uname -srm 2>/dev/null || printf unknown)"
    {
      printf '{\n  "schema": "combined-security-verification-v1",\n  "commit": "%s",\n  "os_kernel": "%s",\n  "worker_image": "%s",\n  "worker_image_digest": "%s",\n  "worker_image_source_revision": "%s",\n  "podman_version": "%s",\n  "rootless": true,\n  "overall": "%s",\n  "required_capabilities": %s,\n  "pass": %s,\n  "fail": %s,\n  "not_tested": %s,\n  "capabilities": {\n' "$commit" "$os_kernel" "$image" "$digest" "$source_revision" "$version" "$overall" "${#capabilities[@]}" "$pass_count" "$fail_count" "$not_tested_count"
      for capability in "${capabilities[@]}"; do
        [ "$first" -eq 1 ] || printf ',\n'; first=0
        printf '    "%s": {"result": "%s", "test": "%s", "evidence": "%s"}' "$capability" "${status[$capability]}" "${capability_test[$capability]:-unassigned}" "${capability_evidence[$capability]:-unassigned}"
      done
      printf '\n  }\n}\n'
    } > "$artifact"
    cp "$artifact" "$artifact_dir/combined-security-verification.json"
    printf 'Verification artifact: %s\n' "$artifact"
    printf 'REAL ISOLATION VERIFICATION: %s\n' "$(printf '%s' "$overall" | tr '[:lower:]' '[:upper:]')"
    [ "$overall" = pass ]
}

record_test_results() {
    local test_name="$1" result="$2" log="$3" capability marker
    for capability in ${TEST_CAPABILITIES[$test_name]}; do
        marker="$(rg -N "^CLAW_SECURITY_ASSERTION[[:space:]]+$capability[[:space:]]+(PASS|FAIL)$" "$log" | tail -1 || true)"
        case "$marker" in
            *" PASS") status["$capability"]="pass" ;;
            *" FAIL") status["$capability"]="fail" ;;
            *) status["$capability"]="not_tested" ;;
        esac
        if [ "$result" = fail ] && [ "${status[$capability]}" = pass ]; then
            status["$capability"]="fail"
        fi
    done
}

if [ "${1:-}" = "--accounting-self-test" ]; then
    TEST_CAPABILITIES[synthetic_assertion_test]="synthetic_pass synthetic_missing synthetic_fail"
    TEST_EVIDENCE[synthetic_assertion_test]="synthetic accounting assertions"
    capability_test[synthetic_pass]=synthetic_assertion_test
    capability_test[synthetic_missing]=synthetic_assertion_test
    capability_test[synthetic_fail]=synthetic_assertion_test
    synthetic_log="$(mktemp)"
    printf '%s\n' \
        'CLAW_SECURITY_ASSERTION synthetic_pass PASS' \
        'CLAW_SECURITY_ASSERTION synthetic_fail FAIL' > "$synthetic_log"
    record_test_results synthetic_assertion_test pass "$synthetic_log"
    rm -f "$synthetic_log"
    [ "${status[synthetic_pass]}" = pass ]
    [ "${status[synthetic_missing]}" = not_tested ]
    [ "${status[synthetic_fail]}" = fail ]
    printf 'security accounting self-test: PASS\n'
    exit 0
fi

printf '%s\n' 'Claw Real Isolation Verification'
if ! CLAW_PREFLIGHT_CONFIG_ONLY=1 "$root/scripts/security-runner-preflight.sh" || ! command -v podman >/dev/null 2>&1; then
    write_artifact || true
    exit 2
fi
if [ "${CLAW_REUSE_RUNTIME_IMAGE:-0}" = 1 ]; then
    expected_id="${CLAW_EXPECTED_RUNTIME_IMAGE_ID:-}"
    expected_revision="${CLAW_EXPECTED_SOURCE_REVISION:-$(git -C "$root" rev-parse HEAD)}"
    actual_id="$(podman image inspect --format '{{.Id}}' "$image" 2>/dev/null || true)"
    actual_revision="$(podman image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image" 2>/dev/null || true)"
    if [ -z "$actual_id" ] || { [ -n "$expected_id" ] && [ "$actual_id" != "$expected_id" ]; } || [ "$actual_revision" != "$expected_revision" ]; then
        printf 'FAIL runtime identity: image=%s id=%s revision=%s expected_id=%s expected_revision=%s\n' "$image" "$actual_id" "$actual_revision" "$expected_id" "$expected_revision"
        write_artifact || true
        exit 2
    fi
else
    if ! podman build --build-arg "CLAW_SOURCE_REVISION=$(git -C "$root" rev-parse HEAD)" -f "$root/Containerfile.worker" -t "$image" "$root"; then
        write_artifact || true
        exit 2
    fi
fi
if ! CLAW_REAL_PODMAN_IMAGE="$image" "$root/scripts/security-runner-preflight.sh"; then
    write_artifact || true
    exit 2
fi

for test_name in "${!TEST_CAPABILITIES[@]}"; do
    printf '\n== %s ==\n' "$test_name"
    test_args=("$test_name" "--" "--nocapture")
    if [ "${TEST_IGNORED[$test_name]}" = 1 ]; then test_args=("$test_name" "--" "--ignored" "--nocapture"); fi
    if [ "${TEST_LIB[$test_name]:-0}" = 1 ]; then cargo_args=("--lib"); elif [ -n "${TEST_BIN[$test_name]:-}" ]; then cargo_args=("--bin" "${TEST_BIN[$test_name]}"); else cargo_args=("--test" "${TEST_TARGETS[$test_name]}"); fi
    if [ "$test_name" = post_rc_security_boundaries ]; then
        if (cd "$root" && ./scripts/test-post-rc-security.sh) 2>&1 | tee "$log_dir/$test_name.log"; then result=pass; else result=fail; fi
    elif (cd "$root/rust" && CLAW_REAL_PODMAN_IMAGE="$image" CLAW_WORKER_IMAGE="$image" CLAW_VALIDATOR_IMAGE="$image" cargo test -p "${TEST_PACKAGES[$test_name]}" "${cargo_args[@]}" "${test_args[@]}") 2>&1 | tee "$log_dir/$test_name.log"; then result=pass; else result=fail; fi
    record_test_results "$test_name" "$result" "$log_dir/$test_name.log"
done
for capability in "${capabilities[@]}"; do printf '%-42s %s\n' "$capability" "${status[$capability]}"; done
write_artifact
