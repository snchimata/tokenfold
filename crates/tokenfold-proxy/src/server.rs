//! Request handling for `tokenfold-proxy`.
//!
//! Scope is the Phase 5 exit criterion (ROADMAP.md F-040): compress provider-shaped JSON
//! requests before forwarding upstream, pass SSE responses through unbuffered, reject
//! conflicting-framing (CL.TE/TE.CL) requests before any upstream call, and never log a
//! credential header value. The full route table in INTERFACES.md §3.0 also documents
//! `/v1/retrieve`, `/v1/retrieve/{hash}`, `/v1/retrieve/stats`, and `/stats` — those now exist
//! below, backed by the same `tokenfold_core::retrieval_store`/`tokenfold_core::stats` modules
//! the CLI uses (F-045/F-046).
//!
//! Deliberately still deferred (see `handle`'s route dispatch): `/stats-history`, `/metrics`
//! (Prometheus), `/dashboard`, `/stats/reset`, `/cache/clear`, and the `/admin/*`/`/debug/*` dev
//! surfaces. These are observability/ops extras beyond the Phase 5 exit criteria's core ask
//! ("proxy contract complete and accurate", not "every route implemented") — a Prometheus
//! exporter, a static dashboard, and an admin-auth surface are materially more scope than what
//! those two exit criteria require.

use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, StatusCode};
use tokenfold_core::budget::CompressionPolicyBuilder;
use tokenfold_core::report::{CompressionReport, TransformStatus};
use tokenfold_core::retrieval_store::{RetrievalOutcome, RetrievalStore};
use tokenfold_core::status::Status;
use tokenfold_core::{CompressionInput, CompressionMode, CompressionPolicy, InputFormat};

type BodyReader = Box<dyn Read>;

pub struct ProxyConfig {
    pub upstream: String,
    pub max_body_bytes: usize,
    pub compress: bool,
    pub mode: CompressionMode,
    pub target_tokens: Option<usize>,
    /// F-045: backend passed to `RetrievalStore::open` for `/v1/retrieve*` and
    /// `X-TokenFold-Store-Originals`-triggered storage on `/v1/compress`/passthrough.
    pub retrieval_backend: String,
    /// F-045: filesystem backend root override; `None` means `retrieval_store::default_store_path()`.
    pub retrieval_store_path: Option<PathBuf>,
}

pub fn run(config: &ProxyConfig, server: &tiny_http::Server) {
    for request in server.incoming_requests() {
        let start = Instant::now();
        let method = request.method().as_str().to_string();
        let path = request.url().split('?').next().unwrap_or("").to_string();
        let status = handle(config, request);
        log_access(&method, &path, status, start.elapsed());
    }
}

fn handle(config: &ProxyConfig, mut request: Request) -> u16 {
    let method = request.method().clone();
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let headers = request.headers().to_vec();

    if let Err(message) = check_framing(&headers) {
        let (status, resp) = error_response(400, &message);
        let _ = request.respond(resp);
        return status;
    }

    let (status, resp) = match (method.clone(), path.as_str()) {
        (Method::Get, "/livez") => text_response(200, "ok"),
        (Method::Get, "/readyz") => {
            json_response(200, &json!({"ready": true, "upstream": config.upstream}))
        }
        (Method::Get, "/health") => {
            json_response(200, &json!({"status": "ok", "upstream": config.upstream}))
        }
        (Method::Post, "/v1/compress") => match read_body(&mut request, config.max_body_bytes) {
            Ok(body) => handle_compress(config, &headers, &body),
            Err(status) => error_response(status, "request body exceeds max_body_bytes"),
        },
        (Method::Post, "/v1/retrieve") => match read_body(&mut request, config.max_body_bytes) {
            Ok(body) => handle_retrieve_post(config, &headers, &body),
            Err(status) => error_response(status, "request body exceeds max_body_bytes"),
        },
        (Method::Get, "/v1/retrieve/stats") => handle_retrieve_stats(),
        (Method::Get, p) if p.starts_with("/v1/retrieve/") => {
            handle_retrieve_get(config, &headers, p)
        }
        (Method::Get, "/stats") => handle_stats(&url),
        _ => match read_body(&mut request, config.max_body_bytes) {
            Ok(body) => handle_passthrough(config, method, &url, &headers, body),
            Err(status) => error_response(status, "request body exceeds max_body_bytes"),
        },
    };
    let _ = request.respond(resp);
    status
}

