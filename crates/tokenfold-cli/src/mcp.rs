//! Minimal MCP (Model Context Protocol) stdio server: `tokenfold mcp serve`.
//!
//! Implements the `tokenfold_compress`, `tokenfold_inspect`, `tokenfold_retrieve`,
//! and `tokenfold_stats` tools. The backing stores for the latter two now exist
//! (`tokenfold_core::retrieval_store`/`tokenfold_core::stats`), so they're wired
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
//! Recoverable pruning and retrieval share a host-configured filesystem root and namespace
//! authorization. Tool-supplied alternate roots are refused. MCP never loads project config.
//!
//! Transport is newline-delimited JSON-RPC 2.0 over stdio, the standard MCP stdio framing: one
//! JSON object per line in, one per line out. stdout carries only JSON-RPC messages; logs (none
//! currently) would go to stderr. No MCP SDK dependency: the subset of the protocol this server
//! needs (`initialize`, `tools/list`, `tools/call`, notification handling) is a few dozen lines
//! of `serde_json` over stdin/stdout.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};
use tokenfold_core::retrieval_store::{
    RetrievalOutcome, RetrievalStore, parse_retrieval_reference,
};
use tokenfold_core::stats::{self, LedgerStore};
use tokenfold_core::{
    CompressionInput, CompressionPolicy, InputFormat, OutputEncoding, Preset, PruningPolicy,
    TokenFoldError,
};

use crate::args::PresetArg;
use crate::format::FormatArg;

