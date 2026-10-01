//! Black-box tests against the compiled `tokenfold-proxy` binary, covering the proxy's four
//! release criteria: compression on provider passthrough, unbuffered SSE, conflicting-framing
//! rejection, and no credential leakage into logs.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tiny_http::{Header, Response, StatusCode};
use tokenfold_core::measurement::{
    CompletionState, MeasurementEvent, ModelResolution, UsageDisposition, opaque_id,
};

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

/// Waits for the proxy's startup line and returns the address it reports actually binding.
///
/// Reading the bound address back from the child (rather than guessing a free port up front) is
/// what makes concurrent test runs safe: the kernel's assignment cannot be stolen in between.
fn wait_for_bind_addr(stderr: &Arc<Mutex<String>>) -> String {
    const PREFIX: &str = "tokenfold-proxy listening on ";
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = stderr.lock().unwrap().clone();
        if let Some(addr) = snapshot.lines().find_map(|line| {
            line.strip_prefix(PREFIX)
                .and_then(|rest| rest.split(" -> ").next())
                .map(str::to_string)
        }) {
            return addr;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "proxy never reported a bound address; stderr was:\n{snapshot}"
        );
        thread::sleep(Duration::from_millis(20));
    }
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
        // Let the kernel pick the port and read back what it actually bound. Picking a port here
        // and releasing it was a race: the proxy then had to re-bind a port that was free only
        // momentarily, and under parallel runs two proxies could take the same one, leaving a test
        // talking to a sibling's proxy (or to nothing) while `wait_ready` was satisfied by it.
        let mut cmd = Command::new(bin());
        cmd.args([
            "--upstream",
            upstream,
            "--bind",
            "127.0.0.1:0",
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
        let addr = wait_for_bind_addr(&stderr);
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
    // Windows intermittently drops these deliberately malformed sockets when several raw
    // clients close concurrently. The behavior under test is server framing, not client load.
    static RAW_REQUEST_LOCK: Mutex<()> = Mutex::new(());
    let _guard = RAW_REQUEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..3 {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut response = String::new();
        // A reset after a partial read still yields the status line, so keep whatever arrived
        // instead of discarding it along with the error.
        let _ = stream.read_to_string(&mut response);
        if !response.is_empty() {
            return response;
        }
        thread::sleep(Duration::from_millis(20));
    }
    String::new()
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

// --- tool-result observations (opt-in, route-gated) -------------------------

/// A chat request whose assistant turn requested one tool call, answered by a result with enough
/// repeated structure for the data transforms to actually shrink it.
fn chat_with_one_tool_result() -> serde_json::Value {
    let items: Vec<serde_json::Value> = (0..12)
        .map(|i| {
            serde_json::json!({
                "id": format!("row-{i:03}"),
                "host": "worker-07",
                "state": "ready",
                "attempts": 3,
                "note": "processed batch with no anomalies detected in this shard"
            })
        })
        .collect();
    serde_json::json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "summarize the queue"},
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_0",
                    "type": "function",
                    "function": {"name": "queue_status", "arguments": "{}"}
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_0",
                "content": serde_json::to_string(
                    &serde_json::json!({"tool": "queue.status", "items": items})
                )
                .unwrap()
            }
        ]
    })
}

#[test]
fn observations_are_off_unless_the_operator_opts_in() {
    let (upstream_addr, received) = spawn_echo_upstream();
    // No --observations: the default must be a byte-for-byte forward, because the feature rewrites
    // request bodies and must never activate itself.
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &[]);
    let raw_body = serde_json::to_vec(&chat_with_one_tool_result()).unwrap();

    let _ = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(&raw_body[..])
        .unwrap();

    let forwarded = received.lock().unwrap().clone();
    assert_eq!(
        forwarded, raw_body,
        "observations rewrote a request without being enabled"
    );
}