fn log_access(method: &str, path: &str, status: u16, elapsed: Duration) {
    // ponytail: fixed-field access log only, never interpolates header values or query strings —
    // credential values structurally cannot appear here regardless of what a client sends.
    eprintln!("{method} {path} -> {status} ({}ms)", elapsed.as_millis());
}

/// Rejects requests with conflicting framing (multiple differing `Content-Length` values, or
/// both `Content-Length` and `Transfer-Encoding` present) before any upstream request is sent —
/// the standard CL.TE/TE.CL request-smuggling defense.
fn check_framing(headers: &[Header]) -> Result<(), String> {
    let content_lengths: Vec<&str> = headers
        .iter()
        .filter(|h| {
            h.field
                .as_str()
                .as_str()
                .eq_ignore_ascii_case("content-length")
        })
        .map(|h| h.value.as_str())
        .collect();
    let has_transfer_encoding = headers.iter().any(|h| {
        h.field
            .as_str()
            .as_str()
            .eq_ignore_ascii_case("transfer-encoding")
    });

    if content_lengths.iter().any(|v| *v != content_lengths[0]) {
        return Err("conflicting Content-Length headers".to_string());
    }
    if has_transfer_encoding && !content_lengths.is_empty() {
        return Err("conflicting Content-Length and Transfer-Encoding headers".to_string());
    }
    Ok(())
}

fn read_body(request: &mut Request, max_bytes: usize) -> Result<Vec<u8>, u16> {
    let mut buf = Vec::new();
    let mut limited = request.as_reader().take(max_bytes as u64 + 1);
    limited.read_to_end(&mut buf).map_err(|_| 400u16)?;
    if buf.len() > max_bytes {
        return Err(413);
    }
    Ok(buf)
}

fn header_value<'a>(headers: &'a [Header], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

// ---- /v1/compress ----

fn handle_compress(
    config: &ProxyConfig,
    headers: &[Header],
    body: &[u8],
) -> (u16, Response<BodyReader>) {
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return error_response(400, &format!("invalid JSON body: {e}")),
    };
    match run_compress(config, headers, &value) {
        Ok((response_value, report)) => {
            let (status, mut resp) = json_response(200, &response_value);
            let request_id = request_id_for(header_value(headers, "x-tokenfold-request-id"));
            for header in report_headers(&report, request_id) {
                resp.add_header(header);
            }
            (status, resp)
        }
        Err(message) => error_response(422, &message),
    }
}

fn run_compress(
    config: &ProxyConfig,
    headers: &[Header],
    value: &Value,
) -> Result<(Value, CompressionReport), String> {
    let (input, from_messages) = build_input(value)?;
    let policy = build_policy(config, headers, value)?;
    let output = tokenfold_core::compress(input, &policy).map_err(|e| e.to_string())?;
    let report = output.report.clone();
    if from_messages {
        let parsed: Value = serde_json::from_slice(&output.bytes).map_err(|e| e.to_string())?;
        let messages = parsed.get("messages").cloned().unwrap_or_else(|| json!([]));
        Ok((
            json!({"messages": messages, "report": output.report}),
            report,
        ))
    } else {
        let content = String::from_utf8_lossy(&output.bytes).into_owned();
        Ok((json!({"content": content, "report": output.report}), report))
    }
}

/// Same `content`/`messages` shape as the MCP `tokenfold_compress` tool (INTERFACES.md §4), so
/// callers get identical semantics across both surfaces.
fn build_input(value: &Value) -> Result<(CompressionInput, bool), String> {
    let content_present = value.get("content").is_some_and(|v| !v.is_null());
    let messages_present = value.get("messages").is_some_and(|v| !v.is_null());
    let explicit_format = parse_format(value.get("format").and_then(Value::as_str))?;

    match (content_present, messages_present) {
        (true, true) => {
            Err("exactly one of `content` or `messages` is required, not both".to_string())
        }
        (false, false) => Err("exactly one of `content` or `messages` is required".to_string()),
        (true, false) => {
            let text = value["content"]
                .as_str()
                .ok_or("`content` must be a string")?;
            let bytes = text.as_bytes().to_vec();
            let format = explicit_format.unwrap_or_else(|| detect_format(&bytes));
            Ok((CompressionInput { format, bytes }, false))
        }
        (false, true) => {
            let messages = value["messages"]
                .as_array()
                .ok_or("`messages` must be an array")?;
            let bytes =
                serde_json::to_vec(&json!({"messages": messages})).map_err(|e| e.to_string())?;
            let format = explicit_format.unwrap_or_else(|| detect_format(&bytes));
            Ok((CompressionInput { format, bytes }, true))
        }
    }
}

