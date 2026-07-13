//! Minimal MCP (Model Context Protocol) stdio server: `tokenfold mcp serve`.
//!
//! Implements INTERFACES.md §4's `tokenfold_compress`, `tokenfold_inspect`, `tokenfold_retrieve`,
//! and `tokenfold_stats` tools. The backing stores for the latter two now exist
//! (`tokenfold_core::retrieval_store`/`tokenfold_core::stats`, F-045/F-046), so they're wired
//! here rather than omitted as before. `tokenfold_retrieve`'s `source` is always `"local_mcp"`
//! (this is the local retrieval store, not a proxy-side one). `tokenfold_read` is still optional
//! and off-by-default per spec, so it remains deferred.
//!
//! Neither new tool reads `tokenfold.toml`: this file has never called
//! `tokenfold-cli::config::resolve` (see `build_policy` below, which only reads tool
//! `arguments`), so — for consistency with that existing scope cut — `tokenfold_retrieve`/
//! `tokenfold_stats` only honor the same-named environment overrides `tokenfold-cli::config`
//! already documents (`TOKENFOLD_RETRIEVAL_BACKEND`, `TOKENFOLD_RETRIEVAL_STORE_PATH`,
//! `TOKENFOLD_ANALYTICS_LEDGER_DB`) rather than the full config file.
//!
//! Scope cut: `tokenfold_compress`/`tokenfold_inspect`'s own `store_originals` argument still has
//! no effect (see `tool_input_schema`) — wiring per-request storage into those two tools is
//! deferred to a future pass. Only the proxy's `/v1/compress` and provider-passthrough routes got
//! that wiring this pass (`X-TokenFold-Store-Originals`/`X-TokenFold-Retrieve-Store`,
//! INTERFACES.md §3.1).
//!
//! Transport is newline-delimited JSON-RPC 2.0 over stdio, the standard MCP stdio framing: one
//! JSON object per line in, one per line out. stdout carries only JSON-RPC messages; logs (none
//! currently) would go to stderr. No MCP SDK dependency: the subset of the protocol this server
//! needs (`initialize`, `tools/list`, `tools/call`, notification handling) is a few dozen lines
//! of `serde_json` over stdin/stdout.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};
use tokenfold_core::retrieval_store::{RetrievalOutcome, RetrievalStore};
use tokenfold_core::stats::{self, LedgerStore};
use tokenfold_core::{
    CompressionInput, CompressionMode, CompressionPolicy, InputFormat, TokenFoldError,
};

use crate::args::ModeArg;
use crate::format::FormatArg;

const PROTOCOL_VERSION: &str = "2024-11-05";

pub fn serve() -> Result<i32, TokenFoldError> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(TokenFoldError::Io)?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                write_message(
                    &mut stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"code": -32700, "message": format!("parse error: {e}")}
                    }),
                )?;
                continue;
            }
        };
        if let Some(response) = handle_request(&request) {
            write_message(&mut stdout, &response)?;
        }
    }
    Ok(0)
}

fn write_message(stdout: &mut impl Write, value: &Value) -> Result<(), TokenFoldError> {
    writeln!(stdout, "{value}").map_err(TokenFoldError::Io)?;
    stdout.flush().map_err(TokenFoldError::Io)
}

/// Returns `None` for notifications (no `id`), which per JSON-RPC 2.0 never get a response —
/// this is how `notifications/initialized` is handled, with no special-case on method name.
fn handle_request(request: &Value) -> Option<Value> {
    let id = request.get("id").cloned()?;
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => Ok(handle_initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(handle_tools_list()),
        "tools/call" => handle_tools_call(&params),
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    })
}

fn handle_initialize(params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "tokenfold", "version": env!("CARGO_PKG_VERSION")},
    })
}

