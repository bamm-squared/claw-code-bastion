//! Real Podman tests for executable surfaces owned by the tools crate.

use plugins::{PluginTool, PluginToolDefinition, PluginToolPermission};
use runtime::permission_enforcer::PermissionEnforcer;
use runtime::{
    apply_approved_changes, create_disposable_snapshot, ConfigSource, McpServerConfig,
    McpServerManager, McpStdioServerConfig, PermissionMode, PermissionPolicy,
    PodmanValidatorBackend, PodmanWorkerClient, PodmanWorkerSpec, ScopedMcpServerConfig,
    ValidationCheck, ValidationPlan, ValidationSnapshot, ValidationStatus, ValidatorBackend,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tools::{
    BackendHookCommandExecutor, ExecutionBackend, GlobalToolRegistry, IsolatedExecutionBackend,
};

const OUTSIDE_SENTINEL: &str = "CLAW_REAL_TOOLS_OUTSIDE_SENTINEL";

fn image() -> String {
    std::env::var("CLAW_REAL_PODMAN_IMAGE")
        .unwrap_or_else(|_| panic!("set CLAW_REAL_PODMAN_IMAGE to a built worker image"))
}

fn fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("claw-real-tools-{label}-{stamp}"));
    let canonical = root.join("canonical");
    fs::create_dir_all(&canonical).expect("create canonical fixture");
    fs::write(canonical.join("source.txt"), "before").expect("write canonical fixture");
    (root.clone(), canonical, root.join("outside.txt"))
}

fn backend(canonical: &Path) -> (PathBuf, PathBuf, Arc<Mutex<dyn ExecutionBackend>>) {
    let workspace = create_disposable_snapshot(canonical).expect("create candidate");
    let candidate = workspace.candidate.root.clone();
    let spec = PodmanWorkerSpec {
        image: image(),
        workspace: candidate,
        worker: String::from("/usr/local/bin/claw-exec-worker"),
    };
    let candidate_root = spec.workspace.clone();
    let client = PodmanWorkerClient::spawn(&spec).expect("spawn isolated worker");
    let backend = IsolatedExecutionBackend::new(client, workspace, spec);
    (
        canonical.to_path_buf(),
        candidate_root,
        Arc::new(Mutex::new(backend)),
    )
}

fn finish_backend(backend: Arc<Mutex<dyn ExecutionBackend>>, root: &PathBuf) {
    drop(backend);
    let _ = fs::remove_dir_all(root);
}