fn parse_format(raw: Option<&str>) -> Result<Option<InputFormat>, String> {
    match raw {
        None | Some("auto") => Ok(None),
        Some("openai_json") | Some("openai") => Ok(Some(InputFormat::OpenAiJson)),
        Some("anthropic_json") | Some("anthropic") => Ok(Some(InputFormat::AnthropicJson)),
        Some("plain_text") | Some("text") => Ok(Some(InputFormat::PlainText)),
        Some("command_output") | Some("command") => Ok(Some(InputFormat::CommandOutput)),
        Some("git_diff") | Some("diff") => Ok(Some(InputFormat::GitDiff)),
        Some(other) => Err(format!("unknown format: {other}")),
    }
}

fn build_policy(
    config: &ProxyConfig,
    headers: &[Header],
    value: &Value,
) -> Result<CompressionPolicy, String> {
    let mode = match value.get("mode").and_then(Value::as_str) {
        Some("conservative") => CompressionMode::Conservative,
        Some("balanced") => CompressionMode::Balanced,
        Some("aggressive") => CompressionMode::Aggressive,
        Some(other) => return Err(format!("unknown mode: {other}")),
        None => config.mode,
    };
    let mut builder = CompressionPolicy::builder().mode(mode);
    let target = value
        .get("target_tokens")
        .and_then(Value::as_u64)
        .map(|t| t as usize)
        .or(config.target_tokens);
    if let Some(target) = target {
        builder = builder.target_tokens(target);
    }
    builder = apply_retrieval_overrides(builder, config, headers);
    builder.build().map_err(|e| e.to_string())
}

/// Wires `X-TokenFold-Store-Originals`/`X-TokenFold-Retrieve-Store` (INTERFACES.md §3.1) into a
/// policy builder, so a per-request opt-in to the F-045 retrieval store works on both
/// `/v1/compress` (via `build_policy`) and provider passthrough (`handle_passthrough`).
fn apply_retrieval_overrides(
    mut builder: CompressionPolicyBuilder,
    config: &ProxyConfig,
    headers: &[Header],
) -> CompressionPolicyBuilder {
    let store_originals = header_value(headers, "x-tokenfold-store-originals")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    builder = builder
        .store_originals(store_originals)
        .retrieval_backend(config.retrieval_backend.clone())
        .retrieval_store_path(config.retrieval_store_path.clone());
    if let Some(namespace) = header_value(headers, "x-tokenfold-retrieve-store") {
        builder = builder.retrieval_namespace(namespace);
    }
    builder
}

/// Conservative auto-detection for chat-shaped JSON only (`messages` array present). Unlike the
/// CLI's `detect_format`, this never falls back to `PlainText`/`GitDiff` for the passthrough
/// path — non-chat JSON bodies (embeddings, moderation, …) are forwarded untouched rather than
/// risking a transform built for chat payloads mangling an unrelated shape.
fn detect_format(bytes: &[u8]) -> InputFormat {
    let has_messages = serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|v| {
            v.get("messages")
                .and_then(|m| m.as_array())
                .map(|m| !m.is_empty())
        })
        .unwrap_or(false);
    if has_messages {
        InputFormat::OpenAiJson
    } else {
        InputFormat::PlainText
    }
}

fn detect_passthrough_format(bytes: &[u8]) -> Option<InputFormat> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let obj = value.as_object()?;
    let messages = obj.get("messages")?.as_array()?;
    if messages.is_empty() {
        return None;
    }
    Some(if obj.contains_key("system") {
        InputFormat::AnthropicJson
    } else {
        InputFormat::OpenAiJson
    })
}

// ---- provider passthrough ----

