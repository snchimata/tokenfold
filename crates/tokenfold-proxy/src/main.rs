mod server;

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use tokenfold_core::CompressionMode;

/// HTTP proxy that compresses provider-shaped JSON requests before forwarding them upstream.
/// Scope: ROADMAP.md F-040 (Phase 5). Not a CLI subcommand — a separate binary per spec.
#[derive(Parser)]
#[command(name = "tokenfold-proxy", version, about)]
struct Cli {
    /// Upstream base URL (e.g. https://api.openai.com). Fixed at process start; there is no
    /// per-request override (SSRF invariant — see INTERFACES.md §3.2).
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
    /// Disable request-body compression (pure passthrough proxy).
    #[arg(long)]
    no_compress: bool,
    #[arg(long, default_value = "balanced")]
    mode: String,
    #[arg(long)]
    target_tokens: Option<usize>,
    /// Always rejected: secret redaction cannot be disabled in proxy mode.
    #[arg(long = "unsafe-disable-redaction", hide = true)]
    unsafe_disable_redaction: bool,
    /// F-045 retrieval-store backend used by `/v1/retrieve*` and `X-TokenFold-Store-Originals`.
    #[arg(long, default_value = "filesystem")]
    retrieval_backend: String,
    /// F-045 filesystem backend root override; defaults to the standard XDG-based path.
    #[arg(long)]
    retrieval_store_path: Option<PathBuf>,
}

fn main() {
    let cli = Cli::parse();

    if cli.unsafe_disable_redaction {
        eprintln!("error: --unsafe-disable-redaction is forbidden in proxy mode");
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
    let mode = match cli.mode.to_ascii_lowercase().as_str() {
        "conservative" => CompressionMode::Conservative,
        "balanced" => CompressionMode::Balanced,
        "aggressive" => CompressionMode::Aggressive,
        other => {
            eprintln!("error: invalid --mode {other}");
            std::process::exit(2);
        }
    };

    let config = server::ProxyConfig {
        upstream: cli.upstream.trim_end_matches('/').to_string(),
        max_body_bytes: cli.max_body_bytes,
        compress: !cli.no_compress,
        mode,
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
    server::run(&config, &http_server);
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
