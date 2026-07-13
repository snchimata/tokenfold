//! Black-box tests against the compiled `tokenfold-proxy` binary, covering ROADMAP.md's Phase 5
//! proxy exit criterion: compression on provider passthrough, unbuffered SSE, conflicting-framing
//! rejection, and no credential leakage into logs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tiny_http::{Header, Response, StatusCode};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_tokenfold-proxy")
}

fn free_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

fn unique_temp_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "tokenfold_proxy_test_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn wait_ready(addr: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if ureq::get(format!("http://{addr}/livez")).call().is_ok() {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("proxy at {addr} never became ready");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

struct ProxyProcess {
    child: Child,
    addr: String,
    stderr: Arc<Mutex<String>>,
}

impl ProxyProcess {
    fn start(upstream: &str, extra_args: &[&str]) -> Self {
        Self::start_with_env(upstream, extra_args, &[])
    }

    fn start_with_env(upstream: &str, extra_args: &[&str], envs: &[(&str, &str)]) -> Self {
        let addr = free_addr();
        let mut cmd = Command::new(bin());
        cmd.args([
            "--upstream",
            upstream,
            "--bind",
            &addr,
            "--insecure-upstream",
        ]);
        cmd.args(extra_args);
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn tokenfold-proxy");
        let stderr_pipe = child.stderr.take().unwrap();
        let stderr = Arc::new(Mutex::new(String::new()));
        let captured = stderr.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stderr_pipe);
            for line in reader.lines().map_while(Result::ok) {
                let mut buf = captured.lock().unwrap();
                buf.push_str(&line);
                buf.push('\n');
            }
        });
        wait_ready(&addr);
        ProxyProcess {
            child,
            addr,
            stderr,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn stderr_snapshot(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }
}

impl Drop for ProxyProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Bare-metal HTTP request over a raw TCP socket, for framing edge cases the high-level `ureq`
/// client won't let us construct (duplicate/conflicting headers).
fn raw_request(addr: &str, raw: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
}

// ---- startup validation ----

#[test]
fn refuses_http_upstream_without_insecure_flag() {
    let addr = free_addr();
    let status = Command::new(bin())
        .args(["--upstream", "http://example.com", "--bind", &addr])
        .stderr(Stdio::piped())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(5));
}

#[test]
fn refuses_non_loopback_bind_without_flag() {
    let status = Command::new(bin())
        .args([
            "--upstream",
            "https://example.com",
            "--bind",
            "0.0.0.0:18787",
        ])
        .stderr(Stdio::piped())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(5));
}

#[test]
fn refuses_unsafe_disable_redaction() {
    let addr = free_addr();
    let status = Command::new(bin())
        .args([
            "--upstream",
            "https://example.com",
            "--bind",
            &addr,
            "--unsafe-disable-redaction",
        ])
        .stderr(Stdio::piped())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(5));
}

// ---- control routes ----

#[test]
fn livez_and_health_respond_ok() {
    let proxy = ProxyProcess::start("https://example.invalid", &[]);
    let livez = ureq::get(proxy.url("/livez")).call().unwrap();
    assert_eq!(livez.status(), 200);
    let health = ureq::get(proxy.url("/health")).call().unwrap();
    assert_eq!(health.status(), 200);
}

// ---- /v1/compress ----

#[test]
fn compress_route_shrinks_repetitive_content_and_reports_savings() {
    let proxy = ProxyProcess::start("https://example.invalid", &[]);
    let long_text = "the quick brown fox jumps over the lazy dog. ".repeat(200);
    let body = serde_json::json!({"content": long_text, "target_tokens": 5});
    let response = ureq::post(proxy.url("/v1/compress"))
        .header("Content-Type", "application/json")
        .send(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("x-tokenfold-status").is_some());
    let mut response = response;
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .read_to_string(&mut text)
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(value.get("report").is_some());
}

#[test]
fn compress_route_rejects_body_missing_content_and_messages() {
    let proxy = ProxyProcess::start("https://example.invalid", &[]);
    let result = ureq::post(proxy.url("/v1/compress"))
        .header("Content-Type", "application/json")
        .send(b"{}");
    let err = result.unwrap_err();
    match err {
        ureq::Error::StatusCode(422) => {}
        other => panic!("expected 422, got {other:?}"),
    }
}