const PROTOCOL_VERSION: &str = "2025-11-25";

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
                "enum": ["auto", "json", "openai_json", "anthropic_json", "plain_text", "command_output", "git_diff"],
            },
            "preset": {"type": "string", "enum": ["conservative", "balanced", "aggressive"]},
            "target_tokens": {"type": "integer", "minimum": 1},
            "encoding": {"type": "string", "enum": ["json", "toon"]},
            "pruning": {
                "type": "object",
                "properties": {
                    "keep_ratio": {"type": "number", "exclusiveMinimum": 0, "maximum": 1},
                    "preserve_paths": {"type": "array", "items": {"type": "string"}},
                    "retrieval_store": {"type": "string"},
                    "retrieval_namespace": {"type": "string"}
                }
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
                "description": "A legacy `[tokenfold:retrieve ...]` marker or serialized JSON `$tf_ref` marker.",
            },
            "namespace": {
                "type": "string",
                "description": "Namespace override. Takes precedence over a namespace embedded in `marker`.",
            },
            "report_ref": {
                "type": "string",
                "description": "Reserved; not resolvable in this pass (RetrievalReport carries no per-entry content hash yet).",
            },
            "query": {"type":"string", "description":"Search host-approved evidence instead of exact lookup. Requires TOKENFOLD_RETRIEVAL_SEARCH_MANIFEST."},
            "top_k": {"type":"integer", "minimum":1, "maximum":20},
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
                "description": "Restore an original payload previously persisted to the local retrieval store, by hash or marker. Missing/expired retrieval is explicit, never partial; report references are reserved but not yet resolvable.",
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
    let policy = build_policy(arguments, is_inspect)?;
    let output = tokenfold_core::compress(input, &policy).map_err(|e| e.to_string())?;

    if is_inspect {
        Ok(json!({"report": output.report}))
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

fn build_policy(arguments: &Value, is_inspect: bool) -> Result<CompressionPolicy, String> {
    if arguments.get("store_originals").is_some() {
        return Err("store_originals was removed; use the typed pruning policy".to_string());
    }
    let preset = match arguments.get("preset").and_then(Value::as_str) {
        Some(s) => PresetArg::parse(s)?.to_core(),
        None => Preset::Balanced,
    };
    let mut builder = CompressionPolicy::builder()
        .preset(preset)
        .preview(is_inspect)
        .retrieval_max_store_bytes(env_u64("TOKENFOLD_RETRIEVAL_MAX_STORE_BYTES")?);
    if let Some(t) = arguments.get("target_tokens").and_then(Value::as_u64) {
        builder = builder.target_tokens(t as usize);
    }
    if let Some(encoding) = arguments.get("encoding").and_then(Value::as_str) {
        builder = builder.encoding(match encoding {
            "json" => OutputEncoding::Json,
            "toon" => OutputEncoding::Toon,
            value => return Err(format!("unknown encoding: {value}")),
        });
    }
    if let Some(pruning) = arguments.get("pruning") {
        // Store location and authorization belong to the host, not model-supplied arguments.
        let trusted_store = std::env::var_os("TOKENFOLD_RETRIEVAL_STORE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(tokenfold_core::retrieval_store::default_store_path);
        if pruning
            .get("retrieval_store")
            .and_then(Value::as_str)
            .is_some_and(|path| *path != trusted_store)
        {
            return Err("pruning retrieval_store must match the host-configured store".into());
        }
        let namespace = pruning
            .get("retrieval_namespace")
            .and_then(Value::as_str)
            .unwrap_or("default");
        let authorized = authorized_namespaces_from_env();
        if !authorized.is_empty() && !authorized.iter().any(|allowed| allowed == namespace) {
            return Err("pruning namespace is unauthorized".into());
        }
        let backend =
            std::env::var("TOKENFOLD_RETRIEVAL_BACKEND").unwrap_or_else(|_| "filesystem".into());
        if backend != "filesystem" {
            return Err("MCP pruning requires a persistent filesystem retrieval backend".into());
        }
        let preserve_paths = pruning
            .get("preserve_paths")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or("preserve_paths must contain only strings".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        builder = builder.pruning(PruningPolicy {
            keep_ratio: pruning.get("keep_ratio").and_then(Value::as_f64),
            preserve_paths,
            retrieval_store: Some(trusted_store),
            retrieval_namespace: Some(namespace.to_owned()),
        });
    }
    builder.build().map_err(|e| e.to_string())
}

/// Opens the local retrieval store `tokenfold_retrieve` reads from. This file never
/// parses `tokenfold.toml` (see the top-of-file doc comment), so — consistently with that
/// existing scope cut — only the same-named environment overrides `tokenfold-cli::config`
/// already documents are honored here, defaulting to the standard filesystem store.
fn env_u64(name: &str) -> Result<Option<u64>, String> {
    std::env::var(name)
        .ok()
        .map(|raw| {
            raw.trim()
                .parse::<u64>()
                .map_err(|_| format!("invalid non-negative integer for {name}"))
        })
        .transpose()
}

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
    if arguments.get("query").is_some() {
        return run_evidence_search(arguments);
    }
    let marker = arguments.get("marker").and_then(Value::as_str);
    let hash_arg = arguments.get("hash").and_then(Value::as_str);
    let report_ref = arguments.get("report_ref").and_then(Value::as_str);

    let explicit_namespace = arguments.get("namespace").and_then(Value::as_str);
    let reference = if let Some(marker) = marker {
        parse_retrieval_reference(marker).map_err(|error| error.to_string())?
    } else if let Some(hash) = hash_arg {
        parse_retrieval_reference(hash).map_err(|error| error.to_string())?
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
    let namespace = explicit_namespace
        .map(str::to_string)
        .or(reference.namespace)
        .unwrap_or_else(|| "default".to_string());

    let store = retrieval_store_from_env()?;
    Ok(retrieve_outcome_to_value(store.retrieve_authorized(
        &reference.hash,
        &namespace,
        &authorized_namespaces_from_env(),
        max_restore_bytes_from_env()?,
    )))
}

// Publication is a host decision. A model cannot enumerate arbitrary store contents or
// choose a manifest path. Rebuilding each query synchronizes expiry/deletion after restart.
fn run_evidence_search(arguments: &Value) -> Result<Value, String> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .filter(|q| !q.trim().is_empty() && q.len() <= 4096)
        .ok_or("invalid search query")?;
    let namespace = arguments
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let allowed = authorized_namespaces_from_env();
    if !allowed.is_empty() && !allowed.iter().any(|n| n == namespace) {
        return Ok(json!({"status":"unauthorized", "source":"local_mcp"}));
    }
    let path = std::env::var_os("TOKENFOLD_RETRIEVAL_SEARCH_MANIFEST")
        .ok_or("evidence search is not enabled by the host")?;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("search manifest exceeds 64 KiB".into());
    }
    let manifest: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if manifest.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported search manifest version".into());
    }
    let hashes = manifest
        .get("namespaces")
        .and_then(|v| v.get(namespace))
        .and_then(Value::as_array);
    let store = retrieval_store_from_env()?;
    let mut approved = Vec::new();
    let mut indexed_bytes = 0usize;
    if let Some(hashes) = hashes {
        if hashes.len() > 512 {
            return Err("search manifest exceeds 512 entries per namespace".into());
        }
        for hash in hashes {
            let hash = hash.as_str().ok_or("manifest hashes must be strings")?;
            if parse_retrieval_reference(hash)
                .map_err(|e| e.to_string())?
                .hash
                != hash
            {
                return Err("manifest requires bare SHA-256 hashes".into());
            }
            if approved.iter().any(|h| h == hash) {
                continue;
            }
            if let RetrievalOutcome::Found(bytes) =
                store.retrieve_authorized(hash, namespace, &allowed, Some(16384))
            {
                indexed_bytes += bytes.len();
                if indexed_bytes > 1048576 {
                    return Err("search index exceeds 1 MiB".into());
                }
                approved.push(hash.to_owned());
            }
        }
    }
    let top_k = match arguments.get("top_k") {
        None => 5,
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=20).contains(n))
            .ok_or("top_k must be 1..20")? as usize,
    };
    let (index, _) = tokenfold_rag::EvidenceIndex::build(&store, namespace, &approved, 16384);
    let mut remaining = env_u64("TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES")?
        .unwrap_or(65536)
        .min(65536) as usize;
    let mut hits = Vec::new();
    let mut over_budget = false;
    for hit in index.search(&store, query, top_k, &allowed) {
        // Whole entries only: no snippet can masquerade as a complete original row.
        if hit.text.len() > remaining {
            over_budget = true;
            continue;
        }
        remaining -= hit.text.len();
        hits.push(
            json!({"hash":hit.hash, "namespace":namespace, "content":hit.text, "score":hit.score}),
        );
    }
    Ok(
        json!({"status":if over_budget {"over_budget"} else if hits.is_empty() {"no_match"} else {"found"},
        "source":"local_mcp", "hits":hits}),
    )
}

