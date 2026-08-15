//! Black-box tests against the compiled `tokenfold` binary, covering the v0.1 CLI contract:
//! stream routing, exit codes, `--disable secret_redaction` rejection, and
//! stdin handling.

use std::io::Write;
use std::process::{Command, Stdio};

/// Every test spawns the real `tokenfold` binary as a child process, and a plain
/// `compress`/`wrap` run appends to the local analytics ledger by default
/// (`[analytics].enabled = true`). Without this, every test in this file would silently write a
/// real `ledger.db` under the *developer's actual home directory* (`$XDG_DATA_HOME` /
/// `~/.local/share/tokenfold`) on every run. Redirecting `XDG_DATA_HOME` once, for the whole
/// test binary's lifetime, keeps every child process's default ledger (and retrieval store)
/// path inside a throwaway temp directory instead — this only overrides the *parent* test
/// process's own env (inherited by children at spawn time), never the developer's real one.
fn bin() -> &'static str {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_cli_test_xdg_data_home_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", dir);
        }
    });
    env!("CARGO_BIN_EXE_tokenfold")
}

fn payload_path() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/openai_payload.json"
    )
    .to_string()
}

#[test]
fn inspect_under_budget_is_passthrough_with_exit_zero() {
    let out = Command::new(bin())
        .args([
            "inspect",
            &payload_path(),
            "--format",
            "openai",
            "--target-tokens",
            "999999",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("UNDER budget"), "stderr was: {stderr}");
    assert!(
        out.stdout.is_empty(),
        "inspect must never emit a payload to stdout"
    );
}

#[test]
fn inspect_renders_verdict_table_totals_and_warnings() {
    let out = Command::new(bin())
        .args([
            "inspect",
            &payload_path(),
            "--format",
            "openai",
            "--target-tokens",
            "100",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("OVER budget"),
        "verdict line missing: {stderr}"
    );
    assert!(
        stderr.contains("TRANSFORM"),
        "transform table header missing: {stderr}"
    );
    assert!(stderr.contains("TOTAL"), "totals row missing: {stderr}");
    assert!(
        stderr.contains("WARNINGS:"),
        "warnings block missing: {stderr}"
    );
}

#[test]
fn inspect_json_emits_only_the_report_to_stdout() {
    let out = Command::new(bin())
        .args(["inspect", &payload_path(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        out.stderr.is_empty(),
        "stderr should be empty, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be valid JSON");
    assert_eq!(value["schema_version"], "1.0");
}

#[test]
fn compress_reads_stdin_and_keeps_payload_on_stdout() {
    let mut child = Command::new(bin())
        .args(["compress", "-", "--format", "text"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"hello world, this is plain text input")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"hello world, this is plain text input");
}

#[test]
fn compress_disable_secret_redaction_is_rejected_with_nonzero_exit() {
    let out = Command::new(bin())
        .args(["compress", &payload_path(), "--disable", "secret_redaction"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(
        out.status.code(),
        Some(5),
        "ConfigError must map to exit code 5"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("secret_redaction"), "stderr was: {stderr}");
}

#[test]
fn compress_json_keeps_payload_on_stdout_and_report_on_stderr() {
    let out = Command::new(bin())
        .args(["compress", &payload_path(), "--format", "openai", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).is_ok(),
        "stdout must be the raw payload (still JSON here, since input was JSON)"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&out.stderr).expect("stderr must be the report JSON");
    assert_eq!(report["schema_version"], "1.0");
}

/// Regression test for a bug an adversarial review caught: `--dry-run` routes to the same code
/// path as `inspect`, which used to hardcode `lossy: None` regardless of what the user passed,
/// silently making `--dry-run --lossy ...` preview nothing about the lossy stage at all.
#[test]
fn compress_dry_run_with_lossy_previews_json_prune_in_the_transform_table() {
    let dir = unique_temp_dir("dry_run_lossy");
    std::fs::create_dir_all(&dir).unwrap();
    let payload_path = dir.join("payload.json");
    let items: Vec<serde_json::Value> = (0..40)
        .map(|i| {
            serde_json::json!({
                "id": i,
                "note": "x".repeat(200),
            })
        })
        .collect();
    std::fs::write(
        &payload_path,
        serde_json::to_vec(&serde_json::json!({"items": items})).unwrap(),
    )
    .unwrap();

    let out = Command::new(bin())
        .args([
            "compress",
            payload_path.to_str().unwrap(),
            "--format",
            "json",
            "--dry-run",
            "--lossy",
            "heuristic",
            "--lossy-ratio",
            "0.1",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be the report JSON");
    let transforms = report["transforms"].as_array().expect("transforms array");
    assert!(
        transforms.iter().any(|t| t["id"] == "json_prune"),
        "--dry-run --lossy must preview the json_prune stage, got transforms: {transforms:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Regression test for a real, confirmed bug: `--dry-run --lossy` (routing through `cmd_inspect`)
/// used to physically write to the retrieval store even though `inspect`/`--dry-run` is
/// documented as a side-effect-free preview -- forwarding the lossy flags into `cmd_inspect`
/// (the fix for the test above) reintroduced a real disk-write side effect that didn't exist
/// before lossy did. Uses an isolated `--config`-specified store path (not the shared
/// process-wide `XDG_DATA_HOME` from `bin()`) so this assertion can't be polluted by other tests
/// running concurrently.
#[test]
fn compress_dry_run_with_lossy_never_writes_to_the_retrieval_store() {
    let dir = unique_temp_dir("dry_run_lossy_no_write");
    std::fs::create_dir_all(&dir).unwrap();
    let payload_path = dir.join("payload.json");
    let store_dir = dir.join("store");
    let config_path = dir.join("config.toml");

    // The real CLI uses tiktoken by default, which compresses a repeated-character string (e.g.
    // "x".repeat(N)) far better than a random-looking marker hash -- varied, natural-language-ish
    // padding is needed here so items are actually worth dropping under a real tokenizer, same
    // lesson learned earlier this session benchmarking json_prune directly. Each item also gets
    // a uniquely-named extra field, which breaks fold's same-key-set homogeneity requirement
    // structurally, so the array is never folded regardless of `--disable` (kept this way so the
    // test doesn't depend on `--disable` forwarding, which is covered by its own test below).
    let words = [
        "processed",
        "batch",
        "successfully",
        "no",
        "anomalies",
        "detected",
        "in",
        "shard",
        "cache",
        "warm",
        "hit",
        "ratio",
        "nominal",
        "replication",
        "lag",
        "within",
        "tolerance",
        "checksum",
        "verified",
        "for",
        "segment",
        "worker",
        "completed",
        "cycle",
        "queue",
        "drain",
        "normal",
    ];
    let items: Vec<serde_json::Value> = (0..20)
        .map(|i| {
            // Prefixed with `i`, and long enough (120 words) to comfortably clear a $tf_ref
            // marker's own fixed overhead under a real tokenizer -- otherwise json_value_dict
            // would dictionary-reference repeated word sequences (shrinking items to nothing
            // before json_prune runs), or items would simply be too small to ever be worth
            // dropping in the first place, same lessons learned earlier this session.
            let note = format!(
                "item {i}: {}",
                (0..120)
                    .map(|j| words[(i * 11 + j * 5 + (j * j) % 13) % words.len()])
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let mut obj = serde_json::Map::new();
            obj.insert("id".to_string(), serde_json::json!(i));
            obj.insert("note".to_string(), serde_json::json!(note));
            obj.insert(format!("unique_{i}"), serde_json::json!(true));
            serde_json::Value::Object(obj)
        })
        .collect();
    std::fs::write(
        &payload_path,
        serde_json::to_vec(&serde_json::json!({"items": items})).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &config_path,
        format!(
            "[retrieval]\nbackend = \"filesystem\"\nstore_path = {:?}\n",
            store_dir.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();

    let out = Command::new(bin())
        .args([
            "compress",
            payload_path.to_str().unwrap(),
            "--format",
            "json",
            "--dry-run",
            "--lossy",
            "heuristic",
            "--lossy-ratio",
            "0.2",
            "--json",
            "--config",
        ])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be the report JSON");
    let jp = report["transforms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "json_prune")
        .expect("json_prune report present");
    assert_eq!(
        jp["status"], "applied",
        "preview must still show projected savings"
    );
    assert!(
        report["retrieval"].is_null(),
        "a preview must not claim anything was persisted, got: {:?}",
        report["retrieval"]
    );
    assert!(
        !store_dir.exists(),
        "a dry-run preview must never create the retrieval store directory at all"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Regression test for a real, confirmed bug: `compress --dry-run` routes to `cmd_inspect`,
/// which used to build its own overrides from scratch and hardcode `disable: []` regardless of
/// what the user actually passed to `--disable` -- a real run and its own `--dry-run` preview of
/// the SAME flags could show different active transforms, defeating the point of a preview.
#[test]
fn compress_dry_run_forwards_disable_so_the_preview_matches_a_real_run() {
    // A disabled transform is filtered out of the mode pipeline entirely (`modes::pipeline_for`)
    // before the transform loop runs, so it never appears in `transforms` at all -- the
    // regression this guards is `--disable` not reaching `--dry-run`'s preview, which would show
    // json_minify running (present in `transforms`) even though a real run with the same flag
    // wouldn't include it.
    let dir = unique_temp_dir("dry_run_disable_forwarded");
    std::fs::create_dir_all(&dir).unwrap();
    let payload_path = dir.join("payload.json");
    std::fs::write(&payload_path, br#"{"a":1,   "b":2}"#).unwrap();

    let run = |disable: bool| -> serde_json::Value {
        let mut args = vec![
            "compress",
            payload_path.to_str().unwrap(),
            "--format",
            "json",
            "--dry-run",
        ];
        if disable {
            args.push("--disable");
            args.push("json_minify");
        }
        args.push("--json");
        let out = Command::new(bin()).args(&args).output().unwrap();
        assert!(
            out.status.success(),
            "stderr was: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).expect("stdout must be the report JSON")
    };

    let without_disable = run(false);
    assert!(
        without_disable["transforms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "json_minify"),
        "json_minify must run by default, got {without_disable:#?}"
    );

    let with_disable = run(true);
    assert!(
        !with_disable["transforms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "json_minify"),
        "--dry-run --disable json_minify must preview it as disabled, matching a real run; \
         got {with_disable:#?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn wrap_runs_a_command_and_compresses_its_output() {
    let out = Command::new(bin())
        .args(["wrap", "--", "git", "--version"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("git version"));
}

#[test]
fn wrap_without_a_command_is_a_clear_invalid_input_error() {
    let out = Command::new(bin()).args(["wrap"]).output().unwrap();
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn enable_without_experimental_is_rejected() {
    // diff_compaction stays --experimental after the 2026-07-12 fidelity re-investigation
    // (log_compaction was promoted out of --experimental and no longer exercises this path).
    let out = Command::new(bin())
        .args([
            "compress",
            &payload_path(),
            "--format",
            "openai",
            "--enable",
            "diff_compaction",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--experimental"), "stderr was: {stderr}");
}

#[test]
fn list_transforms_reports_known_canonical_ids() {
    let out = Command::new(bin())
        .args(["inspect", "--list-transforms"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for id in [
        "secret_redaction",
        "json_minify",
        "schema_compaction",
        "log_compaction",
        "diff_compaction",
    ] {
        assert!(stdout.contains(id), "missing {id} in: {stdout}");
    }
}

#[test]
fn init_with_no_supported_host_is_a_clear_non_success() {
    let out = Command::new(bin())
        .args(["init", "--agent", "made-up-host"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a supported host"),
        "stderr was: {stderr}"
    );
}

#[test]
fn doctor_reports_estimator_and_config_status() {
    let out = Command::new(bin())
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(value["estimator"]["tiktoken_available"].is_boolean());
}

fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "tokenfold_cli_test_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn store_originals_then_retrieve_round_trips_exact_bytes() {
    let store_dir = unique_temp_dir("retrieve_store");
    let config_path = unique_temp_dir("retrieve_config").with_extension("toml");
    std::fs::write(
        &config_path,
        format!(
            "[retrieval]\nbackend = \"filesystem\"\nstore_path = {:?}\n",
            store_dir.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();

    let original = b"a plain text payload with nothing secret in it whatsoever";
    let out = Command::new(bin())
        .args([
            "compress",
            "-",
            "--format",
            "text",
            "--store-originals",
            "--config",
        ])
        .arg(&config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(original)?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        out.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hash = tokenfold_core::retrieval_store::hex_sha256(original);
    let out = Command::new(bin())
        .args(["retrieve", &hash, "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "retrieve failed, stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, original);

    std::fs::remove_dir_all(&store_dir).ok();
    std::fs::remove_file(&config_path).ok();
}

#[test]
fn retrieve_missing_hash_is_a_clear_nonzero_exit() {
    let store_dir = unique_temp_dir("retrieve_missing_store");
    let config_path = unique_temp_dir("retrieve_missing_config").with_extension("toml");
    std::fs::write(
        &config_path,
        format!(
            "[retrieval]\nbackend = \"filesystem\"\nstore_path = {:?}\n",
            store_dir.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();

    let out = Command::new(bin())
        .args([
            "retrieve",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--config",
        ])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no stored original found"),
        "stderr was: {stderr}"
    );
    assert!(out.stdout.is_empty());

    std::fs::remove_file(&config_path).ok();
}

#[test]
fn retrieve_on_a_report_file_says_no_storable_hash_rather_than_guessing() {
    let report_path = unique_temp_dir("retrieve_report").with_extension("json");
    let out = Command::new(bin())
        .args(["compress", &payload_path(), "--format", "openai", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::write(&report_path, &out.stderr).unwrap();

    let out = Command::new(bin())
        .args(["retrieve", report_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no storable hash in the current schema"),
        "stderr was: {stderr}"
    );

    std::fs::remove_file(&report_path).ok();
}

#[test]
fn diff_renders_a_compression_aware_header_and_body() {
    let dir = std::env::temp_dir();
    let raw = dir.join("tokenfold_cli_test_diff_raw.txt");
    let compressed = dir.join("tokenfold_cli_test_diff_compressed.txt");
    std::fs::write(&raw, "line one\nline two\nline three\n").unwrap();
    std::fs::write(&compressed, "line one\nline three\n").unwrap();

    let out = Command::new(bin())
        .args(["diff", raw.to_str().unwrap(), compressed.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("reduction"));
    assert!(stdout.contains("- line two"));

    std::fs::remove_file(&raw).ok();
    std::fs::remove_file(&compressed).ok();
}

// --- savings ledger, stats, gain, session -------------------------------------------------

fn analytics_config(body: &str) -> std::path::PathBuf {
    let path = unique_temp_dir("analytics_config").with_extension("toml");
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn wrap_records_a_ledger_entry_that_stats_can_aggregate() {
    let ledger_path = unique_temp_dir("ledger_wrap").with_extension("db");
    let config_path = analytics_config(&format!(
        "[analytics]\nledger_db = {:?}\n",
        ledger_path.to_string_lossy().replace('\\', "/")
    ));

    let out = Command::new(bin())
        .args(["wrap", "--config"])
        .arg(&config_path)
        .args(["--", "git", "--version"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = Command::new(bin())
        .args(["stats", "--json", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["requests"], 1);
    assert_eq!(value["commands"], 1);
    assert_eq!(value["wrapped_commands"], 1);
    assert_eq!(value["recent_requests"][0]["surface"], "wrap");

    std::fs::remove_file(&config_path).ok();
    std::fs::remove_file(&ledger_path).ok();
}

#[test]
fn analytics_disabled_writes_no_ledger_file() {
    let ledger_path = unique_temp_dir("ledger_disabled").with_extension("db");
    let config_path = analytics_config(&format!(
        "[analytics]\nenabled = false\nledger_db = {:?}\n",
        ledger_path.to_string_lossy().replace('\\', "/")
    ));

    let out = Command::new(bin())
        .args([
            "compress",
            &payload_path(),
            "--format",
            "openai",
            "--config",
        ])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        !ledger_path.exists(),
        "no ledger file should be created when [analytics].enabled is false"
    );

    std::fs::remove_file(&config_path).ok();
}

#[test]
fn compress_records_a_hashed_project_path_by_default() {
    let ledger_path = unique_temp_dir("ledger_hash").with_extension("db");
    let config_path = analytics_config(&format!(
        "[analytics]\nledger_db = {:?}\n",
        ledger_path.to_string_lossy().replace('\\', "/")
    ));

    let out = Command::new(bin())
        .args([
            "compress",
            &payload_path(),
            "--format",
            "openai",
            "--config",
        ])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let ledger_text = std::fs::read_to_string(&ledger_path).unwrap();
    let record: serde_json::Value =
        serde_json::from_str(ledger_text.lines().next().unwrap()).unwrap();
    let expected_hash = format!(
        "sha256:{}",
        tokenfold_core::retrieval_store::hex_sha256(payload_path().as_bytes())
    );
    assert_eq!(record["project_hash"], expected_hash);

    std::fs::remove_file(&config_path).ok();
    std::fs::remove_file(&ledger_path).ok();
}

#[test]
fn compress_records_the_raw_project_path_when_hashing_is_disabled() {
    let ledger_path = unique_temp_dir("ledger_nohash").with_extension("db");
    let config_path = analytics_config(&format!(
        "[analytics]\nhash_project_paths = false\nledger_db = {:?}\n",
        ledger_path.to_string_lossy().replace('\\', "/")
    ));

    let out = Command::new(bin())
        .args([
            "compress",
            &payload_path(),
            "--format",
            "openai",
            "--config",
        ])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(out.status.success());

    let ledger_text = std::fs::read_to_string(&ledger_path).unwrap();
    let record: serde_json::Value =
        serde_json::from_str(ledger_text.lines().next().unwrap()).unwrap();
    assert_eq!(record["project_hash"], payload_path());

    std::fs::remove_file(&config_path).ok();
    std::fs::remove_file(&ledger_path).ok();
}

#[test]
fn stats_aggregates_ad_hoc_report_glob_files() {
    let dir = unique_temp_dir("stats_glob");
    std::fs::create_dir_all(&dir).unwrap();

    for i in 0..2 {
        let out = Command::new(bin())
            .args(["compress", &payload_path(), "--format", "openai", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(dir.join(format!("report_{i}.json")), &out.stderr).unwrap();
    }

    let pattern = dir.join("*.json").to_string_lossy().replace('\\', "/");
    // Disable ledger merging so this only counts the two glob-matched report files.
    let config_path = analytics_config("[analytics]\nenabled = false\n");

    let out = Command::new(bin())
        .args(["stats", &pattern, "--json", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["requests"], 2);

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_file(&config_path).ok();
}

#[test]
fn stats_csv_output_has_a_summary_section_and_a_recent_requests_section() {
    let config_path = analytics_config("[analytics]\nenabled = false\n");

    let out = Command::new(bin())
        .args(["stats", "--csv", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("schema_version,scope,window"));
    assert!(stdout.contains("request_id,timestamp,surface"));

    std::fs::remove_file(&config_path).ok();
}

#[test]
fn gain_reports_realized_savings_from_the_ledger_within_the_since_window() {
    let ledger_path = unique_temp_dir("gain_ledger").with_extension("db");
    let config_path = analytics_config(&format!(
        "[analytics]\nledger_db = {:?}\n",
        ledger_path.to_string_lossy().replace('\\', "/")
    ));

    let out = Command::new(bin())
        .args([
            "compress",
            &payload_path(),
            "--format",
            "openai",
            "--config",
        ])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(out.status.success());

    let out = Command::new(bin())
        .args(["gain", "--json", "--since", "30d", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["requests"], 1);
    assert!(value["saved_tokens"].as_u64().unwrap() > 0);

    std::fs::remove_file(&config_path).ok();
    std::fs::remove_file(&ledger_path).ok();
}

#[test]
fn session_reports_wrap_coverage_and_respects_recent_limit() {
    let ledger_path = unique_temp_dir("session_ledger").with_extension("db");
    let config_path = analytics_config(&format!(
        "[analytics]\nledger_db = {:?}\n",
        ledger_path.to_string_lossy().replace('\\', "/")
    ));

    for _ in 0..2 {
        let out = Command::new(bin())
            .args(["wrap", "--config"])
            .arg(&config_path)
            .args(["--", "git", "--version"])
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    let out = Command::new(bin())
        .args(["session", "--json", "--recent", "1", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["commands"], 2);
    assert_eq!(value["wrapped_commands"], 2);
    assert_eq!(value["recent_requests"].as_array().unwrap().len(), 1);

    std::fs::remove_file(&config_path).ok();
    std::fs::remove_file(&ledger_path).ok();
}

// --- declarative command filter registry --------------------------------------------------

fn filters_config(body: &str) -> std::path::PathBuf {
    let path = unique_temp_dir("filters_config").with_extension("toml");
    std::fs::write(&path, body).unwrap();
    path
}

fn write_test_filter_pack(path: &std::path::Path, pack_id: &str, pattern: &str, replacement: &str) {
    let toml_str = format!(
        r#"
schema_version = "1.0"

[pack]
id = "{pack_id}"
version = "1.0.0"

[[filters]]
id = "git-version-marker"
version = "1.0.0"
match_command = ["git", "--version"]

[[filters.stages]]
type = "replace"
pattern = "{pattern}"
replacement = "{replacement}"
"#
    );
    std::fs::write(path, toml_str).unwrap();
}

#[test]
fn filters_list_json_includes_all_three_built_ins() {
    let out = Command::new(bin())
        .args(["--json", "filters", "list"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    let built_in_pack_ids: Vec<&str> = rows
        .iter()
        .filter(|r| r["tier"] == "built_in")
        .map(|r| r["pack_id"].as_str().unwrap())
        .collect();
    assert!(built_in_pack_ids.contains(&"git-diff"));
    assert!(built_in_pack_ids.contains(&"git-status"));
    assert!(built_in_pack_ids.contains(&"build-test-log"));
    assert!(
        rows.iter()
            .all(|r| r["tier"] != "built_in" || r["trusted"] == true)
    );
}

#[test]
fn filters_verify_passes_for_built_ins() {
    let out = Command::new(bin())
        .args(["filters", "verify", "--require-all"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn filters_verify_require_all_fails_closed_on_a_bad_project_filter() {
    let project_path = unique_temp_dir("verify_bad_project").with_extension("toml");
    std::fs::write(
        &project_path,
        "schema_version = \"9.9\"\n\n[pack]\nid = \"bad\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();
    let config_path = filters_config(&format!(
        "[filters]\nproject_filters = {:?}\n",
        project_path.to_string_lossy().replace('\\', "/")
    ));

    let out = Command::new(bin())
        .args(["filters", "verify", "--require-all", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(!out.status.success());

    // Without --require-all, the same failure is reported but the exit code stays 0.
    let out = Command::new(bin())
        .args(["filters", "verify", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(out.status.success());

    std::fs::remove_file(&project_path).ok();
    std::fs::remove_file(&config_path).ok();
}

#[test]
fn filters_trust_then_wrap_applies_a_trusted_project_filter() {
    let project_path = unique_temp_dir("trust_wrap_project").with_extension("toml");
    let trust_path = unique_temp_dir("trust_wrap_trust").with_extension("json");
    write_test_filter_pack(&project_path, "test-project-pack", "git version", "GITVER");
    let config_path = filters_config(&format!(
        "[filters]\nproject_filters = {:?}\ntrust_store = {:?}\n",
        project_path.to_string_lossy().replace('\\', "/"),
        trust_path.to_string_lossy().replace('\\', "/"),
    ));

    let out = Command::new(bin())
        .args(["filters", "trust", "--config"])
        .arg(&config_path)
        .arg(&project_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = Command::new(bin())
        .args(["--json", "wrap", "--config"])
        .arg(&config_path)
        .args(["--", "git", "--version"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(report["command"]["filter_pack_id"], "test-project-pack");

    std::fs::remove_file(&project_path).ok();
    std::fs::remove_file(&trust_path).ok();
    std::fs::remove_file(&config_path).ok();
}

#[test]
fn wrap_leaves_filter_pack_id_null_when_no_filter_matches() {
    // `git --version` doesn't match any built-in filter's `match_command`.
    let out = Command::new(bin())
        .args(["--json", "wrap", "--", "git", "--version"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert!(report["command"]["filter_pack_id"].is_null());
}

/// Round-5 external review, measured live: `--lossy-ratio 0.25` over a payload whose rows are all
/// too small to be worth replacing with a `$tf_ref` marker emitted 1,834 bytes where a plain
/// lossless run emitted 644 -- ~3x WORSE while dropping nothing -- because `--lossy`
/// unconditionally switched `json_field_fold`/`json_value_dict` off up front for a pruning stage
/// that then never applied. They are deferred past the lossy stage now, not disabled.
#[test]
fn lossy_that_prunes_nothing_falls_back_to_the_lossless_output() {
    let dir = unique_temp_dir("lossy_noop_fallback");
    std::fs::create_dir_all(&dir).unwrap();
    let payload_path = dir.join("payload.json");
    let config_path = dir.join("config.toml");
    // Identical short rows: nothing here can profitably become a marker, but `json_field_fold`
    // and `json_value_dict` both have plenty of work to do.
    let events: Vec<serde_json::Value> = (0..15)
        .map(|i| {
            serde_json::json!({
                "seq": i,
                "retries": 0,
                "note": "queue drain cycle completed normally with no backpressure observed",
            })
        })
        .collect();
    std::fs::write(
        &payload_path,
        serde_json::to_vec(&serde_json::json!({"events": events})).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &config_path,
        format!(
            "[retrieval]\nbackend = \"filesystem\"\nstore_path = {:?}\n",
            dir.join("store").to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();

    let run = |extra: &[&str]| {
        let mut args = vec![
            "compress",
            payload_path.to_str().unwrap(),
            "--format",
            "json",
            "--quiet",
        ];
        args.extend_from_slice(extra);
        let out = Command::new(bin())
            .args(&args)
            .arg("--config")
            .arg(&config_path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stderr was: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };

    let lossless = run(&[]);
    let lossy = run(&["--lossy", "heuristic", "--lossy-ratio", "0.25"]);
    assert_eq!(
        String::from_utf8_lossy(&lossy),
        String::from_utf8_lossy(&lossless),
        "a --lossy run that pruned nothing must not be worse than the plain lossless run"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Round-5 external review, measured live: an eligible ROOT array has path `""`, and the
/// nearest-eligible-ancestor preserve rule was written as `starts_with("{path}.")`, which no path
/// can satisfy when the path is empty. `--lossy-preserve users` on a top-level array left 20
/// `$tf_ref` markers in the output -- the safety flag silently protected nothing.
#[test]
fn lossy_preserve_protects_a_top_level_array() {
    let dir = unique_temp_dir("lossy_preserve_root");
    std::fs::create_dir_all(&dir).unwrap();
    let payload_path = dir.join("payload.json");
    let config_path = dir.join("config.toml");

    let words = [
        "processed",
        "batch",
        "successfully",
        "no",
        "anomalies",
        "detected",
        "in",
        "shard",
        "cache",
        "warm",
        "hit",
        "ratio",
        "nominal",
        "replication",
        "lag",
        "within",
        "tolerance",
        "checksum",
        "verified",
        "for",
        "segment",
        "worker",
        "completed",
        "cycle",
        "queue",
        "drain",
        "normal",
    ];
    // A bare top-level array, each row big enough under a real tokenizer to be worth dropping.
    let rows: Vec<serde_json::Value> = (0..20)
        .map(|i| {
            let note = format!(
                "row {i}: {}",
                (0..120)
                    .map(|j| words[(i * 11 + j * 5 + (j * j) % 13) % words.len()])
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            serde_json::json!({"users": [1, 2, 3], "id": i, "note": note})
        })
        .collect();
    std::fs::write(&payload_path, serde_json::to_vec(&rows).unwrap()).unwrap();
    std::fs::write(
        &config_path,
        format!(
            "[retrieval]\nbackend = \"filesystem\"\nstore_path = {:?}\n",
            dir.join("store").to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();

    let run = |extra: &[&str]| {
        let mut args = vec![
            "compress",
            payload_path.to_str().unwrap(),
            "--format",
            "json",
            "--quiet",
            "--lossy",
            "heuristic",
            "--lossy-ratio",
            "0",
        ];
        args.extend_from_slice(extra);
        let out = Command::new(bin())
            .args(&args)
            .arg("--config")
            .arg(&config_path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stderr was: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // Control first: without the flag this document really does prune, so a zero-marker result
    // below can only mean the flag worked.
    assert!(
        run(&[]).contains("$tf_ref"),
        "control: the root array must be prunable without --lossy-preserve"
    );
    assert!(
        !run(&["--lossy-preserve", "users"]).contains("$tf_ref"),
        "--lossy-preserve must protect an eligible root array, not silently match nothing"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Round-5 external review, measured live: `--lossy` on an OpenAI payload correctly reported
/// `json_prune: skipped / not_applicable_to_format`, and still wrote 23,487 bytes of the user's
/// unmodified input to a freshly created retrieval directory. `--lossy` implies a durable receipt
/// only where the lossy stage can actually run; `--store-originals` is the independent request.
#[test]
fn lossy_on_an_unsupported_format_never_persists_the_input() {
    let dir = unique_temp_dir("lossy_wrong_format_no_store");
    std::fs::create_dir_all(&dir).unwrap();
    let payload_path = dir.join("payload.json");
    let store_dir = dir.join("store");
    let config_path = dir.join("config.toml");

    let messages: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            serde_json::json!({
                "role": "user",
                "content": format!("question {i} {}", "lorem ipsum dolor sit amet ".repeat(30)),
            })
        })
        .collect();
    std::fs::write(
        &payload_path,
        serde_json::to_vec(&serde_json::json!({"model": "gpt-4o", "messages": messages})).unwrap(),
    )
    .unwrap();
    std::fs::write(
        &config_path,
        format!(
            "[retrieval]\nbackend = \"filesystem\"\nstore_path = {:?}\n",
            store_dir.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();

    let out = Command::new(bin())
        .args([
            "compress",
            payload_path.to_str().unwrap(),
            "--format",
            "openai",
            "--lossy",
            "heuristic",
            "--lossy-ratio",
            "0.2",
            "--json",
            "--config",
        ])
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert!(
        report["retrieval"].is_null(),
        "nothing may be persisted when the lossy stage cannot run, got: {:?}",
        report["retrieval"]
    );
    assert!(
        !store_dir.exists(),
        "no retrieval directory may be created either"
    );

    std::fs::remove_dir_all(&dir).ok();
}