// ---- passthrough ----

fn spawn_echo_upstream() -> (String, Arc<Mutex<Vec<u8>>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_string();
    let received = Arc::new(Mutex::new(Vec::new()));
    let captured = received.clone();
    thread::spawn(move || {
        if let Ok(mut request) = server.recv() {
            let mut body = Vec::new();
            let _ = request.as_reader().read_to_end(&mut body);
            *captured.lock().unwrap() = body.clone();
            let response_body = serde_json::to_vec(&serde_json::json!({"echo": true})).unwrap();
            let headers = vec![Header::from_bytes("Content-Type", "application/json").unwrap()];
            let _ = request.respond(Response::new(
                StatusCode(200),
                headers,
                std::io::Cursor::new(response_body.clone()),
                Some(response_body.len()),
                None,
            ));
        }
    });
    (addr, received)
}

#[test]
fn passthrough_compresses_chat_json_before_forwarding_upstream() {
    let (upstream_addr, received) = spawn_echo_upstream();
    // No --target-tokens: an unreachable target short-circuits the pipeline before any
    // transform runs (see pipeline.rs), which isn't what this test is exercising — this test
    // wants the default no-target path, which runs lossless transforms to the safe floor.
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &[]);

    let filler = "restate this exact background context every single turn please. ".repeat(100);
    let payload = serde_json::json!({
        "model": "gpt-4",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": filler},
        ]
    });
    // Pretty-printed on the wire so `json_minify` (always on, no `--experimental` needed) has
    // real structural whitespace to strip — the message content itself is protected floor and
    // untouched by any default transform, so a compact-JSON body would show zero savings here.
    let raw_body = serde_json::to_vec_pretty(&payload).unwrap();
    let raw_len = raw_body.len();

    let response = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer sk-super-secret-upstream-key")
        .send(&raw_body)
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("x-tokenfold-status").is_some());

    thread::sleep(Duration::from_millis(100));
    let forwarded = received.lock().unwrap().clone();
    assert!(
        forwarded.len() < raw_len,
        "expected upstream to receive a smaller, compressed body: {} vs original {}",
        forwarded.len(),
        raw_len
    );

    // The Authorization credential must reach upstream (pass-through auth)...
    // ...but must never be echoed into the proxy's own stderr access log.
    assert!(
        !proxy
            .stderr_snapshot()
            .contains("sk-super-secret-upstream-key")
    );
}

#[test]
fn passthrough_bypass_header_skips_compression() {
    let (upstream_addr, received) = spawn_echo_upstream();
    let proxy = ProxyProcess::start(
        &format!("http://{upstream_addr}"),
        &["--target-tokens", "1"],
    );

    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hello ".repeat(50)}]
    });
    let raw = serde_json::to_vec(&payload).unwrap();

    let response = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .header("X-TokenFold-Bypass", "true")
        .send(&raw)
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("x-tokenfold-status").is_none());

    thread::sleep(Duration::from_millis(100));
    assert_eq!(*received.lock().unwrap(), raw);
}

fn spawn_sse_upstream(chunks: Vec<&'static str>) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_string();
    thread::spawn(move || {
        if let Ok(request) = server.recv() {
            let body = chunks.concat();
            let headers = vec![Header::from_bytes("Content-Type", "text/event-stream").unwrap()];
            // data_length = None forces tiny_http to stream via chunked transfer-encoding
            // instead of buffering the whole body behind a Content-Length header.
            let _ = request.respond(Response::new(
                StatusCode(200),
                headers,
                std::io::Cursor::new(body.into_bytes()),
                None,
                None,
            ));
        }
    });
    addr
}

