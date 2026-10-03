//! Black-box contract tests for `tokenfold mcp serve`: newline-delimited
//! JSON-RPC 2.0 over stdio, `initialize`/`tools/list`/`tools/call` for `tokenfold_compress` and
//! `tokenfold_inspect`, and notifications (no `id`) never getting a response.

use serde_json::json;

#[test]
fn evidence_search_requires_host_publication_and_honors_total_restore_budget() {
    let root = unique_temp_path("search");
    let manifest_path = unique_temp_path("search_manifest");
    let store = tokenfold_core::retrieval_store::RetrievalStore::filesystem(&root);
    let approved = store
        .store(b"parser failed on token stream", "host", Some(3600))
        .unwrap();
    let _private = store
        .store(b"parser confidential unpublished", "host", Some(3600))
        .unwrap();
    std::fs::write(
        &manifest_path,
        json!({"version":1,"namespaces":{"host":[approved.hash]}}).to_string(),
    )
    .unwrap();
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{
        "name":"tokenfold_retrieve", "arguments":{"query":"parser", "namespace":"host"}
    }})
    .to_string()
        + "\n";
    let envs = [
        ("TOKENFOLD_RETRIEVAL_STORE_PATH", root.to_str().unwrap()),
        (
            "TOKENFOLD_RETRIEVAL_SEARCH_MANIFEST",
            manifest_path.to_str().unwrap(),
        ),
        ("TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES", "host"),
    ];
    let responses = run_mcp_with_env(&request, &envs);
    let result = &responses[0]["result"]["structuredContent"];
    assert_eq!(result["status"], "found");
    assert_eq!(result["hits"].as_array().unwrap().len(), 1);
    assert_eq!(result["hits"][0]["hash"], approved.hash);
    let mut bounded = envs.to_vec();
    bounded.push(("TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES", "1"));
    let responses = run_mcp_with_env(&request, &bounded);
    let result = &responses[0]["result"]["structuredContent"];
    assert_eq!(result["status"], "over_budget");
    assert!(result["hits"].as_array().unwrap().is_empty());
    let unauthorized = request.replace("\"namespace\":\"host\"", "\"namespace\":\"other\"");
    let responses = run_mcp_with_env(&unauthorized, &envs);
    assert_eq!(
        responses[0]["result"]["structuredContent"]["status"],
        "unauthorized"
    );
    let missing = request.replace("parser", "nonmatchingword");
    let responses = run_mcp_with_env(&missing, &envs);
    assert_eq!(
        responses[0]["result"]["structuredContent"]["status"],
        "no_match"
    );
    let mut invalid = envs.to_vec();
    invalid.push(("TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES", "invalid"));
    assert_eq!(
        run_mcp_with_env(&request, &invalid)[0]["result"]["isError"],
        true
    );
    // Removing publication takes effect on the next process/query, without stale index hits.
    std::fs::write(
        &manifest_path,
        json!({"version":1,"namespaces":{}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        run_mcp_with_env(&request, &envs)[0]["result"]["structuredContent"]["status"],
        "no_match"
    );
    std::fs::write(
        &manifest_path,
        json!({"version":2,"namespaces":{}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        run_mcp_with_env(&request, &envs)[0]["result"]["isError"],
        true
    );
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_file(manifest_path).unwrap();
}
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

#[test]
fn pruning_and_retrieval_share_the_host_configured_store_after_restart() {
    let store_path = unique_temp_path("pruning_host_store");
    let content = include_str!("../../../examples/incident_feed.json");
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{
        "name":"tokenfold_compress", "arguments":{
            "content":content, "format":"json", "target_tokens":50,
            "pruning":{"keep_ratio":0.05, "retrieval_namespace":"host"}
        }
    }});
    let envs = [
        (
            "TOKENFOLD_RETRIEVAL_STORE_PATH",
            store_path.to_str().unwrap(),
        ),
        ("TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES", "host"),
    ];
    let responses = run_mcp_with_env(&format!("{request}\n"), &envs);
    let result = &responses[0]["result"]["structuredContent"];
    let compressed: serde_json::Value =
        serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    fn find_ref(value: &serde_json::Value) -> Option<serde_json::Value> {
        if value.get("$tf_ref").is_some() {
            return Some(value.clone());
        }
        match value {
            serde_json::Value::Array(values) => values.iter().find_map(find_ref),
            serde_json::Value::Object(values) => values.values().find_map(find_ref),
            _ => None,
        }
    }
    let marker = find_ref(&compressed).expect("expected a recoverable dropped row");
    let request = json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{
        "name":"tokenfold_retrieve", "arguments":{"marker":marker.to_string()}
    }});
    let responses = run_mcp_with_env(&format!("{request}\n"), &envs);
    let restored = &responses[0]["result"]["structuredContent"];
    assert_eq!(restored["status"], "found");
    let row: serde_json::Value =
        serde_json::from_str(restored["content"].as_str().unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(content).unwrap();
    assert!(original["events"].as_array().unwrap().contains(&row));
    std::fs::remove_dir_all(store_path).unwrap();
}

