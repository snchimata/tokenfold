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
fn text_receipt_includes_savings_percentage() {
    let input = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/api_response.json"
    );
    let out = Command::new(bin())
        .args([
            "inspect",
            input,
            "--format",
            "json",
            "--receipt-format",
            "text",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("savings_pct: 63.9"), "stdout was: {stdout}");
}

/// Regression test for a bug an adversarial review caught: `--dry-run` routes to the same code
/// path as `inspect`, which used to hardcode `lossy: None` regardless of what the user passed,
/// silently making `--dry-run --lossy ...` preview nothing about the lossy stage at all.
/// Regression test for a real, confirmed bug: `--dry-run --lossy` (routing through `cmd_inspect`)
/// used to physically write to the retrieval store even though `inspect`/`--dry-run` is
/// documented as a side-effect-free preview -- forwarding the lossy flags into `cmd_inspect`
/// (the fix for the test above) reintroduced a real disk-write side effect that didn't exist
/// before lossy did. Uses an isolated `--config`-specified store path (not the shared
/// process-wide `XDG_DATA_HOME` from `bin()`) so this assertion can't be polluted by other tests
/// running concurrently.
/// Regression test for a real, confirmed bug: `compress --dry-run` routes to `cmd_inspect`,
/// which used to build its own overrides from scratch and hardcode `disable: []` regardless of
/// what the user actually passed to `--disable` -- a real run and its own `--dry-run` preview of
/// the SAME flags could show different active transforms, defeating the point of a preview.
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
fn init_and_uninit_claude_code_merge_project_mcp_config() {
    let dir = unique_temp_dir("claude_mcp_init");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".mcp.json"),
        br#"{"mcpServers":{"existing":{"command":"existing"}},"keep":true}"#,
    )
    .unwrap();

    let init = Command::new(bin())
        .current_dir(&dir)
        .args(["init", "--agent", "claude-code"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let configured: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(configured["keep"], true);
    assert_eq!(configured["mcpServers"]["existing"]["command"], "existing");
    assert_eq!(
        configured["mcpServers"]["tokenfold"]["command"],
        "tokenfold"
    );
    assert_eq!(
        configured["mcpServers"]["tokenfold"]["args"],
        serde_json::json!(["mcp", "serve"])
    );

    let uninit = Command::new(bin())
        .current_dir(&dir)
        .args(["uninit", "--agent", "claude"])
        .output()
        .unwrap();
    assert!(uninit.status.success());
    let configured: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join(".mcp.json")).unwrap()).unwrap();
    assert!(configured["mcpServers"].get("tokenfold").is_none());
    assert_eq!(configured["mcpServers"]["existing"]["command"], "existing");
}

#[test]
fn init_claude_code_dry_run_does_not_write_a_config() {
    let dir = unique_temp_dir("claude_mcp_init_dry_run");
    std::fs::create_dir_all(&dir).unwrap();

    let output = Command::new(bin())
        .current_dir(&dir)
        .args(["init", "--agent", "claude-code", "--dry-run"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!dir.join(".mcp.json").exists());
}

#[test]
fn init_claude_code_rejects_an_invalid_existing_config() {
    let dir = unique_temp_dir("claude_mcp_init_invalid");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".mcp.json"), b"[]").unwrap();

    let output = Command::new(bin())
        .current_dir(&dir)
        .args(["init", "--agent", "claude-code"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert!(String::from_utf8_lossy(&output.stderr).contains("root must be a JSON object"));
    assert_eq!(std::fs::read(dir.join(".mcp.json")).unwrap(), b"[]");
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

fn run_stdin(args: &[&str], input: &[u8]) -> std::process::Output {
    let mut child = Command::new(bin())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn v2_default_compress_has_pure_payload_and_json_receipt() {
    let input = br#"{"items":[{"id":1},{"id":2}]}"#;
    let out = run_stdin(&["compress", "--format", "json"], input);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(serde_json::from_slice::<serde_json::Value>(&out.stdout).is_ok());
    let receipt: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(receipt["schema_version"], "2.0");
    assert_eq!(receipt["preset"], "balanced");
    assert_eq!(receipt["budget"]["status"], "not_requested");
}

#[test]
fn v2_inspect_writes_only_receipt_and_can_redirect_it() {
    let input = br#"{"a": 1}"#;
    let out = run_stdin(&["inspect", "--format", "json"], input);
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["schema_version"],
        "2.0"
    );

    let path = std::env::temp_dir().join(format!("tokenfold_receipt_{}.json", std::process::id()));
    let mut child = Command::new(bin())
        .args(["inspect", "--format", "json", "--receipt-file"])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    assert!(serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).is_ok());
    std::fs::remove_file(path).ok();
}

#[test]
fn v2_hard_target_suppresses_payload_and_output_file() {
    let path = std::env::temp_dir().join(format!("tokenfold_hard_{}.json", std::process::id()));
    std::fs::remove_file(&path).ok();
    let mut child = Command::new(bin())
        .args([
            "compress",
            "--format",
            "json",
            "--target-tokens",
            "1",
            "--require-target",
            "--output",
        ])
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"items":[1,2,3,4]}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(7));
    assert!(out.stdout.is_empty());
    assert!(!path.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("budget unmet"));
}

#[test]
fn v2_toon_compress_decodes_to_original_json_value() {
    let input = br#"{"users":[{"id":1,"name":"Ada"},{"id":2,"name":"Lin"}]}"#;
    let compressed = run_stdin(
        &["compress", "--format", "json", "--encoding", "toon"],
        input,
    );
    assert!(
        compressed.status.success(),
        "{}",
        String::from_utf8_lossy(&compressed.stderr)
    );
    let decoded = run_stdin(&["decode", "--from", "toon"], &compressed.stdout);
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&decoded.stdout).unwrap(),
        serde_json::from_slice::<serde_json::Value>(input).unwrap(),
    );
}

#[test]
fn toon_metrics_example_round_trips() {
    let input = include_bytes!("../../../examples/toon_metrics.json");
    let compressed = run_stdin(
        &[
            "compress",
            "--format",
            "json",
            "--preset",
            "conservative",
            "--encoding",
            "toon",
        ],
        input,
    );
    assert!(
        compressed.status.success(),
        "{}",
        String::from_utf8_lossy(&compressed.stderr)
    );
    let decoded = run_stdin(&["decode", "--from", "toon"], &compressed.stdout);
    assert!(
        decoded.status.success(),
        "{}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&decoded.stdout).unwrap(),
        serde_json::from_slice::<serde_json::Value>(input).unwrap(),
    );
}

#[test]
fn v2_pruning_dependencies_and_removed_flags_fail_early() {
    let out = run_stdin(
        &["compress", "--format", "json", "--prune"],
        br#"{"a":[1,2]}"#,
    );
    assert_eq!(out.status.code(), Some(5));
    for removed in [
        "--mode",
        "--lossy",
        "--dry-run",
        "--unsafe-disable-redaction",
    ] {
        let out = run_stdin(&["compress", removed], b"x");
        assert_eq!(out.status.code(), Some(2), "{removed}");
    }
}

#[test]
fn v2_missing_retrieval_reference_exits_eight() {
    let out = Command::new(bin())
        .args(["retrieve", &"a".repeat(64)])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(8));
}
