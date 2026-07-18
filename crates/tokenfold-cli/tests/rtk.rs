//! F-054 / F-055 integration tests for `wrap --rtk` composition.
//!
//! RTK is faked with a tiny script the test writes and points `TOKENFOLD_RTK_BIN` at, driven by
//! env vars (`FAKE_RTK_VERSION`, `FAKE_RTK_STDOUT`, `FAKE_RTK_EXIT`, `FAKE_RTK_TEE`) that the
//! test sets on the tokenfold child and tokenfold passes through to RTK. Covers the ENGINEERING
//! v0.3 RTK gate list: missing/incompatible preflight fail-open, staged receipts, double-filter
//! avoidance, nonzero-child propagation, and CCR complete/secret/absent capture.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_tokenfold")
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn unique_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("tf_rtk_{tag}_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const FAKE_RTK_SH: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "${FAKE_RTK_VERSION:-rtk 0.4.1}"
  exit 0
fi
if [ -n "$RTK_TEE_DIR" ] && [ -n "$FAKE_RTK_TEE" ]; then
  printf '%s' "$FAKE_RTK_TEE" > "$RTK_TEE_DIR/raw.log"
fi
printf '%s' "${FAKE_RTK_STDOUT:-FILTERED}"
exit "${FAKE_RTK_EXIT:-0}"
"#;

const FAKE_RTK_CMD: &str = "@echo off\r\n\
if \"%~1\"==\"--version\" (\r\n\
  if defined FAKE_RTK_VERSION (echo %FAKE_RTK_VERSION%) else (echo rtk 0.4.1)\r\n\
  exit /b 0\r\n\
)\r\n\
if defined RTK_TEE_DIR if defined FAKE_RTK_TEE >\"%RTK_TEE_DIR%\\raw.log\" echo %FAKE_RTK_TEE%\r\n\
if defined FAKE_RTK_STDOUT (echo %FAKE_RTK_STDOUT%) else (echo FILTERED)\r\n\
if defined FAKE_RTK_EXIT exit /b %FAKE_RTK_EXIT%\r\n\
exit /b 0\r\n";

/// Writes a platform-appropriate fake `rtk` into `dir` and returns its path.
fn fake_rtk(dir: &Path) -> PathBuf {
    if cfg!(windows) {
        let p = dir.join("rtk.cmd");
        std::fs::write(&p, FAKE_RTK_CMD).unwrap();
        p
    } else {
        let p = dir.join("rtk");
        std::fs::write(&p, FAKE_RTK_SH).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }
}

/// A tokenfold command with an isolated data home, so the ledger/retrieval store never touch the
/// developer's real home directory.
fn tf(dir: &Path) -> Command {
    let mut c = Command::new(bin());
    c.env("XDG_DATA_HOME", dir.join("xdg"))
        .env("HOME", dir.join("home"));
    c
}

fn report_from_stderr(stderr: &[u8]) -> Value {
    serde_json::from_slice(stderr)
        .unwrap_or_else(|e| panic!("stderr not JSON ({e}): {}", String::from_utf8_lossy(stderr)))
}

/// Recursively collects `*.bin` files under `root` (the retrieval store layout).
fn stored_bins(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(stored_bins(&p));
            } else if p.extension().is_some_and(|x| x == "bin") {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn rtk_missing_falls_open_to_tokenfold_only() {
    let dir = unique_dir("missing");
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", dir.join("does-not-exist"))
        .args(["wrap", "--rtk", "--json", "--", "git", "--version"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report_from_stderr(&out.stderr);
    let stages = report["pipeline"]["stages"].as_array().unwrap();
    assert_eq!(stages[0]["id"], "rtk");
    assert_eq!(stages[0]["status"], "unavailable");
    assert_eq!(stages[1]["id"], "tokenfold");
    // The child still ran (tokenfold-only path), so a payload was produced.
    assert!(!out.stdout.is_empty());
}

#[test]
fn rtk_incompatible_version_falls_open() {
    let dir = unique_dir("incompat");
    let rtk = fake_rtk(&dir);
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", &rtk)
        .env("FAKE_RTK_VERSION", "garbage-no-version")
        .args(["wrap", "--rtk", "--json", "--", "git", "--version"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report = report_from_stderr(&out.stderr);
    assert_eq!(report["pipeline"]["stages"][0]["status"], "incompatible");
}

#[test]
fn rtk_runs_and_composes_staged_receipt() {
    let dir = unique_dir("compose");
    let rtk = fake_rtk(&dir);
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", &rtk)
        .args(["wrap", "--rtk", "--json", "--", "git", "--version"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report_from_stderr(&out.stderr);
    let stages = report["pipeline"]["stages"].as_array().unwrap();
    assert_eq!(stages[0]["id"], "rtk");
    assert_eq!(stages[0]["status"], "applied");
    assert_eq!(stages[0]["provenance"], "external:rtk@0.4.1");
    assert_eq!(stages[1]["id"], "tokenfold");
    // tokenfold compressed RTK's filtered output, never git's real output.
    let payload = String::from_utf8_lossy(&out.stdout);
    assert!(payload.contains("FILTERED"), "payload: {payload}");
    assert!(
        !payload.contains("git version"),
        "tokenfold must see RTK output, not raw git: {payload}"
    );
}

#[test]
fn rtk_run_skips_overlapping_tokenfold_filter() {
    let dir = unique_dir("nofilter");
    let rtk = fake_rtk(&dir);
    // `git diff` matches a built-in tokenfold filter pack; --rtk must skip it (no double filter).
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", &rtk)
        .args(["wrap", "--rtk", "--json", "--", "git", "diff"])
        .output()
        .unwrap();
    let report = report_from_stderr(&out.stderr);
    assert!(
        report["command"]["filter_pack_id"].is_null(),
        "tokenfold filter must be skipped when RTK ran: {}",
        report["command"]
    );
}

#[test]
fn rtk_nonzero_child_exit_is_propagated_and_output_preserved() {
    let dir = unique_dir("exit");
    let rtk = fake_rtk(&dir);
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", &rtk)
        .env("FAKE_RTK_EXIT", "3")
        .args(["wrap", "--rtk", "--", "git", "--version"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(!out.stdout.is_empty());
}

#[test]
fn ccr_complete_capture_persists_and_reports_full() {
    let dir = unique_dir("ccr_ok");
    let rtk = fake_rtk(&dir);
    let store = dir.join("store");
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", &rtk)
        .env(
            "FAKE_RTK_TEE",
            "the complete pre-RTK raw output, longer than the filtered stdout",
        )
        .env("TOKENFOLD_RETRIEVAL_STORE_PATH", &store)
        .args([
            "wrap",
            "--rtk",
            "--rtk-capture-raw",
            "--json",
            "--",
            "git",
            "--version",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report_from_stderr(&out.stderr);
    let pipeline = &report["pipeline"];
    assert_eq!(pipeline["raw_capture"], "complete");
    assert_eq!(pipeline["upstream_recoverability"], "full");
    assert_eq!(pipeline["stages"][0]["recoverability"], "full");
    assert!(!pipeline["stages"][0]["evidence_ref"].is_null());
    assert!(pipeline["raw_input_bytes"].as_u64().unwrap() > 0);
    assert!(
        !stored_bins(&store).is_empty(),
        "expected a stored raw capture under {store:?}"
    );
}

#[test]
fn ccr_secret_capture_is_never_persisted() {
    let dir = unique_dir("ccr_secret");
    let rtk = fake_rtk(&dir);
    let store = dir.join("store");
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", &rtk)
        // AWS-access-key-shaped secret, longer than the filtered stdout so capture is "complete".
        .env(
            "FAKE_RTK_TEE",
            "AKIAIOSFODNN7EXAMPLE plus more raw output text",
        )
        .env("TOKENFOLD_RETRIEVAL_STORE_PATH", &store)
        .args([
            "wrap",
            "--rtk",
            "--rtk-capture-raw",
            "--json",
            "--",
            "git",
            "--version",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report = report_from_stderr(&out.stderr);
    let pipeline = &report["pipeline"];
    // Capture was complete, but persistence was refused -> unrecoverable.
    assert_eq!(pipeline["upstream_recoverability"], "none");
    assert_eq!(pipeline["stages"][0]["recoverability"], "none");
    let reason = pipeline["stages"][0]["bypass_reason"]
        .as_str()
        .unwrap_or_default();
    assert!(reason.contains("secret"), "bypass_reason: {reason}");
    assert!(
        stored_bins(&store).is_empty(),
        "secret-matched bytes must never be persisted"
    );
}

#[test]
fn ccr_absent_capture_reports_unrecoverable() {
    let dir = unique_dir("ccr_absent");
    let rtk = fake_rtk(&dir); // FAKE_RTK_TEE unset -> RTK writes no tee file.
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", &rtk)
        .args([
            "wrap",
            "--rtk",
            "--rtk-capture-raw",
            "--json",
            "--",
            "git",
            "--version",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let pipeline = report_from_stderr(&out.stderr)["pipeline"].clone();
    assert_eq!(pipeline["raw_capture"], "unavailable");
    assert_eq!(pipeline["upstream_recoverability"], "none");
    assert!(pipeline["raw_input_bytes"].is_null());
}

#[test]
fn doctor_reports_rtk_missing_without_failing() {
    let dir = unique_dir("doc_missing");
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", dir.join("nope"))
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["rtk"]["status"], "missing");
    assert_eq!(report["rtk"]["compatible"], false);
}

#[test]
fn doctor_reports_rtk_available_with_capture_guidance() {
    let dir = unique_dir("doc_avail");
    let rtk = fake_rtk(&dir);
    let out = tf(&dir)
        .env("TOKENFOLD_RTK_BIN", &rtk)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["rtk"]["status"], "available");
    assert_eq!(report["rtk"]["compatible"], true);
    assert_eq!(report["rtk"]["version"], "0.4.1");
    // Tee is off by default; capture is not ready until the user enables RTK's [tee] section.
    assert_eq!(report["rtk"]["raw_capture"], "unavailable");
}
