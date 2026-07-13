//! Black-box contract tests for `tokenfold mcp serve` per INTERFACES.md §4: newline-delimited
//! JSON-RPC 2.0 over stdio, `initialize`/`tools/list`/`tools/call` for `tokenfold_compress` and
//! `tokenfold_inspect`, and notifications (no `id`) never getting a response.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_tokenfold")
}

/// Spawns `tokenfold mcp serve` with `envs` set on the child process only (never the test
/// process's own environment), writes `requests` (already newline-delimited) to stdin, closes
/// stdin, and parses each non-empty stdout line as a JSON-RPC message.
fn run_mcp_with_env(requests: &str, envs: &[(&str, &str)]) -> Vec<serde_json::Value> {
    let mut cmd = Command::new(bin());
    cmd.args(["mcp", "serve"]);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(requests.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn run_mcp(requests: &str) -> Vec<serde_json::Value> {
    run_mcp_with_env(requests, &[])
}

fn unique_temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "tokenfold_mcp_test_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn initialize_and_tools_list_contract() {
    let requests = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    );
    let responses = run_mcp(&requests);

    assert_eq!(
        responses.len(),
        2,
        "the id-less notification must not get a response: {responses:?}"
    );
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "tokenfold");

    assert_eq!(responses[1]["id"], 2);
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"tokenfold_compress"));
    assert!(names.contains(&"tokenfold_inspect"));
    // The retrieval store and stats ledger now exist, so both are wired and listed (INTERFACES.md
    // §4).
    assert!(names.contains(&"tokenfold_retrieve"));
    assert!(names.contains(&"tokenfold_stats"));
}

#[test]
fn tools_call_compress_returns_messages_array_and_report() {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tokenfold_compress",
            "arguments": {
                "messages": [
                    {"role": "system", "content": "You are a helpful assistant."},
                    {"role": "user", "content": "hello"}
                ]
            }
        }
    });
    let responses = run_mcp(&format!("{request}\n"));
    assert_eq!(responses.len(), 1);
    let structured = &responses[0]["result"]["structuredContent"];
    assert!(structured["messages"].is_array());
    assert_eq!(structured["report"]["schema_version"], "1.0");
    assert_eq!(responses[0]["result"]["isError"], false);
}

#[test]
fn tools_call_inspect_never_returns_a_modified_payload() {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tokenfold_inspect",
            "arguments": {"content": "hello world"}
        }
    });
    let responses = run_mcp(&format!("{request}\n"));
    let structured = &responses[0]["result"]["structuredContent"];
    assert!(structured.get("content").is_none());
    assert!(structured.get("messages").is_none());
    assert!(structured["preview"].is_string());
    assert!(structured["report"]["schema_version"].is_string());
}

#[test]
fn tools_call_rejects_neither_content_nor_messages() {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tokenfold_compress", "arguments": {}}
    });
    let responses = run_mcp(&format!("{request}\n"));
    assert_eq!(responses[0]["result"]["isError"], true);
}

#[test]
fn tools_call_rejects_both_content_and_messages() {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tokenfold_compress",
            "arguments": {"content": "a", "messages": []}
        }
    });
    let responses = run_mcp(&format!("{request}\n"));
    assert_eq!(responses[0]["result"]["isError"], true);
}

#[test]
fn unknown_tool_name_is_a_protocol_error_not_a_tool_error() {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "not_a_real_tool", "arguments": {}}
    });
    let responses = run_mcp(&format!("{request}\n"));
    assert_eq!(responses[0]["error"]["code"], -32602);
}

#[test]
fn unknown_method_returns_jsonrpc_method_not_found() {
    let request = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "not/a/method"});
    let responses = run_mcp(&format!("{request}\n"));
    assert_eq!(responses[0]["error"]["code"], -32601);
}

#[test]
fn malformed_json_line_returns_parse_error_and_does_not_crash_the_server() {
    let requests = format!(
        "not json at all\n{}\n",
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})
    );
    let responses = run_mcp(&requests);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert_eq!(responses[1]["result"], serde_json::json!({}));
}

