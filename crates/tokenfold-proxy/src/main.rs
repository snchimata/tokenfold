mod server;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use clap::Parser;
use tokenfold_core::Preset;

/// HTTP proxy that compresses provider-shaped JSON requests before forwarding them upstream.
/// Deliberately a separate binary rather than a `tokenfold` subcommand, so the HTTP server and
/// its network surface stay out of the CLI.
#[derive(Parser)]
#[command(name = "tokenfold-proxy", version, about)]
struct Cli {
    /// Upstream base URL (e.g. https://api.openai.com). Fixed at process start; no request
    /// header or body field can redirect it (SSRF invariant).
    #[arg(long)]
    upstream: String,
    /// Address to bind. Defaults to loopback; use --allow-non-loopback-bind to change that.
    #[arg(long, default_value = "127.0.0.1:8787")]
    bind: String,
    /// Allow a plain http:// upstream (default requires https://).
    #[arg(long)]
    insecure_upstream: bool,
    /// Allow binding a non-loopback address.
    #[arg(long)]
    allow_non_loopback_bind: bool,
    /// Reject non-streaming request bodies larger than this many bytes.
    #[arg(long, default_value_t = 10_000_000)]
    max_body_bytes: usize,
    /// Worker count (default: CPU count clamped to 2-16).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=256))]
    workers: Option<u16>,
    /// Pending application requests (default: twice the worker count).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=1024))]
    queue_capacity: Option<u16>,
    /// Whole upstream exchange deadline, including SSE body (not an idle timeout).
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=86400))]
    upstream_timeout_secs: u64,
    /// Drain deadline after a termination signal; forced exit is code 6.
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=300))]
    shutdown_timeout_secs: u64,
    /// Disable request-body compression (pure passthrough proxy).
    #[arg(long)]
    no_compress: bool,
    #[arg(long, default_value = "balanced")]
    preset: String,
    #[arg(long)]
    target_tokens: Option<usize>,
    /// Always rejected: secret redaction cannot be disabled in proxy preset.
    #[arg(long = "unsafe-disable-redaction", hide = true)]
    unsafe_disable_redaction: bool,
    /// Retrieval-store backend used by `/v1/retrieve*` and `X-TokenFold-Store-Originals`.
    #[arg(long, default_value = "filesystem")]
    retrieval_backend: String,
    /// Retrieval-store filesystem root override; defaults to the standard XDG-based path.
    #[arg(long)]
    retrieval_store_path: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();

    if cli.unsafe_disable_redaction {
        eprintln!("error: --unsafe-disable-redaction is forbidden in proxy preset");
        std::process::exit(5);
    }
    if let Err(message) = validate_upstream(&cli.upstream, cli.insecure_upstream) {
        eprintln!("error: {message}");
        std::process::exit(5);
    }
    let bind_addr: SocketAddr = match cli.bind.parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("error: invalid --bind address {}: {e}", cli.bind);
            std::process::exit(2);
        }
    };
    if !bind_addr.ip().is_loopback() && !cli.allow_non_loopback_bind {
        eprintln!(
            "error: refusing to bind non-loopback address {bind_addr} without --allow-non-loopback-bind"
        );
        std::process::exit(5);
    }
    let preset = match cli.preset.to_ascii_lowercase().as_str() {
        "conservative" => Preset::Conservative,
        "balanced" => Preset::Balanced,
        "aggressive" => Preset::Aggressive,
        other => {
            eprintln!("error: invalid --preset {other}");
            std::process::exit(2);
        }
    };

    if !bind_addr.ip().is_loopback() {
        eprintln!(
            "warning: non-loopback routes have no client authentication; require an authenticated gateway and network isolation (including retrieval routes)"
        );
    }
    let workers = cli.workers.map(usize::from).unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2)
            .clamp(2, 16)
    });
    let config = server::ProxyConfig {
        workers,
        queue_capacity: cli.queue_capacity.map(usize::from).unwrap_or(workers * 2),
        upstream_agent: ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(cli.upstream_timeout_secs)))
            .build()
            .into(),
        upstream: cli.upstream.trim_end_matches('/').to_string(),
        max_body_bytes: cli.max_body_bytes,
        compress: !cli.no_compress,
        preset,
        target_tokens: cli.target_tokens,
        retrieval_backend: cli.retrieval_backend,
        retrieval_store_path: cli.retrieval_store_path,
    };

    let http_server = match tiny_http::Server::http(&cli.bind) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to bind {}: {e}", cli.bind);
            std::process::exit(6);
        }
    };
    eprintln!(
        "tokenfold-proxy listening on {} -> {}",
        cli.bind, config.upstream
    );
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopping);
    if let Err(error) = ctrlc::set_handler(move || {
        if !signal.swap(true, Ordering::SeqCst) {
            eprintln!("shutdown requested; draining active requests");
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(cli.shutdown_timeout_secs));
                eprintln!("shutdown deadline exceeded; terminating active requests");
                std::process::exit(6);
            });
        }
    }) {
        eprintln!("error: cannot install shutdown handler: {error}");
        std::process::exit(6);
    }
    server::run(&config, &http_server, &stopping);
}

fn validate_upstream(upstream: &str, insecure: bool) -> Result<(), String> {
    if upstream.starts_with("https://") {
        Ok(())
    } else if upstream.starts_with("http://") {
        if insecure {
            Ok(())
        } else {
            Err(format!(
                "upstream {upstream} uses http:// without TLS; pass --insecure-upstream to allow this (not recommended)"
            ))
        }
    } else {
        Err(format!(
            "upstream must start with https:// (or http:// with --insecure-upstream): {upstream}"
        ))
    }
}
