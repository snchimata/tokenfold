//! Task 7 benchmark suite (ROADMAP.md Phase 4 / ENGINEERING.md "Benchmark Tests").
//!
//! ponytail: ENGINEERING.md names `criterion` + `divan` as the tools here. Both pull in real
//! dependency trees (criterion: plotting/serde chain; divan: its own harness+macros) for what
//! this project actually needs: a p95-latency and bytes-allocated regression gate against a
//! checked-in threshold file. `std::time::Instant` + a custom counting `GlobalAlloc` do the same
//! job in ~150 lines with zero new dependencies. Revisit if we need criterion's HTML reports or
//! statistical outlier detection.
//!
//! Run with `cargo bench -p tokenfold-core`. Fails (panics, nonzero exit) on regression beyond
//! `benches/THRESHOLDS.toml`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde_json::json;
use tokenfold_core::{CompressionInput, CompressionMode, CompressionPolicy, compress};

struct CountingAllocator;

static BYTES_ALLOCATED: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        BYTES_ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn command_output_fixture(target_bytes: usize) -> Vec<u8> {
    let mut out = String::with_capacity(target_bytes + 4096);
    let mut i: u64 = 0;
    while out.len() < target_bytes {
        i += 1;
        if i.is_multiple_of(37) {
            // Sprinkle in secret-shaped lines so `secret_redaction` does real work, not a no-op.
            out.push_str(&format!(
                "auth: Bearer sk-{:0>48}\n",
                i.wrapping_mul(2654435761)
            ));
        } else if i.is_multiple_of(11) {
            // Runs of identical lines (log_compaction fodder; log_compaction was promoted out
            // of --experimental and now runs under the default Balanced policy used here).
            for _ in 0..5 {
                out.push_str("INFO 200 GET /health 3ms\n");
            }
        } else {
            out.push_str(&format!(
                "line {i}: build step completed in {}ms\n",
                i % 500
            ));
        }
    }
    out.into_bytes()
}

fn structured_json_fixture(target_bytes: usize) -> Vec<u8> {
    let mut tools = Vec::new();
    let mut approx_len = 2; // "[]"
    let mut n: u64 = 0;
    while approx_len < target_bytes {
        n += 1;
        let tool = json!({
            "type": "function",
            "function": {
                "name": format!("lookup_record_{n}"),
                "description": "Looks up a record by ID and returns its normalized fields for downstream processing.",
                "parameters": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "description": "Opaque record identifier." },
                        "verbose": { "type": "boolean", "default": false }
                    },
                    "examples": [
                        { "id": "rec_001", "verbose": false },
                        { "id": "rec_002", "verbose": true },
                        { "id": "rec_003", "verbose": false },
                        { "id": "rec_004", "verbose": true },
                        { "id": "rec_005", "verbose": false }
                    ]
                }
            }
        });
        approx_len += serde_json::to_vec(&tool).unwrap().len() + 1;
        tools.push(tool);
    }
    // Pretty-printed, like a real client SDK request log — gives json_minify real whitespace to
    // strip. `to_vec` (compact) would make this fixture measure ~0 savings for reasons that have
    // nothing to do with the transform's quality.
    serde_json::to_vec_pretty(&json!({ "model": "gpt-4o", "tools": tools })).unwrap()
}

/// `iterations` timed runs after 3 untimed warmup runs; returns (p95_millis, last_saved_ratio).
fn measure(
    input_bytes: &[u8],
    format_ctor: impl Fn(Vec<u8>) -> CompressionInput,
    iterations: usize,
) -> (f64, f64) {
    let policy = CompressionPolicy::builder()
        .mode(CompressionMode::Balanced)
        .build()
        .expect("valid policy");

    for _ in 0..3 {
        let input = format_ctor(input_bytes.to_vec());
        let _ = compress(input, &policy).expect("compress succeeds");
    }

    let mut millis = Vec::with_capacity(iterations);
    let mut last_ratio = 0.0;
    for _ in 0..iterations {
        let input = format_ctor(input_bytes.to_vec());
        let start = Instant::now();
        let output = compress(input, &policy).expect("compress succeeds");
        millis.push(start.elapsed().as_secs_f64() * 1000.0);
        last_ratio = output.report.savings_ratio;
    }
    millis.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95_index = ((millis.len() as f64) * 0.95).ceil() as usize - 1;
    (millis[p95_index.min(millis.len() - 1)], last_ratio)
}