/// The namespaces this MCP server is permitted to read, from
/// `TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES` (comma-separated).
///
/// Unset or empty means unrestricted, which is the pre-EP-05 behavior. This file deliberately
/// never parses `tokenfold.toml` (see the top-of-file doc comment), so the authorization boundary
/// is configured through the same environment channel that already selects the store itself.
fn authorized_namespaces_from_env() -> Vec<String> {
    std::env::var("TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// Per-retrieval byte budget. Malformed configured limits fail closed.
fn max_restore_bytes_from_env() -> Result<Option<usize>, String> {
    env_u64("TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES")?
        .map(|n| usize::try_from(n).map_err(|_| "restore budget exceeds platform limits".into()))
        .transpose()
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
        // `unauthorized` deliberately omits any hint about the hash, so a host cannot use this
        // tool to discover what exists in a namespace it was not granted.
        RetrievalOutcome::Unauthorized => json!({"status": "unauthorized", "source": "local_mcp"}),
        // `over_budget` reports the whole-entry size against the caller's own limit so it can
        // retry deliberately. The content is never partially returned.
        RetrievalOutcome::OverBudget { bytes, limit_bytes } => json!({
            "status": "over_budget",
            "source": "local_mcp",
            "bytes": bytes,
            "limit_bytes": limit_bytes,
        }),
    }
}

fn call_stats(arguments: &Value) -> Value {
    match run_stats(arguments) {
        Ok(value) => tool_success(value),
        Err(message) => tool_error(message),
    }
}

/// Aggregates the local ledger (no ad-hoc report-glob support here — the tool schema is
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