#[test]
fn pruning_cannot_override_the_trusted_host_store_or_namespace() {
    let store_path = unique_temp_path("trusted_store");
    let other_path = unique_temp_path("untrusted_store");
    for pruning in [
        json!({"keep_ratio":0.05, "retrieval_store":other_path, "retrieval_namespace":"host"}),
        json!({"keep_ratio":0.05, "retrieval_namespace":"other"}),
    ] {
        let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{
            "name":"tokenfold_compress", "arguments":{"content":"{}", "format":"json", "pruning":pruning}
        }});
        let responses = run_mcp_with_env(
            &format!("{request}\n"),
            &[
                (
                    "TOKENFOLD_RETRIEVAL_STORE_PATH",
                    store_path.to_str().unwrap(),
                ),
                ("TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES", "host"),
            ],
        );
        assert_eq!(responses[0]["result"]["isError"], true);
    }
    assert!(!store_path.exists());
    assert!(!other_path.exists());
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
    // The retrieval store and stats ledger now exist, so both are wired and listed.
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
    assert_eq!(structured["report"]["schema_version"], "2.0");
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
    assert!(structured.get("preview").is_none());
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
fn tools_call_retrieve_explicit_namespace_overrides_json_marker_namespace() {
    let store_path = unique_temp_path("retrieve_namespace");
    let payload = b"stored in the explicit namespace";
    let store = tokenfold_core::retrieval_store::RetrievalStore::filesystem(&store_path);
    let stored = store.store(payload, "explicit", None).unwrap();
    let marker = serde_json::json!({
        "$tf_ref": {"hash": stored.hash, "alg": "sha256", "namespace": "embedded"}
    });
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tokenfold_retrieve",
            "arguments": {"marker": marker.to_string(), "namespace": "explicit"}
        }
    });
    let responses = run_mcp_with_env(
        &format!("{request}\n"),
        &[(
            "TOKENFOLD_RETRIEVAL_STORE_PATH",
            store_path.to_str().unwrap(),
        )],
    );
    assert_eq!(
        responses[0]["result"]["structuredContent"]["content"],
        String::from_utf8_lossy(payload).into_owned()
    );
    std::fs::remove_dir_all(&store_path).ok();
}

// ---- EP-05 Part B: authorized namespaces and bounded retrieval ----

#[test]
fn tools_call_retrieve_refuses_a_namespace_outside_the_authorized_set() {
    // A host granted one namespace must not be able to read another one it was not given, even
    // though the hash is real and sitting in the same store root.
    let store_path = unique_temp_path("retrieve_unauthorized");
    let store = tokenfold_core::retrieval_store::RetrievalStore::filesystem(&store_path);
    let stored = store
        .store(b"another tenant's data", "other", None)
        .unwrap();

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tokenfold_retrieve",
            "arguments": {"hash": stored.hash, "namespace": "other"}
        }
    });
    let responses = run_mcp_with_env(
        &format!("{request}\n"),
        &[
            (
                "TOKENFOLD_RETRIEVAL_STORE_PATH",
                store_path.to_str().unwrap(),
            ),
            ("TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES", "mine"),
        ],
    );

    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(structured["status"], "unauthorized");
    // A refusal must not be a tool error -- the call succeeded and was correctly denied.
    assert_eq!(responses[0]["result"]["isError"], false);
    assert!(
        structured.get("content").is_none(),
        "an unauthorized retrieval must not leak content: {structured}"
    );
    std::fs::remove_dir_all(&store_path).ok();
}