fn measure_bytes_allocated(
    input_bytes: &[u8],
    format_ctor: impl Fn(Vec<u8>) -> CompressionInput,
) -> usize {
    let policy = CompressionPolicy::builder()
        .mode(CompressionMode::Balanced)
        .build()
        .expect("valid policy");
    let input = format_ctor(input_bytes.to_vec());
    BYTES_ALLOCATED.store(0, Ordering::Relaxed);
    let output = compress(input, &policy).expect("compress succeeds");
    let bytes = BYTES_ALLOCATED.load(Ordering::Relaxed);
    std::hint::black_box(output);
    bytes
}

#[derive(Debug, serde::Deserialize)]
struct Thresholds {
    thresholds: ThresholdValues,
}

#[derive(Debug, serde::Deserialize)]
struct ThresholdValues {
    command_output_under_1mb_p95_ms: f64,
    structured_json_under_2mb_p95_ms: f64,
    max_bytes_allocated_per_call: usize,
    min_exact_token_savings_ratio: f64,
}

fn load_thresholds() -> Thresholds {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/THRESHOLDS.toml");
    let text = std::fs::read_to_string(path).expect("THRESHOLDS.toml readable");
    toml::from_str(&text).expect("THRESHOLDS.toml parses")
}

fn main() {
    let thresholds = load_thresholds();

    let command_output = command_output_fixture(900_000);
    let structured_json = structured_json_fixture(1_800_000);

    let (command_p95, _) = measure(&command_output, CompressionInput::command_output, 15);
    let (json_p95, json_savings_ratio) =
        measure(&structured_json, CompressionInput::openai_json, 15);
    let bytes_allocated = measure_bytes_allocated(&structured_json, CompressionInput::openai_json);

    println!(
        "command_output_under_1mb_p95_ms = {command_p95:.3} (threshold {})",
        thresholds.thresholds.command_output_under_1mb_p95_ms
    );
    println!(
        "structured_json_under_2mb_p95_ms = {json_p95:.3} (threshold {})",
        thresholds.thresholds.structured_json_under_2mb_p95_ms
    );
    println!(
        "bytes_allocated_per_call (structured_json) = {bytes_allocated} (threshold {})",
        thresholds.thresholds.max_bytes_allocated_per_call
    );
    println!(
        "exact_token_savings_ratio (structured_json) = {json_savings_ratio:.4} (threshold {})",
        thresholds.thresholds.min_exact_token_savings_ratio
    );

    let mut failures = Vec::new();
    if command_p95 > thresholds.thresholds.command_output_under_1mb_p95_ms {
        failures.push(format!(
            "command_output_under_1mb_p95_ms regressed: {command_p95:.3}ms > {:.3}ms",
            thresholds.thresholds.command_output_under_1mb_p95_ms
        ));
    }
    if json_p95 > thresholds.thresholds.structured_json_under_2mb_p95_ms {
        failures.push(format!(
            "structured_json_under_2mb_p95_ms regressed: {json_p95:.3}ms > {:.3}ms",
            thresholds.thresholds.structured_json_under_2mb_p95_ms
        ));
    }
    if bytes_allocated > thresholds.thresholds.max_bytes_allocated_per_call {
        failures.push(format!(
            "max_bytes_allocated_per_call regressed: {bytes_allocated} > {}",
            thresholds.thresholds.max_bytes_allocated_per_call
        ));
    }
    if json_savings_ratio < thresholds.thresholds.min_exact_token_savings_ratio {
        failures.push(format!(
            "min_exact_token_savings_ratio regressed: {json_savings_ratio:.4} < {:.4}",
            thresholds.thresholds.min_exact_token_savings_ratio
        ));
    }

    if !failures.is_empty() {
        for f in &failures {
            eprintln!("REGRESSION: {f}");
        }
        std::process::exit(1);
    }
}
