//! F-046 CLI glue for `stats`/`gain`/`session`: ad-hoc report-glob expansion, report-file ->
//! `LedgerRecord` conversion, and JSON/CSV/human printing. The actual aggregation logic lives in
//! `tokenfold_core::stats::aggregate` — this module only feeds it and renders its output.

use std::path::{Path, PathBuf};

use tokenfold_core::TokenFoldError;
use tokenfold_core::report::CompressionReport;
use tokenfold_core::stats::{self, LedgerRecord, StatsSummary};

/// Expands a glob-ish pattern to matching file paths, sorted for determinism. Supports `*`
/// (any run of characters) and `?` (any single character) in the final path segment only — no
/// recursive `**`, which this pass doesn't need (`reports/*.json` covers the documented use
/// case). A pattern that names an existing file directly (no wildcard needed) is returned as-is
/// without touching the filesystem's directory listing at all.
pub fn expand_glob(pattern: &str) -> Result<Vec<PathBuf>, TokenFoldError> {
    let path = Path::new(pattern);
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let (dir, file_pattern) = match pattern.rfind(['/', '\\']) {
        Some(idx) => (&pattern[..idx], &pattern[idx + 1..]),
        None => ("", pattern),
    };
    let dir = if dir.is_empty() { "." } else { dir };

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if glob_match(file_pattern, &name) {
            matches.push(entry.path());
        }
    }
    matches.sort();
    Ok(matches)
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    glob_match_rec(&p, &n)
}

fn glob_match_rec(p: &[char], n: &[char]) -> bool {
    match p.first() {
        None => n.is_empty(),
        Some('*') => glob_match_rec(&p[1..], n) || (!n.is_empty() && glob_match_rec(p, &n[1..])),
        Some('?') => !n.is_empty() && glob_match_rec(&p[1..], &n[1..]),
        Some(c) => !n.is_empty() && n[0] == *c && glob_match_rec(&p[1..], &n[1..]),
    }
}

/// Reads and parses one ad-hoc `CompressionReport` JSON file into a `LedgerRecord`, plus its
/// own `retrieval.marker_count` (the one retrieval figure this pass can honestly report — see
/// `tokenfold_core::stats` module doc). The request ID is derived from the file's content hash
/// (repeated `stats` runs over the same files reproduce the same ID); the timestamp is the
/// file's mtime, falling back to "now" when unavailable.
pub fn record_from_report_file(path: &Path) -> Result<(LedgerRecord, usize), TokenFoldError> {
    let bytes = std::fs::read(path)?;
    let report: CompressionReport = serde_json::from_slice(&bytes).map_err(|e| {
        TokenFoldError::InvalidInput(format!(
            "{} is not a valid CompressionReport JSON file: {e}",
            path.display()
        ))
    })?;
    let hash = tokenfold_core::retrieval_store::hex_sha256(&bytes);
    let request_id = format!("tc-{}", &hash[..8]);
    let timestamp = file_mtime_timestamp(path);
    let markers = report
        .retrieval
        .as_ref()
        .map(|r| r.marker_count)
        .unwrap_or(0);
    let record = stats::record_from_report(&report, request_id, timestamp, None);
    Ok((record, markers))
}

fn file_mtime_timestamp(path: &Path) -> String {
    let unix = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_else(stats::now_unix);
    stats::format_unix_timestamp(unix)
}

/// Dispatches to JSON, CSV, or a short human summary, in that precedence order (both flags set
/// prefers JSON, since `--json` is a global flag shared by every subcommand).
pub fn print_summary(summary: &StatsSummary, json: bool, csv: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(summary).unwrap());
    } else if csv {
        print!("{}", stats::to_csv(summary));
    } else {
        print!("{}", render_human(summary));
    }
}

fn render_human(summary: &StatsSummary) -> String {
    format!(
        "tokenfold {} stats ({})\n  requests: {}  commands: {} (wrapped {}, raw {})  bypassed: {}\n  tokens: {} -> {} (saved {}, {:.1}%)\n  coverage: {:.1}%  estimated lost tokens: {}\n  recent requests: {}\n",
        summary.scope,
        summary.window,
        summary.requests,
        summary.commands,
        summary.wrapped_commands,
        summary.raw_commands,
        summary.bypass_count,
        summary.raw_tokens,
        summary.compressed_tokens,
        summary.saved_tokens,
        summary.savings_pct,
        summary.coverage_pct,
        summary.estimated_lost_tokens,
        summary.recent_requests.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "tokenfold_stats_cmd_test_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn expand_glob_matches_wildcard_within_a_directory() {
        let dir = unique_dir("glob");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), b"{}").unwrap();
        std::fs::write(dir.join("b.json"), b"{}").unwrap();
        std::fs::write(dir.join("c.txt"), b"nope").unwrap();

        let pattern = dir.join("*.json").to_string_lossy().replace('\\', "/");
        let matches = expand_glob(&pattern).unwrap();
        assert_eq!(matches.len(), 2);
        assert!(
            matches
                .iter()
                .all(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expand_glob_returns_an_existing_literal_file_directly() {
        let dir = unique_dir("literal");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("report.json");
        std::fs::write(&file, b"{}").unwrap();

        let matches = expand_glob(file.to_str().unwrap()).unwrap();
        assert_eq!(matches, vec![file.clone()]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expand_glob_on_a_nonexistent_directory_returns_no_matches() {
        let matches = expand_glob("/no/such/directory/*.json").unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn record_from_report_file_rejects_invalid_json() {
        let dir = unique_dir("invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("bad.json");
        std::fs::write(&file, b"not json").unwrap();

        let err = record_from_report_file(&file).unwrap_err();
        assert!(matches!(err, TokenFoldError::InvalidInput(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_from_report_file_extracts_marker_count_and_a_stable_request_id() {
        use tokenfold_core::report::{CompressionReport, EstimatorInfo, RetrievalReport};
        use tokenfold_core::status::Status;

        let mut report = CompressionReport::new(
            1000,
            600,
            EstimatorInfo {
                backend: "heuristic".to_string(),
                model: None,
                is_exact: false,
            },
            Status::Compressed,
            "balanced".to_string(),
            "plain_text".to_string(),
            "general".to_string(),
            vec![],
            vec![],
        );
        report.retrieval = Some(RetrievalReport {
            store_namespace: "default".to_string(),
            hash_algorithm: "sha256".to_string(),
            marker_count: 2,
            ttl_seconds: Some(60),
            persisted_original_bytes: 1000,
            skipped_original_bytes: 0,
        });

        let dir = unique_dir("markers");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("report.json");
        std::fs::write(&file, serde_json::to_vec(&report).unwrap()).unwrap();

        let (record, markers) = record_from_report_file(&file).unwrap();
        assert_eq!(markers, 2);
        assert_eq!(record.original_tokens, 1000);
        assert!(record.request_id.starts_with("tc-"));

        // Re-reading the same file yields the same request ID (content-derived, not random).
        let (record_again, _) = record_from_report_file(&file).unwrap();
        assert_eq!(record.request_id, record_again.request_id);

        std::fs::remove_dir_all(&dir).ok();
    }
}