fn handle_passthrough(
    config: &ProxyConfig,
    method: Method,
    url: &str,
    headers: &[Header],
    body: Vec<u8>,
) -> (u16, Response<BodyReader>) {
    let target = format!("{}{}", config.upstream, url);
    let content_type = header_value(headers, "content-type").unwrap_or("");
    let bypassed =
        header_value(headers, "x-tokenfold-bypass").is_some_and(|v| v.eq_ignore_ascii_case("true"));

    let mut forward_body = body;
    let mut report: Option<CompressionReport> = None;
    if config.compress
        && !bypassed
        && content_type.to_ascii_lowercase().contains("json")
        && let Some(format) = detect_passthrough_format(&forward_body)
    {
        let mut builder = CompressionPolicy::builder().mode(config.mode);
        if let Some(target) = config.target_tokens {
            builder = builder.target_tokens(target);
        }
        builder = apply_retrieval_overrides(builder, config, headers);
        if let Ok(policy) = builder.build() {
            let input = CompressionInput {
                format,
                bytes: forward_body.clone(),
            };
            if let Ok(output) = tokenfold_core::compress(input, &policy) {
                forward_body = output.bytes;
                report = Some(output.report);
            }
        }
    }

    let request_id = request_id_for(header_value(headers, "x-tokenfold-request-id"));
    match send_upstream(&target, method, headers, &forward_body) {
        Ok(response) => build_upstream_response(response, report.as_ref(), request_id),
        Err(e) => {
            eprintln!("upstream request error: {e}");
            error_response(502, "upstream request failed")
        }
    }
}

fn should_forward_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    ) && !name.to_ascii_lowercase().starts_with("x-tokenfold-")
}

fn send_upstream(
    url: &str,
    method: Method,
    headers: &[Header],
    body: &[u8],
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    match method {
        Method::Post => with_headers(ureq::post(url), headers).send(body),
        Method::Put => with_headers(ureq::put(url), headers).send(body),
        Method::Patch => with_headers(ureq::patch(url), headers).send(body),
        Method::Delete => with_headers(ureq::delete(url), headers).call(),
        Method::Head => with_headers(ureq::head(url), headers).call(),
        Method::Options => with_headers(ureq::options(url), headers).call(),
        _ => with_headers(ureq::get(url), headers).call(),
    }
}

fn with_headers<B>(
    mut builder: ureq::RequestBuilder<B>,
    headers: &[Header],
) -> ureq::RequestBuilder<B> {
    for header in headers {
        let name = header.field.as_str().as_str();
        if should_forward_header(name) {
            builder = builder.header(name, header.value.as_str());
        }
    }
    builder
}

fn build_upstream_response(
    response: ureq::http::Response<ureq::Body>,
    report: Option<&CompressionReport>,
    request_id: String,
) -> (u16, Response<BodyReader>) {
    let status = response.status().as_u16();
    let is_success = (200..300).contains(&status);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let is_streaming = content_type.contains("text/event-stream")
        || response.headers().get("content-length").is_none();

    let mut out_headers: Vec<Header> = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str();
            if !should_forward_header(name) {
                return None;
            }
            let value = value.to_str().ok()?;
            Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
        })
        .collect();
    if is_success && let Some(report) = report {
        out_headers.extend(report_headers(report, request_id));
    }

    let body = response.into_body();
    if is_streaming {
        let reader: BodyReader = Box::new(body.into_reader());
        (
            status,
            Response::new(StatusCode(status), out_headers, reader, None, None),
        )
    } else {
        let mut buf = Vec::new();
        let _ = body.into_reader().read_to_end(&mut buf);
        out_headers.push(Header::from_bytes("Content-Length", buf.len().to_string()).unwrap());
        let len = buf.len();
        (
            status,
            Response::new(
                StatusCode(status),
                out_headers,
                Box::new(Cursor::new(buf)) as BodyReader,
                Some(len),
                None,
            ),
        )
    }
}

// ---- /v1/retrieve, /v1/retrieve/{hash}, /v1/retrieve/stats ----

fn open_retrieval_store(config: &ProxyConfig) -> Result<RetrievalStore, String> {
    RetrievalStore::open(
        &config.retrieval_backend,
        "sha256",
        config.retrieval_store_path.clone(),
    )
    .map_err(|e| e.to_string())
}

/// `X-TokenFold-Retrieve-Store` selects the namespace to look a bare `hash` up under (a marker
/// carries its own namespace); falls back to `"default"`, the same fallback
/// `CompressionPolicyBuilder::build` uses when nothing sets `retrieval_namespace`.
fn retrieval_namespace_header(headers: &[Header]) -> String {
    header_value(headers, "x-tokenfold-retrieve-store")
        .map(str::to_string)
        .unwrap_or_else(|| "default".to_string())
}