// ---- tokenfold_retrieve ----

#[test]
fn tools_call_retrieve_round_trips_a_payload_stored_via_cli_compress() {
    let store_path = unique_temp_path("retrieve_roundtrip");
    let payload = b"the quick brown fox jumps over the lazy dog, over and over.";

    // Pre-populate the retrieval store via `tokenfold compress --store-originals`, scoped to
    // this test's own temp store path via env (never the test process's own environment).
    let mut child = Command::new(bin())
        .args(["compress", "-", "--format", "text", "--store-originals"])
        .env("TOKENFOLD_RETRIEVAL_STORE_PATH", &store_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(payload).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "compress stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hash = tokenfold_core::retrieval_store::hex_sha256(payload);
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tokenfold_retrieve", "arguments": {"hash": hash}}
    });
    let responses = run_mcp_with_env(
        &format!("{request}\n"),
        &[(
            "TOKENFOLD_RETRIEVAL_STORE_PATH",
            store_path.to_str().unwrap(),
        )],
    );

    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(structured["status"], "found");
    assert_eq!(structured["source"], "local_mcp");
    assert_eq!(
        structured["content"],
        String::from_utf8_lossy(payload).into_owned()
    );
    assert_eq!(responses[0]["result"]["isError"], false);

    std::fs::remove_dir_all(&store_path).ok();
}

#[test]
fn tools_call_retrieve_missing_hash_returns_missing_status_not_a_tool_error() {
    let store_path = unique_temp_path("retrieve_missing");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tokenfold_retrieve",
            "arguments": {"hash": "0".repeat(64)}
        }
    });
    let responses = run_mcp_with_env(
        &format!("{request}\n"),
        &[(
            "TOKENFOLD_RETRIEVAL_STORE_PATH",
            store_path.to_str().unwrap(),
        )],
    );

    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(structured["status"], "missing");
    assert_eq!(structured["source"], "local_mcp");
    assert_eq!(responses[0]["result"]["isError"], false);

    std::fs::remove_dir_all(&store_path).ok();
}

#[test]
fn tools_call_retrieve_requires_at_least_one_reference() {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tokenfold_retrieve", "arguments": {}}
    });
    let responses = run_mcp(&format!("{request}\n"));
    assert_eq!(responses[0]["result"]["isError"], true);
}

// ---- tokenfold_stats ----

#[test]
fn tools_call_stats_aggregates_the_local_ledger_and_carries_no_raw_payload() {
    let ledger_path = unique_temp_path("stats_ledger").with_extension("db");
    let record = tokenfold_core::stats::LedgerRecord {
        request_id: "tc-mcptest1".to_string(),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        surface: "cli".to_string(),
        format: "plain_text".to_string(),
        mode: "balanced".to_string(),
        status: "compressed".to_string(),
        original_tokens: 1000,
        compressed_tokens: 600,
        saved_tokens: 400,
        savings_pct: 40.0,
        bypass_reason: None,
        project_hash: None,
    };
    tokenfold_core::stats::LedgerStore::new(&ledger_path)
        .append(&record)
        .unwrap();

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tokenfold_stats", "arguments": {"scope": "project"}}
    });
    let responses = run_mcp_with_env(
        &format!("{request}\n"),
        &[(
            "TOKENFOLD_ANALYTICS_LEDGER_DB",
            ledger_path.to_str().unwrap(),
        )],
    );

    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(structured["schema_version"], "1.0");
    assert_eq!(structured["scope"], "project");
    assert_eq!(structured["requests"], 1);
    assert_eq!(structured["raw_tokens"], 1000);
    assert_eq!(structured["compressed_tokens"], 600);
    assert!(structured.get("recent_requests").is_some());

    std::fs::remove_file(&ledger_path).ok();
}

#[test]
fn tools_call_stats_rejects_unknown_scope() {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tokenfold_stats", "arguments": {"scope": "nonsense"}}
    });
    let responses = run_mcp(&format!("{request}\n"));
    assert_eq!(responses[0]["result"]["isError"], true);
}