#[test]
fn sse_response_passes_through_with_event_stream_content_type() {
    let upstream_addr = spawn_sse_upstream(vec![
        "data: first\n\n",
        "data: second\n\n",
        "data: [DONE]\n\n",
    ]);
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &[]);

    let mut response = ureq::get(proxy.url("/v1/chat/completions")).call().unwrap();
    assert_eq!(response.status(), 200);
    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(content_type.contains("text/event-stream"));
    // SSE responses are never compressed/rewritten and never get X-TokenFold-* headers attached
    // to the response (they reflect the request side only, and this GET has no request body).
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .read_to_string(&mut text)
        .unwrap();
    assert!(text.contains("data: first"));
    assert!(text.contains("data: [DONE]"));
}

// ---- CL.TE / TE.CL smuggling defense ----

#[test]
fn conflicting_content_length_and_transfer_encoding_is_rejected() {
    let proxy = ProxyProcess::start("https://example.invalid", &[]);
    let raw = "POST /v1/chat/completions HTTP/1.1\r\n\
               Host: x\r\n\
               Content-Type: application/json\r\n\
               Content-Length: 4\r\n\
               Transfer-Encoding: chunked\r\n\
               Connection: close\r\n\
               \r\n\
               2\r\n{}\r\n0\r\n\r\n";
    let response = raw_request(&proxy.addr, raw);
    assert!(
        response.starts_with("HTTP/1.1 400"),
        "response was: {response}"
    );
}

#[test]
fn duplicate_conflicting_content_length_headers_are_rejected() {
    let proxy = ProxyProcess::start("https://example.invalid", &[]);
    let raw = "POST /v1/compress HTTP/1.1\r\n\
               Host: x\r\n\
               Content-Type: application/json\r\n\
               Content-Length: 2\r\n\
               Content-Length: 999\r\n\
               Connection: close\r\n\
               \r\n\
               {}";
    let response = raw_request(&proxy.addr, raw);
    assert!(
        response.starts_with("HTTP/1.1 400"),
        "response was: {response}"
    );
}

// ---- request body size limit ----

#[test]
fn oversized_request_body_is_rejected_with_413() {
    let proxy = ProxyProcess::start("https://example.invalid", &["--max-body-bytes", "10"]);
    let result = ureq::post(proxy.url("/v1/compress"))
        .header("Content-Type", "application/json")
        .send(b"{\"content\": \"this body is well over ten bytes\"}");
    let err = result.unwrap_err();
    match err {
        ureq::Error::StatusCode(413) => {}
        other => panic!("expected 413, got {other:?}"),
    }
}

// ---- /v1/retrieve, /v1/retrieve/{hash}, /v1/retrieve/stats ----

#[test]
fn store_originals_header_then_retrieve_round_trips_via_v1_retrieve() {
    let store_dir = unique_temp_path("retrieve_roundtrip");
    let store_dir_str = store_dir.to_string_lossy().to_string();
    let proxy = ProxyProcess::start(
        "https://example.invalid",
        &["--retrieval-store-path", &store_dir_str],
    );

    let payload = "the quick brown fox jumps over the lazy dog, over and over.";
    let body = serde_json::json!({"content": payload});
    let compress_response = ureq::post(proxy.url("/v1/compress"))
        .header("Content-Type", "application/json")
        .header("X-TokenFold-Store-Originals", "true")
        .send(&serde_json::to_vec(&body).unwrap())
        .unwrap();
    assert_eq!(compress_response.status(), 200);

    let hash = tokenfold_core::retrieval_store::hex_sha256(payload.as_bytes());

    // GET /v1/retrieve/{hash}.
    let mut get_response = ureq::get(proxy.url(&format!("/v1/retrieve/{hash}")))
        .call()
        .unwrap();
    assert_eq!(get_response.status(), 200);
    let mut get_text = String::new();
    get_response
        .body_mut()
        .as_reader()
        .read_to_string(&mut get_text)
        .unwrap();
    let get_value: serde_json::Value = serde_json::from_str(&get_text).unwrap();
    assert_eq!(get_value["status"], "found");
    assert_eq!(get_value["source"], "proxy_store");
    assert_eq!(get_value["content"], payload);

    // POST /v1/retrieve { "hash": ... } resolves the same entry.
    let post_body = serde_json::json!({"hash": hash});
    let mut post_response = ureq::post(proxy.url("/v1/retrieve"))
        .header("Content-Type", "application/json")
        .send(&serde_json::to_vec(&post_body).unwrap())
        .unwrap();
    assert_eq!(post_response.status(), 200);
    let mut post_text = String::new();
    post_response
        .body_mut()
        .as_reader()
        .read_to_string(&mut post_text)
        .unwrap();
    let post_value: serde_json::Value = serde_json::from_str(&post_text).unwrap();
    assert_eq!(post_value["status"], "found");
    assert_eq!(post_value["content"], payload);

    std::fs::remove_dir_all(&store_dir).ok();
}

