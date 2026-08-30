#!/usr/bin/env bash
set -u
set -o pipefail

root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

run_group() {
    local capability="$1"
    shift
    printf '\n== post-RC %s ==\n' "$capability"
    local rc=0 command
    for command in "$@"; do
        (cd "$root/rust" && bash -lc "$command") || rc=1
    done
    if [ "$rc" -eq 0 ]; then
        printf 'CLAW_SECURITY_ASSERTION %s PASS\n' "$capability"
        return 0
    fi
    printf 'CLAW_SECURITY_ASSERTION %s FAIL\n' "$capability"
    return 1
}

failed=0
run_group trusted_git_boundary \
    'cargo test -p tools --lib git_intelligence::tests' || failed=1
run_group retrieval_boundary \
    'cargo test -p tools --lib context_search::tests' \
    'cargo test -p tools --test retrieval_process_boundary' || failed=1
run_group trusted_context_boundary \
    'cargo test -p rusty-claude-cli --lib context_reference::tests' || failed=1
run_group terminal_rendering_boundary \
    'cargo test -p rusty-claude-cli --lib render::tests' || failed=1
run_group trusted_attachment_boundary \
    'cargo test -p rusty-claude-cli --lib snapshot_feeds_typed_image_without_host_path_or_toctou_reread' \
    'cargo test -p rusty-claude-cli --lib persisted_image_metadata_cannot_restore_bytes_or_host_authority' \
    'cargo test -p rusty-claude-cli --lib full_user_attach_command_reaches_both_provider_serializers' \
    'cargo test -p rusty-claude-cli --lib assistant_attach_text_does_not_enter_user_command_dispatch' \
    'cargo test -p rusty-claude-cli --lib private_image_snapshot_and_typed_request_leave_no_persistent_canary' \
    'cargo test -p rusty-claude-cli --lib normal_resume_does_not_restore_image_attachment_or_reread_host_path' || failed=1
run_group multimodal_image_boundary \
    'cargo test -p rusty-claude-cli --lib image_capability_blocks_unsupported_and_unknown_before_provider_runtime' \
    'cargo test -p rusty-claude-cli --lib active_image_history_uses_snapshot_after_host_file_is_deleted' \
    'cargo test -p rusty-claude-cli --lib full_user_attach_command_reaches_both_provider_serializers' \
    'cargo test -p rusty-claude-cli --lib rejects_adversarial_capability_names' || failed=1

exit "$failed"