/// Precedence when more than one of `marker`/`hash`/`report_ref` is given: `marker` (it carries
/// its own namespace), then `hash`, then `report_ref` — same precedence as the MCP
/// `tokenfold_retrieve` tool (see `tokenfold-cli::mcp::run_retrieve`).
fn resolve_retrieve_reference(
    value: &Value,
    default_namespace: &str,
) -> Result<(String, String), String> {
    let marker = value.get("marker").and_then(Value::as_str);
    let hash = value.get("hash").and_then(Value::as_str);
    let report_ref = value.get("report_ref").and_then(Value::as_str);

    if let Some(marker) = marker {
        let hash = extract_marker_field(marker, "hash")
            .ok_or("retrieval marker has no hash=<hex> field")?;
        let namespace = extract_marker_field(marker, "namespace")
            .unwrap_or_else(|| default_namespace.to_string());
        return Ok((hash, namespace));
    }
    if let Some(hash) = hash {
        return Ok((hash.to_string(), default_namespace.to_string()));
    }
    if report_ref.is_some() {
        // `RetrievalReport` (tokenfold_core::report) carries no per-entry content hash, so a
        // report reference alone can never be resolved to a stored hash in the current schema —
        // same limitation the CLI's `tokenfold retrieve <report-path>` already reports.
        return Err(
            "report_ref resolution is not implemented: RetrievalReport carries no per-entry \
             content hash in the current schema; retrieve by `hash` or `marker` instead"
                .to_string(),
        );
    }
    Err("exactly one of `marker`, `hash`, or `report_ref` is required".to_string())
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

/// `source` is honestly `"proxy_store"`: this is the proxy's own retrieval store, not the MCP
/// server's. `found` is `200`; `missing`/`expired` are non-200 (`404`/`410`) so a caller never
/// has to inspect the body to tell success from absence.
fn retrieve_response(outcome: RetrievalOutcome) -> (u16, Response<BodyReader>) {
    match outcome {
        RetrievalOutcome::Found(bytes) => json_response(
            200,
            &json!({
                "status": "found",
                "source": "proxy_store",
                "content": String::from_utf8_lossy(&bytes).into_owned(),
            }),
        ),
        RetrievalOutcome::Missing => {
            json_response(404, &json!({"status": "missing", "source": "proxy_store"}))
        }
        RetrievalOutcome::Expired => {
            json_response(410, &json!({"status": "expired", "source": "proxy_store"}))
        }
    }
}

fn handle_retrieve_post(
    config: &ProxyConfig,
    headers: &[Header],
    body: &[u8],
) -> (u16, Response<BodyReader>) {
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return error_response(400, &format!("invalid JSON body: {e}")),
    };
    let default_namespace = retrieval_namespace_header(headers);
    let (hash, namespace) = match resolve_retrieve_reference(&value, &default_namespace) {
        Ok(pair) => pair,
        Err(message) => return error_response(400, &message),
    };
    let store = match open_retrieval_store(config) {
        Ok(store) => store,
        Err(message) => return error_response(500, &message),
    };
    retrieve_response(store.retrieve(&hash, &namespace))
}

fn handle_retrieve_get(
    config: &ProxyConfig,
    headers: &[Header],
    path: &str,
) -> (u16, Response<BodyReader>) {
    let hash = path.trim_start_matches("/v1/retrieve/");
    if hash.is_empty() {
        return error_response(400, "missing retrieval hash in path");
    }
    let namespace = retrieval_namespace_header(headers);
    let store = match open_retrieval_store(config) {
        Ok(store) => store,
        Err(message) => return error_response(500, &message),
    };
    retrieve_response(store.retrieve(hash, &namespace))
}

/// `RetrievalStore`'s public API (tokenfold_core::retrieval_store) has no entry-count/byte-total
/// accessor — `gc()` is the only thing that inspects stored entries in bulk, and it's
/// destructive. So "retrieval store stats" here honestly means the ledger-derived
/// `StatsSummary.retrieval` counters (`markers`/`hits`/`misses`/`expired`) `tokenfold_core::stats`
/// already tracks, not literal on-disk store totals — consistent with `stats.rs`'s own
/// zero-when-no-honest-source pattern (`hits`/`misses`/`expired` stay `0` there today).
fn handle_retrieve_stats() -> (u16, Response<BodyReader>) {
    let summary = tokenfold_core::stats::aggregate(&ledger_records());
    json_response(
        200,
        &json!({
            "schema_version": tokenfold_core::stats::SCHEMA_VERSION,
            "retrieval": summary.retrieval,
        }),
    )
}

// ---- /stats ----