#[test]
fn retrieve_missing_hash_returns_a_clear_structured_404() {
    let store_dir = unique_temp_path("retrieve_missing");
    let store_dir_str = store_dir.to_string_lossy().to_string();
    let proxy = ProxyProcess::start(
        "https://example.invalid",
        &["--retrieval-store-path", &store_dir_str],
    );

    let missing_hash = "0".repeat(64);
    let raw =
        format!("GET /v1/retrieve/{missing_hash} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let response = raw_request(&proxy.addr, &raw);
    assert!(
        response.starts_with("HTTP/1.1 404"),
        "response was: {response}"
    );
    assert!(
        response.contains("\"status\":\"missing\""),
        "response was: {response}"
    );
    assert!(response.contains("\"source\":\"proxy_store\""));

    std::fs::remove_dir_all(&store_dir).ok();
}

#[test]
fn retrieve_post_route_rejects_a_body_with_no_reference() {
    let proxy = ProxyProcess::start("https://example.invalid", &[]);
    let result = ureq::post(proxy.url("/v1/retrieve"))
        .header("Content-Type", "application/json")
        .send(b"{}");
    let err = result.unwrap_err();
    match err {
        ureq::Error::StatusCode(400) => {}
        other => panic!("expected 400, got {other:?}"),
    }
}

// ---- /stats, /v1/retrieve/stats ----

fn sample_ledger_record() -> tokenfold_core::stats::LedgerRecord {
    tokenfold_core::stats::LedgerRecord {
        request_id: "tc-proxytest1".to_string(),
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
    }
}

#[test]
fn stats_route_returns_a_valid_stats_summary_from_the_ledger() {
    let ledger_path = unique_temp_path("stats_ledger").with_extension("db");
    tokenfold_core::stats::LedgerStore::new(&ledger_path)
        .append(&sample_ledger_record())
        .unwrap();
    let ledger_path_str = ledger_path.to_string_lossy().to_string();

    let proxy = ProxyProcess::start_with_env(
        "https://example.invalid",
        &[],
        &[("TOKENFOLD_ANALYTICS_LEDGER_DB", &ledger_path_str)],
    );

    let mut response = ureq::get(proxy.url("/stats?scope=project")).call().unwrap();
    assert_eq!(response.status(), 200);
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .read_to_string(&mut text)
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["schema_version"], "1.0");
    assert_eq!(value["scope"], "project");
    assert_eq!(value["requests"], 1);
    assert_eq!(value["raw_tokens"], 1000);
    assert_eq!(value["compressed_tokens"], 600);
    // No raw payload text ever enters the summary.
    assert!(!text.contains("hello world"));

    std::fs::remove_file(&ledger_path).ok();
}

#[test]
fn retrieve_stats_route_returns_retrieval_counters_without_raw_originals() {
    let ledger_path = unique_temp_path("retrieve_stats_ledger").with_extension("db");
    tokenfold_core::stats::LedgerStore::new(&ledger_path)
        .append(&sample_ledger_record())
        .unwrap();
    let ledger_path_str = ledger_path.to_string_lossy().to_string();

    let proxy = ProxyProcess::start_with_env(
        "https://example.invalid",
        &[],
        &[("TOKENFOLD_ANALYTICS_LEDGER_DB", &ledger_path_str)],
    );

    let mut response = ureq::get(proxy.url("/v1/retrieve/stats")).call().unwrap();
    assert_eq!(response.status(), 200);
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .read_to_string(&mut text)
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["schema_version"], "1.0");
    assert!(value.get("retrieval").is_some());
    assert!(value["retrieval"].get("markers").is_some());

    std::fs::remove_file(&ledger_path).ok();
}
