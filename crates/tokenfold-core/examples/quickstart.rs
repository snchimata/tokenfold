//! Real-world quickstart for the `tokenfold-core` Rust API.
//!
//! Run it against the bundled OpenAI chat-completion payload:
//!
//! ```sh
//! cargo run -p tokenfold-core --example quickstart
//! ```
//!
//! It compresses a request body (verbose tool schema + chat messages) the way an agent
//! would before sending it to a model, then prints the token accounting and the typed
//! safety report that tokenfold returns for every transformation.

use tokenfold_core::{CompressionInput, CompressionMode, CompressionPolicy, compress};

// The payload ships with the repo (examples/openai_payload.json). Embedding it with
// include_str! keeps this example runnable from any working directory.
const PAYLOAD: &str = include_str!("../../../examples/openai_payload.json");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Wrap the raw request bytes, telling tokenfold it's an OpenAI-style JSON body.
    let input = CompressionInput::openai_json(PAYLOAD.as_bytes().to_vec());

    // 2. Pick a policy. Balanced is the safe default; JSON minification and schema
    //    compaction are on, lossy transforms stay gated behind a fidelity check.
    let policy = CompressionPolicy::builder()
        .mode(CompressionMode::Balanced)
        .build()?;

    // 3. Compress. You get back the rewritten bytes plus a CompressionReport.
    let out = compress(input, &policy)?;
    let report = &out.report;

    println!("status:    {:?}", report.status);
    println!(
        "tokens:    {} -> {}  ({} saved, {:.1}%)",
        report.original_tokens, report.compressed_tokens, report.saved_tokens, report.savings_pct,
    );
    println!(
        "estimator: {} (exact: {})",
        report.estimator.backend, report.estimator.is_exact,
    );

    // 4. The report itemizes what each transform did — receipts, not guesses.
    println!("\ntransforms:");
    for t in &report.transforms {
        println!(
            "  - {:<24} {:<10?} (saved {} tokens)",
            t.id, t.status, t.saved_tokens,
        );
    }

    for w in &report.warnings {
        println!("  warning: {}", w.message);
    }

    // 5. out.bytes is the compressed payload, ready to send to any provider.
    println!("\ncompressed payload ({} bytes):", out.bytes.len());
    println!("{}", String::from_utf8_lossy(&out.bytes));

    Ok(())
}