fn ledger_records() -> Vec<tokenfold_core::stats::LedgerRecord> {
    let path = std::env::var_os("TOKENFOLD_ANALYTICS_LEDGER_DB")
        .map(PathBuf::from)
        .unwrap_or_else(tokenfold_core::stats::LedgerStore::default_path);
    tokenfold_core::stats::LedgerStore::new(path)
        .read_all()
        .unwrap_or_default()
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, val) = pair.split_once('=')?;
        (key == name).then(|| val.to_string())
    })
}

fn handle_stats(url: &str) -> (u16, Response<BodyReader>) {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let scope = query_param(query, "scope");
    let window = query_param(query, "window");

    let all = ledger_records();
    let records = match &window {
        Some(w) => match tokenfold_core::stats::parse_duration_secs(w) {
            Ok(secs) => {
                tokenfold_core::stats::filter_since(&all, tokenfold_core::stats::now_unix(), secs)
            }
            Err(e) => return error_response(400, &e.to_string()),
        },
        None => all,
    };

    let mut summary = tokenfold_core::stats::aggregate(&records);
    if let Some(scope) = scope {
        summary.scope = scope;
    }
    if let Some(window) = window {
        summary.window = window;
    }
    let value = serde_json::to_value(&summary).unwrap_or_default();
    json_response(200, &value)
}

// ---- shared response/report helpers ----

fn json_response(status: u16, value: &Value) -> (u16, Response<BodyReader>) {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let len = bytes.len();
    let headers = vec![Header::from_bytes("Content-Type", "application/json").unwrap()];
    (
        status,
        Response::new(
            StatusCode(status),
            headers,
            Box::new(Cursor::new(bytes)) as BodyReader,
            Some(len),
            None,
        ),
    )
}

fn text_response(status: u16, body: &str) -> (u16, Response<BodyReader>) {
    let bytes = body.as_bytes().to_vec();
    let len = bytes.len();
    (
        status,
        Response::new(
            StatusCode(status),
            vec![],
            Box::new(Cursor::new(bytes)) as BodyReader,
            Some(len),
            None,
        ),
    )
}

fn error_response(status: u16, message: &str) -> (u16, Response<BodyReader>) {
    json_response(status, &json!({"error": message}))
}

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generates a `tc-<8 hex>` request id (no dependency on `rand`: a monotonic counter mixed with
/// wall-clock nanos is unique enough for log correlation, which is all this is used for).
fn generate_request_id() -> String {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "tc-{:08x}",
        (seed ^ counter.wrapping_mul(0x9E37_79B9)) as u32
    )
}

fn request_id_for(client_supplied: Option<&str>) -> String {
    client_supplied
        .map(str::to_string)
        .unwrap_or_else(generate_request_id)
}

fn report_headers(report: &CompressionReport, request_id: String) -> Vec<Header> {
    let applied: Vec<&str> = report
        .transforms
        .iter()
        .filter(|t| t.status == TransformStatus::Applied)
        .map(|t| t.id.as_str())
        .collect();
    let applied_versions: Vec<String> = report
        .transforms
        .iter()
        .filter(|t| t.status == TransformStatus::Applied)
        .map(|t| format!("{}@{}", t.id, t.version))
        .collect();
    let estimator = match &report.estimator.model {
        Some(model) => format!("{}:{model}", report.estimator.backend),
        None => report.estimator.backend.clone(),
    };

    [
        (
            "X-TokenFold-Status",
            status_label(&report.status).to_string(),
        ),
        (
            "X-TokenFold-Original-Tokens",
            report.original_tokens.to_string(),
        ),
        (
            "X-TokenFold-Compressed-Tokens",
            report.compressed_tokens.to_string(),
        ),
        (
            "X-TokenFold-Savings-Pct",
            format!("{:.1}", report.savings_pct),
        ),
        ("X-TokenFold-Estimator", estimator),
        ("X-TokenFold-Applied", applied.join(",")),
        ("X-TokenFold-Applied-Versions", applied_versions.join(",")),
        ("X-TokenFold-Request-Id", request_id),
        ("X-TokenFold-Mode", report.mode.clone()),
        ("X-TokenFold-Format", report.format.clone()),
    ]
    .into_iter()
    .filter_map(|(name, value)| Header::from_bytes(name.as_bytes(), value.as_bytes()).ok())
    .collect()
}

fn status_label(status: &Status) -> &'static str {
    match status {
        Status::Compressed => "compressed",
        Status::Passthrough => "passthrough",
        Status::BestEffort => "best_effort",
        Status::UnreachableTarget => "unreachable_target",
    }
}