#[test]
fn enabling_observations_shrinks_only_the_tool_result() {
    let (upstream_addr, received) = spawn_echo_upstream();
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &["--observations"]);
    let baseline = chat_with_one_tool_result();
    let raw_body = serde_json::to_vec(&baseline).unwrap();

    let _ = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(&raw_body[..])
        .unwrap();

    let forwarded = received.lock().unwrap().clone();
    assert!(
        forwarded.len() < raw_body.len(),
        "expected a smaller forwarded body: {} vs {}",
        forwarded.len(),
        raw_body.len()
    );

    // The envelope survives intact; only the result string is rewritten.
    let candidate: serde_json::Value = serde_json::from_slice(&forwarded).unwrap();
    assert_eq!(candidate["model"], baseline["model"]);
    assert_eq!(candidate["messages"][0], baseline["messages"][0]);
    assert_eq!(candidate["messages"][1], baseline["messages"][1]);
    assert_eq!(candidate["messages"][2]["tool_call_id"], "call_0");
    assert!(
        candidate["messages"][2]["content"].as_str().unwrap().len()
            < baseline["messages"][2]["content"].as_str().unwrap().len()
    );
}

#[test]
fn the_threshold_flag_suppresses_small_results() {
    let (upstream_addr, received) = spawn_echo_upstream();
    let raw_body = serde_json::to_vec(&chat_with_one_tool_result()).unwrap();
    // Above the request's own size, so nothing qualifies.
    let threshold = (raw_body.len() + 1).to_string();
    let proxy = ProxyProcess::start(
        &format!("http://{upstream_addr}"),
        &[
            "--observations",
            "--observation-min-content-bytes",
            &threshold,
        ],
    );

    let _ = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(&raw_body[..])
        .unwrap();

    assert_eq!(received.lock().unwrap().clone(), raw_body);
}

#[test]
fn an_unactivated_route_never_consults_the_observation_adapter() {
    let (upstream_addr, received) = spawn_echo_upstream();
    // Observations enabled, but this route is not the one the adapter owns. The body is a valid
    // chat payload, so a body-sniffing implementation would have rewritten it -- which is exactly
    // the guess this gate exists to prevent.
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &["--observations"]);
    // A small body the pre-existing whole-body path leaves alone, so the only thing that could
    // change it is the observation adapter.
    let raw_body = serde_json::to_vec(&serde_json::json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "hello"}]
    }))
    .unwrap();

    let _ = ureq::post(proxy.url("/v1/messages"))
        .header("Content-Type", "application/json")
        .send(&raw_body[..])
        .unwrap();

    assert_eq!(received.lock().unwrap().clone(), raw_body);
    // The adapter is not merely a no-op here, it is not called: a kept baseline would still be
    // explained on stderr, and its absence is what proves the route gate ran.
    let stderr = proxy.stderr_snapshot();
    assert!(
        !stderr.contains("observation kept baseline"),
        "the observation adapter ran on an unactivated route; stderr was:\n{stderr}"
    );
}

#[test]
fn a_kept_baseline_is_forwarded_and_explained() {
    let (upstream_addr, received) = spawn_echo_upstream();
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &["--observations"]);
    // No tool result at all: nothing to compress, so the request is a normal one.
    let raw_body = serde_json::to_vec(&serde_json::json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "hello"}]
    }))
    .unwrap();

    let response = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(&raw_body[..])
        .unwrap();
    assert_eq!(response.status(), 200);

    assert_eq!(received.lock().unwrap().clone(), raw_body);
    let stderr = proxy.stderr_snapshot();
    assert!(
        stderr.contains("observation kept baseline"),
        "a kept baseline must be explainable; stderr was:\n{stderr}"
    );
}

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

#[test]
fn held_open_sse_does_not_block_health_checks() {
    struct SlowBody(bool);
    impl Read for SlowBody {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.0 {
                self.0 = true;
                let chunk = b"data: first\n\n";
                buf[..chunk.len()].copy_from_slice(chunk);
                return Ok(chunk.len());
            }
            thread::sleep(Duration::from_secs(2));
            Ok(0)
        }
    }

    let upstream = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let upstream_addr = upstream.server_addr().to_string();
    thread::spawn(move || {
        if let Ok(request) = upstream.recv() {
            let headers = vec![Header::from_bytes("Content-Type", "text/event-stream").unwrap()];
            let _ = request.respond(Response::new(
                StatusCode(200),
                headers,
                SlowBody(false),
                None,
                None,
            ));
        }
    });
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &[]);
    let stream_url = proxy.url("/v1/chat/completions");
    let stream = thread::spawn(move || {
        let mut response = ureq::get(stream_url).call().unwrap();
        let mut body = String::new();
        response
            .body_mut()
            .as_reader()
            .read_to_string(&mut body)
            .unwrap();
        body
    });

    thread::sleep(Duration::from_millis(200));
    let start = std::time::Instant::now();
    assert_eq!(ureq::get(proxy.url("/livez")).call().unwrap().status(), 200);
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(stream.join().unwrap().contains("data: first"));
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

