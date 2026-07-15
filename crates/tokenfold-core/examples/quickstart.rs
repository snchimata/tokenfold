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

// The payloads ship with the repo. Embedding them with include_str! keeps this example
// runnable from any working directory.
const PAYLOAD: &str = include_str!("../../../examples/openai_payload.json");
const API_RESPONSE: &str = include_str!("../../../examples/api_response.json");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    compress_openai_body()?;
    println!("\n{}\n", "-".repeat(72));
    compress_json_data()?;
    Ok(())
}

// --- 1. An LLM request body (chat messages + a verbose tool schema) -----------------------
fn compress_openai_body() -> Result<(), Box<dyn std::error::Error>> {
    // Wrap the raw request bytes, telling tokenfold it's an OpenAI-style JSON body.
    let input = CompressionInput::openai_json(PAYLOAD.as_bytes().to_vec());

    // 2. Pick a policy. Balanced is the safe default; JSON minification and schema
    //    compaction are on, lossy transforms stay gated behind a fidelity check.
    let policy = CompressionPolicy::builder()
        .mode(CompressionMode::Balanced)
        .build()?;

    // 3. Compress. You get back the rewritten bytes plus a CompressionReport.
    let out = compress(input, &policy)?;
    let report = &out.report;

    println!("== OpenAI request body ==");
    print_report(report);
    Ok(())
}

// --- 2. Generic JSON data (API response, records, logs) -----------------------------------
// Data-JSON that isn't an LLM message payload is minified,
// folds arrays of same-shape objects into columnar form (each key once, not once per row),
// and dictionaries repeated values — all losslessly (every stage is round-trip gated).
fn compress_json_data() -> Result<(), Box<dyn std::error::Error>> {
    let input = CompressionInput::json(API_RESPONSE.as_bytes().to_vec());
    let policy = CompressionPolicy::builder()
        .mode(CompressionMode::Balanced)
        .build()?;
    let out = compress(input, &policy)?;

    println!("== Generic JSON data ==");
    print_report(&out.report);
    println!("\ncompressed payload ({} bytes):", out.bytes.len());
    println!("{}", String::from_utf8_lossy(&out.bytes));
    Ok(())
}

fn print_report(report: &tokenfold_core::report::CompressionReport) {
    println!("status:    {:?}", report.status);
    println!(
        "tokens:    {} -> {}  ({} saved, {:.1}%)",
        report.original_tokens, report.compressed_tokens, report.saved_tokens, report.savings_pct,
    );
    println!(
        "estimator: {} (exact: {})",
        report.estimator.backend, report.estimator.is_exact,
    );
    // The report itemizes what each transform did — receipts, not guesses.
    println!("transforms:");
    for t in &report.transforms {
        println!(
            "  - {:<24} {:<10?} (saved {} tokens)",
            t.id, t.status, t.saved_tokens,
        );
    }
}