fn tool_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "content": {
                "type": "string",
                "description": "Raw text payload to compress. Exactly one of `content`/`messages` is required.",
            },
            "messages": {
                "type": "array",
                "items": {"type": "object"},
                "description": "Chat messages array (OpenAI/Anthropic style). Exactly one of `content`/`messages` is required.",
            },
            "format": {
                "type": "string",
                "enum": ["auto", "openai_json", "anthropic_json", "plain_text", "command_output", "git_diff"],
            },
            "mode": {"type": "string", "enum": ["conservative", "balanced", "aggressive"]},
            "target_tokens": {"type": "integer", "minimum": 0},
            "store_originals": {
                "type": "boolean",
                "description": "F-045 local retrieval store now exists (see `tokenfold_retrieve`), but this tool doesn't persist to it yet; currently has no effect here.",
            },
        },
    })
}

fn retrieve_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "hash": {
                "type": "string",
                "description": "Lowercase hex SHA-256 hash of the stored payload.",
            },
            "marker": {
                "type": "string",
                "description": "A `[tokenfold:retrieve hash=<hex> alg=sha256 namespace=<ns> bytes=<n> ttl=<seconds>]` marker string.",
            },
            "report_ref": {
                "type": "string",
                "description": "Reserved; not resolvable in this pass (RetrievalReport carries no per-entry content hash yet).",
            },
        },
    })
}

fn stats_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "scope": {"type": "string", "enum": ["session", "project", "user"]},
            "window": {
                "type": "string",
                "description": "Duration shorthand like \"30d\", \"24h\", \"90m\", \"120s\", or a bare integer of seconds.",
            },
        },
    })
}

fn handle_tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "tokenfold_compress",
                "description": "Compress a text payload or chat-message array to reduce LLM token usage.",
                "inputSchema": tool_input_schema(),
            },
            {
                "name": "tokenfold_inspect",
                "description": "Dry-run preview of achievable token savings; never returns a modified payload.",
                "inputSchema": tool_input_schema(),
            },
            {
                "name": "tokenfold_retrieve",
                "description": "Restore an original payload previously persisted to the local retrieval store, by hash, marker, or report reference. Missing/expired retrieval is explicit, never partial.",
                "inputSchema": retrieve_input_schema(),
            },
            {
                "name": "tokenfold_stats",
                "description": "Aggregate token-savings statistics from the local ledger. Never returns raw payloads or originals.",
                "inputSchema": stats_input_schema(),
            },
        ]
    })
}

fn handle_tools_call(params: &Value) -> Result<Value, (i32, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    match name {
        "tokenfold_compress" => Ok(call_compress(&arguments, false)),
        "tokenfold_inspect" => Ok(call_compress(&arguments, true)),
        "tokenfold_retrieve" => Ok(call_retrieve(&arguments)),
        "tokenfold_stats" => Ok(call_stats(&arguments)),
        other => Err((-32602, format!("unknown tool: {other}"))),
    }
}

/// Tool-level failures (bad arguments, compression errors) are reported as a successful
/// JSON-RPC result with `isError: true`, per MCP convention — only request-shape problems
/// (missing/unknown tool name, in `handle_tools_call`) are JSON-RPC protocol errors.
fn call_compress(arguments: &Value, is_inspect: bool) -> Value {
    match run_compress(arguments, is_inspect) {
        Ok(value) => tool_success(value),
        Err(message) => tool_error(message),
    }
}

fn run_compress(arguments: &Value, is_inspect: bool) -> Result<Value, String> {
    let (input, from_messages) = build_input(arguments)?;
    let policy = build_policy(arguments)?;
    let output = tokenfold_core::compress(input, &policy).map_err(|e| e.to_string())?;

    if is_inspect {
        let preview: String = String::from_utf8_lossy(&output.bytes)
            .chars()
            .take(2000)
            .collect();
        Ok(json!({"report": output.report, "preview": preview}))
    } else if from_messages {
        let value: Value = serde_json::from_slice(&output.bytes).map_err(|e| e.to_string())?;
        let messages = value.get("messages").cloned().unwrap_or_else(|| json!([]));
        Ok(json!({"messages": messages, "report": output.report}))
    } else {
        let content = String::from_utf8_lossy(&output.bytes).into_owned();
        Ok(json!({"content": content, "report": output.report}))
    }
}