#[test]
fn tools_call_retrieve_unauthorized_does_not_reveal_whether_the_hash_exists() {
    // Both an existing hash and an absent one must answer identically, otherwise the status
    // field becomes an existence oracle for namespaces the caller cannot read.
    let store_path = unique_temp_path("retrieve_no_oracle");
    let store = tokenfold_core::retrieval_store::RetrievalStore::filesystem(&store_path);
    let stored = store.store(b"private", "other", None).unwrap();

    let run_one = |hash: &str| {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "tokenfold_retrieve",
                "arguments": {"hash": hash, "namespace": "other"}
            }
        });
        run_mcp_with_env(
            &format!("{request}\n"),
            &[
                (
                    "TOKENFOLD_RETRIEVAL_STORE_PATH",
                    store_path.to_str().unwrap(),
                ),
                ("TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES", "mine"),
            ],
        )[0]["result"]["structuredContent"]
            .clone()
    };

    assert_eq!(run_one(&stored.hash), run_one(&"a".repeat(64)));
    std::fs::remove_dir_all(&store_path).ok();
}

#[test]
fn tools_call_retrieve_still_serves_an_authorized_namespace() {
    // Restricting the set must not break the namespaces that were actually granted.
    let store_path = unique_temp_path("retrieve_authorized_ok");
    let payload = b"my own recoverable original";
    let store = tokenfold_core::retrieval_store::RetrievalStore::filesystem(&store_path);
    let stored = store.store(payload, "mine", None).unwrap();

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tokenfold_retrieve",
            "arguments": {"hash": stored.hash, "namespace": "mine"}
        }
    });
    let responses = run_mcp_with_env(
        &format!("{request}\n"),
        &[
            (
                "TOKENFOLD_RETRIEVAL_STORE_PATH",
                store_path.to_str().unwrap(),
            ),
            ("TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES", "mine,spare"),
        ],
    );
    assert_eq!(
        responses[0]["result"]["structuredContent"]["content"],
        String::from_utf8_lossy(payload).into_owned()
    );
    std::fs::remove_dir_all(&store_path).ok();
}

#[test]
fn tools_call_retrieve_refuses_an_oversized_restore_and_never_truncates() {
    // Bounded retrieval refuses rather than returning a prefix: a truncated JSON row handed back
    // as the original would silently corrupt the recovered context.
    let store_path = unique_temp_path("retrieve_over_budget");
    let payload = b"{\"row\":1,\"payload\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}";
    let store = tokenfold_core::retrieval_store::RetrievalStore::filesystem(&store_path);
    let stored = store.store(payload, "default", None).unwrap();

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "tokenfold_retrieve", "arguments": {"hash": stored.hash}}
    });
    let responses = run_mcp_with_env(
        &format!("{request}\n"),
        &[
            (
                "TOKENFOLD_RETRIEVAL_STORE_PATH",
                store_path.to_str().unwrap(),
            ),
            ("TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES", "10"),
        ],
    );

    let structured = &responses[0]["result"]["structuredContent"];
    assert_eq!(structured["status"], "over_budget");
    assert_eq!(structured["bytes"], payload.len());
    assert_eq!(structured["limit_bytes"], 10);
    assert!(
        structured.get("content").is_none(),
        "an over-budget retrieval must return no partial content: {structured}"
    );
    std::fs::remove_dir_all(&store_path).ok();
}

#[test]
fn tools_call_retrieve_is_unrestricted_when_no_authorization_is_configured() {
    // Backward compatibility: an existing host that configures nothing keeps the pre-EP-05
    // behavior rather than being locked out of its own store.
    let store_path = unique_temp_path("retrieve_unrestricted");
    let payload = b"legacy unrestricted retrieval";
    let store = tokenfold_core::retrieval_store::RetrievalStore::filesystem(&store_path);
    let stored = store.store(payload, "anything", None).unwrap();

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "tokenfold_retrieve",
            "arguments": {"hash": stored.hash, "namespace": "anything"}
        }
    });
    let responses = run_mcp_with_env(
        &format!("{request}\n"),
        &[(
            "TOKENFOLD_RETRIEVAL_STORE_PATH",
            store_path.to_str().unwrap(),
        )],
    );
    assert_eq!(
        responses[0]["result"]["structuredContent"]["content"],
        String::from_utf8_lossy(payload).into_owned()
    );
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
        preset: "balanced".to_string(),
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