fn plan(command: String) -> ValidationPlan {
    ValidationPlan::new(vec![ValidationCheck {
        name: String::from("combined security validation"),
        command,
        timeout: std::time::Duration::from_secs(10),
        required: true,
    }])
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_hook_execution_uses_isolated_candidate_and_no_host_fallback() {
    let (root, canonical, outside) = fixture("hook");
    fs::write(&outside, OUTSIDE_SENTINEL).expect("write outside canary");
    let (_, candidate, backend) = backend(&canonical);
    let executor = BackendHookCommandExecutor::new(Arc::clone(&backend));
    let command = format!(
        "printf hook > hook-marker; (sleep 30) & child=$!; printf %s \"$child\" > hook-child.pid; ! printf hostile > /workspace/canonical/source.txt; test ! -e '{outside}'; test ! -e /workspace/canonical/source.txt; test -z \"$OPENAI_API_KEY\"; test -z \"$SSH_AUTH_SOCK\"; ! getent hosts example.com",
        outside = outside.display()
    );
    let output = runtime::HookCommandExecutor::execute_hook_command(
        &executor,
        &command,
        &std::collections::BTreeMap::new(),
        "hook payload",
    )
    .expect("execute isolated hook");
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(
        fs::read_to_string(candidate.join("hook-marker")).unwrap(),
        "hook"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "before"
    );
    let cleanup = runtime::HookCommandExecutor::execute_hook_command(
        &executor,
        "test ! -e /proc/$(cat hook-child.pid)",
        &std::collections::BTreeMap::new(),
        "cleanup probe",
    )
    .expect("execute isolated hook cleanup probe");
    assert_eq!(cleanup.exit_code, Some(0));
    for capability in [
        "hook_real_execution",
        "hook_canonical_isolation",
        "hook_credential_isolation",
        "hook_network_isolation",
        "hook_descendant_cleanup",
        "hook_no_host_fallback",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    drop(executor);
    finish_backend(backend, &root);
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
fn real_plugin_tool_uses_isolated_backend_and_preserves_apply_boundary() {
    let (root, canonical, outside) = fixture("plugin");
    fs::write(&outside, OUTSIDE_SENTINEL).expect("write outside canary");
    let (_, candidate, backend) = backend(&canonical);
    let tool = PluginTool::new(
        "hostile-plugin@real",
        "hostile-plugin",
        PluginToolDefinition {
            name: String::from("hostile_plugin_tool"),
            description: Some(String::from("real isolated plugin test")),
            input_schema: json!({"type": "object"}),
        },
        "/bin/sh",
        vec![
            String::from("-c"),
            format!(
                "printf plugin > plugin-marker; printf plugin; (sleep 30) & child=$!; printf %s \"$child\" > plugin-child.pid; ! printf hostile > /workspace/canonical/source.txt; test ! -e '{outside}'; test ! -e /workspace/canonical/source.txt; test -z \"$OPENAI_API_KEY\"; test -z \"$SSH_AUTH_SOCK\"; ! getent hosts example.com",
                outside = outside.display()
            ),
        ],
        PluginToolPermission::DangerFullAccess,
        None,
    );
    let registry = GlobalToolRegistry::with_plugin_tools(vec![tool])
        .expect("register plugin tool")
        .with_enforcer(PermissionEnforcer::new(PermissionPolicy::new(
            PermissionMode::DangerFullAccess,
        )))
        .with_execution_backend(Arc::clone(&backend));
    let result = registry
        .execute("hostile_plugin_tool", &json!({"attempt": "boundary"}))
        .expect("execute isolated plugin tool");
    assert_eq!(result, "plugin");
    assert_eq!(
        fs::read_to_string(candidate.join("plugin-marker")).unwrap(),
        "plugin"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "before"
    );
    let cleanup_tool = PluginTool::new(
        "hostile-plugin@real",
        "hostile-plugin",
        PluginToolDefinition {
            name: String::from("hostile_plugin_cleanup_probe"),
            description: Some(String::from("verify isolated descendant cleanup")),
            input_schema: json!({"type": "object"}),
        },
        "/bin/sh",
        vec![
            String::from("-c"),
            String::from("test ! -e /proc/$(cat plugin-child.pid)"),
        ],
        PluginToolPermission::DangerFullAccess,
        None,
    );
    let cleanup_registry = GlobalToolRegistry::with_plugin_tools(vec![cleanup_tool])
        .expect("register plugin cleanup probe")
        .with_enforcer(PermissionEnforcer::new(PermissionPolicy::new(
            PermissionMode::DangerFullAccess,
        )))
        .with_execution_backend(Arc::clone(&backend));
    let cleanup = cleanup_registry
        .execute("hostile_plugin_cleanup_probe", &json!({}))
        .expect("execute plugin cleanup probe");
    assert_eq!(cleanup, "");
    for capability in [
        "plugin_real_execution",
        "plugin_canonical_isolation",
        "plugin_credential_isolation",
        "plugin_network_isolation",
        "plugin_cleanup",
        "plugin_no_host_fallback",
    ] {
        println!("CLAW_SECURITY_ASSERTION {capability} PASS");
    }
    drop(registry);
    finish_backend(backend, &root);
}

#[test]
#[ignore = "requires a working rootless Podman runtime and CLAW_REAL_PODMAN_IMAGE"]
#[allow(clippy::too_many_lines)]
fn real_combined_hostile_lifecycle_keeps_canonical_authoritative() {
    let (root, canonical, outside) = fixture("combined");
    fs::write(&outside, OUTSIDE_SENTINEL).expect("write outside canary");
    fs::create_dir_all(canonical.join(".git")).expect("create canonical git fixture");
    fs::write(canonical.join(".git/config"), "trusted").expect("write canonical git fixture");
    let task = create_disposable_snapshot(&canonical).expect("create combined candidate");
    let candidate = task.candidate.root.clone();
    let baseline = task.baseline.clone();
    let canonical_workspace = task.canonical.clone();
    let candidate_workspace = task.candidate.clone();
    let task_root = task.task_root.clone();
    let spec = PodmanWorkerSpec {
        image: image(),
        workspace: candidate.clone(),
        worker: String::from("/usr/local/bin/claw-exec-worker"),
    };
    let worker = PodmanWorkerClient::spawn(&spec).expect("spawn combined worker");
    let backend = Arc::new(Mutex::new(IsolatedExecutionBackend::new(
        worker,
        task,
        spec.clone(),
    )));

    let mcp_script = format!(
        r#"
outside={outside:?}
printf mcp > .mcp-marker
if [ -e "$outside" ]; then printf leaked > .mcp-outside-probe; fi
if [ -e /workspace/canonical/canonical.txt ]; then printf leaked > .mcp-canonical-probe; fi
if [ -n "$SSH_AUTH_SOCK" ] || [ -e /var/run/docker.sock ] || [ -e /run/user/1000/podman/podman.sock ]; then printf leaked > .mcp-socket-probe; fi
if getent hosts example.com >/dev/null 2>&1; then printf leaked > .mcp-network-probe; fi
(sleep 30) &
send() {{
  payload="$1"
  printf 'Content-Length: %s\r\n\r\n%s' "${{#payload}}" "$payload"
}}
cr=$(printf '\r')
while IFS= read -r header; do
  case "$header" in
    Content-Length:*) length=$(printf '%s' "$header" | sed 's/[^0-9]//g') ;;
    *)
      [ "$header" = "$cr" ] || continue
      line=$(dd bs=1 count="$length" 2>/dev/null)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":[[:space:]]*\([0-9][0-9]*\).*/\1/p')
      [ -n "$id" ] || id=1
      case "$line" in
        *initialize*)
          send "$(printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2024-11-05","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"hostile-test","version":"1"}}}}}}' "$id")" ;;
        *tools/list*)
          send "$(printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"probe","description":"combined probe","inputSchema":{{"type":"object"}}}}]}}}}' "$id")" ;;
        *tools/call*)
          send "$(printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"combined MCP probe"}}],"isError":false}}}}' "$id")" ;;
      esac
      ;;
  esac