/// Returns the compression input plus whether it came from `messages` (vs `content`), so the
/// caller gets the same shape back that it sent.
fn build_input(arguments: &Value) -> Result<(CompressionInput, bool), String> {
    let content_present = arguments.get("content").is_some_and(|v| !v.is_null());
    let messages_present = arguments.get("messages").is_some_and(|v| !v.is_null());

    let explicit_format = match arguments.get("format").and_then(Value::as_str) {
        Some(s) => Some(FormatArg::parse(s).map(FormatArg::to_input_format)?),
        None => None,
    };

    match (content_present, messages_present) {
        (true, true) => {
            Err("exactly one of `content` or `messages` is required, not both".to_string())
        }
        (false, false) => Err("exactly one of `content` or `messages` is required".to_string()),
        (true, false) => {
            let text = arguments["content"]
                .as_str()
                .ok_or("`content` must be a string")?;
            let bytes = text.as_bytes().to_vec();
            let format = explicit_format
                .filter(|f| *f != InputFormat::Auto)
                .unwrap_or_else(|| crate::format::detect_format(&bytes, false));
            Ok((CompressionInput { format, bytes }, false))
        }
        (false, true) => {
            let messages = arguments["messages"]
                .as_array()
                .ok_or("`messages` must be an array")?;
            let bytes =
                serde_json::to_vec(&json!({"messages": messages})).map_err(|e| e.to_string())?;
            let format = explicit_format
                .filter(|f| *f != InputFormat::Auto)
                .unwrap_or_else(|| crate::format::detect_format(&bytes, false));
            Ok((CompressionInput { format, bytes }, true))
        }
    }
}

fn build_policy(arguments: &Value) -> Result<CompressionPolicy, String> {
    let mode = match arguments.get("mode").and_then(Value::as_str) {
        Some(s) => ModeArg::parse(s)?.to_core(),
        None => CompressionMode::Balanced,
    };
    let mut builder = CompressionPolicy::builder().mode(mode);
    if let Some(t) = arguments.get("target_tokens").and_then(Value::as_u64) {
        builder = builder.target_tokens(t as usize);
    }
    builder.build().map_err(|e| e.to_string())
}

/// F-045: opens the local retrieval store `tokenfold_retrieve` reads from. This file never
/// parses `tokenfold.toml` (see the top-of-file doc comment), so — consistently with that
/// existing scope cut — only the same-named environment overrides `tokenfold-cli::config`
/// already documents are honored here, defaulting to the standard filesystem store.
fn retrieval_store_from_env() -> Result<RetrievalStore, String> {
    let backend =
        std::env::var("TOKENFOLD_RETRIEVAL_BACKEND").unwrap_or_else(|_| "filesystem".to_string());
    let store_path = std::env::var_os("TOKENFOLD_RETRIEVAL_STORE_PATH").map(PathBuf::from);
    RetrievalStore::open(&backend, "sha256", store_path).map_err(|e| e.to_string())
}

/// Tool-level failures (bad arguments, an unopenable store) are `isError: true` results, per the
/// same convention as `call_compress`.
fn call_retrieve(arguments: &Value) -> Value {
    match run_retrieve(arguments) {
        Ok(value) => tool_success(value),
        Err(message) => tool_error(message),
    }
}