#[test]
fn a_refused_request_with_a_body_returns_the_refusal_instead_of_resetting() {
    // Regression: the proxy used to reject conflicting framing without reading the request body.
    // Closing a socket that still holds unread inbound bytes makes the peer reset the connection,
    // which discards the 400 the proxy had already written — the client saw a transport fault
    // instead of a refusal, intermittently, and could not tell them apart.
    let proxy = ProxyProcess::start("https://example.invalid", &[]);
    let raw = "POST /v1/chat/completions HTTP/1.1\r\n\
               Host: x\r\n\
               Content-Type: application/json\r\n\
               Content-Length: 4\r\n\
               Transfer-Encoding: chunked\r\n\
               Connection: close\r\n\
               \r\n\
               2\r\n{}\r\n0\r\n\r\n";

    // The OS still discards an already-written response now and then on this host, so a single
    // connection cannot be the oracle and neither can a demand for a perfect score. What matters
    // is that the refusal is deliverable at all: before the drain, zero of these arrived.
    let mut delivered = 0;
    for _ in 0..10 {
        let mut stream = TcpStream::connect(&proxy.addr).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        if response.starts_with("HTTP/1.1 400") {
            delivered += 1;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        delivered >= 5,
        "the refusal must be deliverable, not lost to a connection reset; delivered {delivered}/10"
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
    let post_body = serde_json::json!({"hash": &hash});
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

    let json_marker = serde_json::json!({
        "$tf_ref": {"hash": hash, "alg": "sha256", "namespace": "default"}
    });
    let marker_body = serde_json::json!({"marker": json_marker.to_string()});
    let mut marker_response = ureq::post(proxy.url("/v1/retrieve"))
        .header("Content-Type", "application/json")
        .send(&serde_json::to_vec(&marker_body).unwrap())
        .unwrap();
    let mut marker_text = String::new();
    marker_response
        .body_mut()
        .as_reader()
        .read_to_string(&mut marker_text)
        .unwrap();
    let marker_value: serde_json::Value = serde_json::from_str(&marker_text).unwrap();
    assert_eq!(marker_value["content"], payload);

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
        preset: "balanced".to_string(),
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

// ---- measurement events: exactly one terminal event per provider attempt ----

/// Polls the proxy's stderr until `expected` measurement events have arrived. Any stderr line
/// that claims to be an event must parse, so a schema drift fails loudly here.
fn wait_for_events(proxy: &ProxyProcess, expected: usize) -> Vec<MeasurementEvent> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let events = proxy
            .stderr_snapshot()
            .lines()
            .filter_map(tokenfold_core::measurement::parse_event_line)
            .collect::<Result<Vec<MeasurementEvent>, _>>()
            .expect("every emitted measurement line must parse");
        if events.len() >= expected || std::time::Instant::now() > deadline {
            return events;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn body_text(mut response: ureq::http::Response<ureq::Body>) -> String {
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .read_to_string(&mut text)
        .unwrap();
    text
}

/// Serves one request with a complete JSON body (explicit Content-Length, so the proxy buffers it
/// instead of streaming it).
fn spawn_json_upstream(body: &'static str, content_type: &'static str) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_string();
    thread::spawn(move || {
        if let Ok(request) = server.recv() {
            let headers = vec![Header::from_bytes("Content-Type", content_type).unwrap()];
            let _ = request.respond(Response::new(
                StatusCode(200),
                headers,
                std::io::Cursor::new(body.as_bytes().to_vec()),
                Some(body.len()),
                None,
            ));
        }
    });
    addr
}

fn chat_request(model: &str, stream: bool) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "hello there"}],
        "stream": stream
    }))
    .unwrap()
}