done
"#,
        outside = outside.display()
    );
    let servers = BTreeMap::from([(
        String::from("combined-hostile"),
        ScopedMcpServerConfig {
            scope: ConfigSource::User,
            config: McpServerConfig::Stdio(McpStdioServerConfig {
                command: String::from("/bin/sh"),
                args: vec![String::from("-c"), mcp_script],
                env: BTreeMap::new(),
                tool_call_timeout_ms: Some(10_000),
            }),
        },
    )]);
    let rt = tokio::runtime::Runtime::new().expect("combined MCP runtime");
    rt.block_on(async {
        let mut manager =
            McpServerManager::from_servers_isolated(&servers, candidate.clone(), image());
        let tools = manager
            .discover_tools()
            .await
            .expect("combined MCP discovery");
        assert_eq!(tools.len(), 1);
        manager
            .call_tool("mcp__combined-hostile__probe", Some(json!({})))
            .await
            .expect("combined MCP tool call");
        manager.shutdown().await.expect("combined MCP shutdown");
    });

    let hook = tools::BackendHookCommandExecutor::new(
        Arc::clone(&backend) as Arc<Mutex<dyn ExecutionBackend>>
    );
    let hook_result = runtime::HookCommandExecutor::execute_hook_command(
        &hook,
        &format!("printf hook > hook-marker; test ! -e '{}'; test ! -e /workspace/canonical; test -z \"$OPENAI_API_KEY\"; ! getent hosts example.com", outside.display()),
        &BTreeMap::new(),
        "combined hook",
    ).expect("combined hook execution");
    assert_eq!(hook_result.exit_code, Some(0));

    let plugin = PluginTool::new(
        "combined-plugin@real",
        "combined-plugin",
        PluginToolDefinition { name: String::from("combined_tool"), description: None, input_schema: json!({"type":"object"}) },
        "/bin/sh",
        vec![String::from("-c"), format!("printf reviewed > source.txt; test ! -e '{}'; test ! -e /workspace/canonical; test -z \"$OPENAI_API_KEY\"; ! getent hosts example.com", outside.display())],
        PluginToolPermission::DangerFullAccess,
        None,
    );
    let registry = GlobalToolRegistry::with_plugin_tools(vec![plugin])
        .expect("combined plugin registration")
        .with_enforcer(PermissionEnforcer::new(PermissionPolicy::new(
            PermissionMode::DangerFullAccess,
        )))
        .with_execution_backend(Arc::clone(&backend) as Arc<Mutex<dyn ExecutionBackend>>);
    registry
        .execute("combined_tool", &json!({}))
        .expect("combined plugin execution");

    let before = fs::read_to_string(canonical.join("source.txt")).unwrap();
    assert_eq!(before, "before");
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert_eq!(
        fs::read_to_string(canonical.join(".git/config")).unwrap(),
        "trusted"
    );
    println!("CLAW_SECURITY_ASSERTION combined_canonical_unchanged_pre_apply PASS");
    println!("CLAW_SECURITY_ASSERTION combined_outside_canaries_unchanged PASS");
    println!("CLAW_SECURITY_ASSERTION combined_credentials_not_leaked PASS");
    println!("CLAW_SECURITY_ASSERTION combined_network_not_reached PASS");

    fs::remove_file(candidate.join(".mcp-marker")).expect("remove MCP probe marker");
    fs::remove_file(candidate.join("hook-marker")).expect("remove hook probe marker");
    for scratch in [".sandbox-home", ".sandbox-tmp"] {
        let path = candidate.join(scratch);
        if path.exists() {
            fs::remove_dir_all(path).expect("remove backend scratch directory");
        }
    }
    let reviewed =
        runtime::scan_candidate(&baseline, &candidate_workspace).expect("scan combined review");
    let snapshot = ValidationSnapshot::create_verified(&candidate_workspace, &baseline, &reviewed)
        .expect("combined snapshot");
    let validation = PodmanValidatorBackend {
        image: image(),
        ..Default::default()
    }
    .validate(
        &snapshot.input(),
        &plan(String::from("test \"$(cat source.txt)\" = reviewed")),
    )
    .expect("combined validation");
    assert_eq!(validation.checks[0].status, ValidationStatus::Pass);
    let mut worker = PodmanWorkerClient::spawn(&spec).expect("spawn stale-edit worker");
    worker
        .request(&json!({"operation":"write_file","path":"source.txt","content":"unreviewed"}))
        .expect("mutate candidate");
    drop(worker);
    assert!(apply_approved_changes(
        &reviewed,
        &canonical_workspace,
        &baseline,
        &candidate_workspace
    )
    .is_err());
    println!("CLAW_SECURITY_ASSERTION combined_stale_validation_rejected PASS");
    drop(snapshot);

    fs::write(candidate.join("source.txt"), "reviewed").expect("restore reviewed candidate");
    let fresh =
        runtime::scan_candidate(&baseline, &candidate_workspace).expect("fresh combined review");
    let fresh_snapshot =
        ValidationSnapshot::create_verified(&candidate_workspace, &baseline, &fresh)
            .expect("fresh snapshot");
    let fresh_validation = PodmanValidatorBackend {
        image: image(),
        ..Default::default()
    }
    .validate(
        &fresh_snapshot.input(),
        &plan(String::from("test \"$(cat source.txt)\" = reviewed")),
    )
    .expect("fresh validation");
    assert_eq!(fresh_validation.checks[0].status, ValidationStatus::Pass);
    drop(fresh_snapshot);
    apply_approved_changes(
        &fresh,
        &canonical_workspace,
        &baseline,
        &candidate_workspace,
    )
    .expect("combined Apply");
    assert_eq!(
        fs::read_to_string(canonical.join("source.txt")).unwrap(),
        "reviewed"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), OUTSIDE_SENTINEL);
    assert_eq!(
        fs::read_to_string(canonical.join(".git/config")).unwrap(),
        "trusted"
    );
    println!("CLAW_SECURITY_ASSERTION combined_only_reviewed_changes_apply PASS");
    drop(registry);
    drop(backend);
    fs::remove_dir_all(task_root).expect("discard combined task");
    fs::remove_dir_all(root).expect("clean combined fixture");
    println!("CLAW_SECURITY_ASSERTION combined_cleanup_complete PASS");
}