/// Precedence when more than one of `hash`/`marker`/`report_ref` is given: `marker` (it carries
/// its own namespace), then `hash` (looked up under the `"default"` namespace — the tool schema
/// has no namespace field of its own), then `report_ref`.
fn run_retrieve(arguments: &Value) -> Result<Value, String> {
    let marker = arguments.get("marker").and_then(Value::as_str);
    let hash_arg = arguments.get("hash").and_then(Value::as_str);
    let report_ref = arguments.get("report_ref").and_then(Value::as_str);

    let (hash, namespace) = if let Some(marker) = marker {
        let hash = extract_marker_field(marker, "hash")
            .ok_or_else(|| "retrieval marker has no hash=<hex> field".to_string())?;
        let namespace =
            extract_marker_field(marker, "namespace").unwrap_or_else(|| "default".to_string());
        (hash, namespace)
    } else if let Some(hash) = hash_arg {
        (hash.to_string(), "default".to_string())
    } else if report_ref.is_some() {
        // `RetrievalReport` (report.rs) carries no per-entry content hash, so a report
        // reference alone can never be resolved to a stored hash in the current schema —
        // same limitation `tokenfold retrieve <report-path>` already reports (see
        // `main.rs::cmd_retrieve`). No file is read here to reach that conclusion.
        return Err(
            "report_ref resolution is not implemented: RetrievalReport carries no per-entry \
             content hash in the current schema; retrieve by `hash` or `marker` instead"
                .to_string(),
        );
    } else {
        return Err("at least one of `hash`, `marker`, or `report_ref` is required".to_string());
    };

    let store = retrieval_store_from_env()?;
    Ok(retrieve_outcome_to_value(store.retrieve(&hash, &namespace)))
}

/// `source` is honestly always `"local_mcp"`: this tool only ever reads the local retrieval
/// store, never a proxy-side one.
fn retrieve_outcome_to_value(outcome: RetrievalOutcome) -> Value {
    match outcome {
        RetrievalOutcome::Found(bytes) => json!({
            "status": "found",
            "source": "local_mcp",
            "content": String::from_utf8_lossy(&bytes).into_owned(),
        }),
        RetrievalOutcome::Missing => json!({"status": "missing", "source": "local_mcp"}),
        RetrievalOutcome::Expired => json!({"status": "expired", "source": "local_mcp"}),
    }
}

/// Extracts `field=<value>` from a `[tokenfold:retrieve ...]` marker string (INTERFACES.md's
/// Retrieval Marker Grammar), stopping at the next whitespace or `]`.
fn extract_marker_field(marker: &str, field: &str) -> Option<String> {
    let needle = format!("{field}=");
    let start = marker.find(&needle)? + needle.len();
    let rest = &marker[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ']')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn call_stats(arguments: &Value) -> Value {
    match run_stats(arguments) {
        Ok(value) => tool_success(value),
        Err(message) => tool_error(message),
    }
}

/// F-046: aggregates the local ledger (no ad-hoc report-glob support here — the tool schema is
/// just `{ scope?, window? }`) through the one shared `tokenfold_core::stats::aggregate` path.
/// Never touches raw payload bytes: `StatsSummary` structurally carries none.
fn run_stats(arguments: &Value) -> Result<Value, String> {
    let scope = match arguments.get("scope").and_then(Value::as_str) {
        Some(s @ ("session" | "project" | "user")) => s.to_string(),
        Some(other) => {
            return Err(format!(
                "unknown scope: {other:?}; expected \"session\", \"project\", or \"user\""
            ));
        }
        None => "project".to_string(),
    };
    let window = arguments.get("window").and_then(Value::as_str);

    let ledger_path = std::env::var_os("TOKENFOLD_ANALYTICS_LEDGER_DB")
        .map(PathBuf::from)
        .unwrap_or_else(LedgerStore::default_path);
    let all = LedgerStore::new(ledger_path)
        .read_all()
        .map_err(|e| e.to_string())?;

    let records = match window {
        Some(w) => {
            let window_secs = stats::parse_duration_secs(w).map_err(|e| e.to_string())?;
            stats::filter_since(&all, stats::now_unix(), window_secs)
        }
        None => all,
    };

    let mut summary = stats::aggregate(&records);
    summary.scope = scope;
    summary.window = window.unwrap_or("all").to_string();
    serde_json::to_value(&summary).map_err(|e| e.to_string())
}

fn tool_success(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({"content": [{"type": "text", "text": text}], "structuredContent": value, "isError": false})
}

fn tool_error(message: String) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}