#[test]
fn streamed_sse_usage_emits_exactly_one_measurement_event() {
    let upstream_addr = spawn_sse_upstream(vec![
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n",
        "data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n",
        "data: [DONE]\n\n",
    ]);
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &[]);
    let response = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .header("X-TokenFold-Session-Id", "sess-abc")
        .send(&chat_request("gpt-4o-mini", true))
        .unwrap();
    let text = body_text(response);
    assert!(
        text.contains("data: [DONE]"),
        "stream must be forwarded verbatim"
    );

    let events = wait_for_events(&proxy, 1);
    assert_eq!(
        events.len(),
        1,
        "one terminal event per attempt, got {events:?}"
    );
    let event = &events[0];
    assert_eq!(event.completion, CompletionState::Completed);
    assert_eq!(event.usage_disposition, UsageDisposition::Reported);
    // The repeated cumulative snapshot is merged, never summed.
    assert_eq!(event.provider_usage.unwrap().total(), Some(15));
    assert_eq!(event.attempt, 1);
    assert_eq!(
        event.model,
        Some(ModelResolution::Known {
            model: "gpt-4o-mini".to_string(),
            tokenizer: "o200k_base".to_string()
        })
    );
    // Local counts come from the compression report, and the delta is the provider's prompt count
    // minus the local input-side count (never its completion tokens).
    let local_after = event
        .local_after_tokens
        .expect("compression report carries local counts");
    assert_eq!(event.provider_delta_tokens, Some(10 - local_after as i64));
    assert!(event.estimator.is_some());
    assert!(
        event
            .policy_revision
            .as_deref()
            .is_none_or(|revision| revision.contains('@')),
        "a policy revision is an id@version list: {:?}",
        event.policy_revision
    );
    // The session id is recorded, but never in its raw form.
    assert_eq!(
        event.session_id.as_deref(),
        Some(opaque_id("sess-abc").as_str())
    );
    assert!(!proxy.stderr_snapshot().contains("sess-abc"));
}

#[test]
fn non_streaming_json_usage_is_measured_with_local_counts_and_a_signed_delta() {
    let upstream_addr = spawn_json_upstream(
        "{\"id\":\"cmpl-1\",\"choices\":[{\"message\":{\"content\":\"hi\"}}],\
         \"usage\":{\"prompt_tokens\":12,\"completion_tokens\":3,\"total_tokens\":15}}",
        "application/json",
    );
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &[]);
    let response = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(&chat_request("gpt-4.1", false))
        .unwrap();
    assert!(body_text(response).contains("cmpl-1"));

    let events = wait_for_events(&proxy, 1);
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.usage_disposition, UsageDisposition::Reported);
    assert_eq!(event.provider_usage.unwrap().prompt_tokens, Some(12));
    let local_after = event.local_after_tokens.unwrap();
    assert_eq!(event.provider_delta_tokens, Some(12 - local_after as i64));
    assert!(event.local_transform_micros.is_some());
}

#[test]
fn a_malformed_usage_payload_disables_accounting_without_breaking_the_stream() {
    let chunks = vec![
        "data: {\"usage\":{\"total_tokens\":7}}\n\n",
        "data: {truncated\n\n",
        "data: [DONE]\n\n",
    ];
    let expected = chunks.concat();
    let upstream_addr = spawn_sse_upstream(chunks);
    let proxy = ProxyProcess::start(&format!("http://{upstream_addr}"), &[]);
    let response = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(&chat_request("gpt-4o", true))
        .unwrap();
    assert_eq!(body_text(response), expected);

    let events = wait_for_events(&proxy, 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].usage_disposition, UsageDisposition::Malformed);
    assert_eq!(events[0].provider_usage, None);
    assert_eq!(events[0].completion, CompletionState::Completed);
}

#[test]
fn an_upstream_connect_failure_still_emits_one_terminal_event() {
    let proxy = ProxyProcess::start(&format!("http://{}", free_addr()), &[]);
    let result = ureq::post(proxy.url("/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(&chat_request("qwen2.5:7b", false));
    match result.unwrap_err() {
        ureq::Error::StatusCode(502) => {}
        other => panic!("expected 502, got {other:?}"),
    }

    let events = wait_for_events(&proxy, 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].completion, CompletionState::Failed);
    assert_eq!(events[0].usage_disposition, UsageDisposition::Absent);
    assert_eq!(events[0].provider_usage, None);
    // The local stage still ran, so a failed round trip is still accounted for.
    assert!(events[0].local_after_tokens.is_some());
    assert_eq!(
        events[0].model,
        Some(ModelResolution::Unknown {
            requested: "qwen2.5:7b".to_string()
        })
    );
}
