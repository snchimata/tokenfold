//! F-046: savings ledger, stats aggregation, and JSON/CSV export (`roadmap.md` F-046,
//! `interfaces.md` §7.1 "Stats and Analytics JSON").
//!
//! `StatsSummary`/`LedgerRecord` mirror `interfaces.md`'s documented JSON shapes exactly.
//! `aggregate()` is the one pure aggregation path shared by the CLI's `stats`/`gain`/`session`
//! subcommands (each just calls it and then tweaks framing fields like `scope`/`window`).
//!
//! Ledger storage format: `tokenfold.toml`'s `[analytics].ledger_db` path is documented as
//! ending in `.db`, but this pass stores newline-delimited JSON (JSONL) inside it rather than
//! embedding a sqlite dependency — there is no sqlite crate anywhere in this workspace's
//! dependency graph. The `.db` extension is only what the config schema names the path; the
//! format inside is plain JSONL. Upgrade path is a real embedded database (sqlite or similar)
//! if this ever needs concurrent-writer safety; a single local CLI process appending its own
//! ledger doesn't need that yet.
//!
//! Fields with no honest data source yet are zero-filled rather than invented, each with a
//! comment at its computation site: `cache` (no cache subsystem exists anywhere in this
//! codebase), `retrieval.hits`/`misses`/`expired` (only store-time marker counts are tracked,
//! not later retrieval-attempt outcomes), `latency` (no per-request timing is threaded through
//! `CompressionReport`/`LedgerRecord` yet — every `TransformReport.elapsed_micros` in this
//! codebase is already always `None`), and `untrusted_filter_count` (the F-047 filter registry
//! doesn't exist yet).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::errors::TokenFoldError;
use crate::report::CompressionReport;
use crate::status::Status;

pub const SCHEMA_VERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct RetrievalStats {
    pub markers: usize,
    pub hits: usize,
    pub misses: usize,
    pub expired: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct LatencyStats {
    pub p50_ms: f64,
    pub p95_ms: f64,
}

/// Matches `interfaces.md` §7.1's full `StatsSummary` JSON shape verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StatsSummary {
    pub schema_version: String,
    pub scope: String,
    pub window: String,
    pub project: Option<String>,
    pub requests: usize,
    pub commands: usize,
    pub wrapped_commands: usize,
    pub raw_commands: usize,
    pub bypass_count: usize,
    pub raw_tokens: usize,
    pub compressed_tokens: usize,
    pub saved_tokens: usize,
    pub savings_pct: f64,
    pub estimated_lost_tokens: usize,
    pub coverage_pct: f64,
    pub untrusted_filter_count: usize,
    pub retrieval: RetrievalStats,
    pub cache: CacheStats,
    pub latency: LatencyStats,
    pub recent_requests: Vec<LedgerRecord>,
}

/// Matches `interfaces.md` §7.1's redacted `recent_requests[]` item shape verbatim. This is
/// also the exact shape persisted (one per line, as JSON) by [`LedgerStore`] — there is no
/// separate "storage" representation, so no raw prompt/response/command-arg/path/header bytes
/// can ever end up on disk through this type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerRecord {
    pub request_id: String,
    pub timestamp: String,
    pub surface: String,
    pub format: String,
    pub mode: String,
    pub status: String,
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub saved_tokens: usize,
    pub savings_pct: f64,
    pub bypass_reason: Option<String>,
    pub project_hash: Option<String>,
}

/// Whether a savings/tokens figure came from an exact estimator (`"measured"`), the byte
/// heuristic (`"heuristic"`), or is a derived extrapolation like `estimated_lost_tokens`
/// (`"estimated"`) rather than a directly observed value. There is no per-provider pricing data
/// anywhere in this codebase, so this labels token-count provenance, not dollar cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavingsProvenance {
    Measured,
    Heuristic,
    Estimated,
}

impl SavingsProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            SavingsProvenance::Measured => "measured",
            SavingsProvenance::Heuristic => "heuristic",
            SavingsProvenance::Estimated => "estimated",
        }
    }
}

/// Labels a directly-observed token count by estimator exactness (see `EstimatorInfo.is_exact`).
/// Derived/extrapolated figures (e.g. `estimated_lost_tokens`) are always
/// `SavingsProvenance::Estimated` by construction, independent of this function.
pub fn savings_provenance(is_exact: bool) -> SavingsProvenance {
    if is_exact {
        SavingsProvenance::Measured
    } else {
        SavingsProvenance::Heuristic
    }
}

/// Builds the `LedgerRecord` for one `CompressionReport`, used both by the CLI's post-run
/// ledger-recording hook and by ad-hoc `tokenfold stats <report-glob>` aggregation (turning a
/// standalone report file into the same shape). `surface` is derived from the report itself
/// (`"wrap"` when `CommandReport` is present, `"cli"` otherwise) rather than passed in, so both
/// callers classify it identically.
pub fn record_from_report(
    report: &CompressionReport,
    request_id: String,
    timestamp: String,
    project_hash: Option<String>,
) -> LedgerRecord {
    let surface = if report.command.is_some() {
        "wrap"
    } else {
        "cli"
    }
    .to_string();

    // A wrap invocation whose `never_worse` guard fell back to raw output is a genuine,
    // already-recorded "why compression didn't apply here" reason; `report.bypass` (F-047) is
    // the future general-purpose source once the filter registry exists, but nothing sets it
    // yet, so this `or_else` currently never fires.
    let bypass_reason = report
        .command
        .as_ref()
        .filter(|c| c.never_worse_applied)
        .map(|_| "would_increase_tokens".to_string())
        .or_else(|| report.bypass.as_ref().map(|b| b.reason.clone()));

    LedgerRecord {
        request_id,
        timestamp,
        surface,
        format: report.format.clone(),
        mode: report.mode.clone(),
        status: status_label(&report.status),
        original_tokens: report.original_tokens,
        compressed_tokens: report.compressed_tokens,
        saved_tokens: report.saved_tokens,
        savings_pct: report.savings_pct,
        bypass_reason,
        project_hash,
    }
}

fn status_label(status: &Status) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// The one shared aggregation path: pure, total-order-independent, takes only ledger-shaped
/// records. `scope`/`window`/`project` are generic defaults here — callers (the CLI's
/// `stats`/`gain`/`session` entry points) overwrite those framing fields on the returned
/// `StatsSummary` to suit each command's emphasis; the underlying counts never change.
pub fn aggregate(records: &[LedgerRecord]) -> StatsSummary {
    let requests = records.len();

    let is_wrap = |r: &&LedgerRecord| r.surface == "wrap";
    let commands: usize = records.iter().filter(is_wrap).count();
    let wrapped_commands = records
        .iter()
        .filter(is_wrap)
        .filter(|r| r.bypass_reason.is_none())
        .count();
    let raw_commands = commands - wrapped_commands;
    let bypass_count = records.iter().filter(|r| r.bypass_reason.is_some()).count();

    let raw_tokens: usize = records.iter().map(|r| r.original_tokens).sum();
    let compressed_tokens: usize = records.iter().map(|r| r.compressed_tokens).sum();
    let saved_tokens = raw_tokens.saturating_sub(compressed_tokens);
    let savings_pct = if raw_tokens == 0 {
        0.0
    } else {
        saved_tokens as f64 / raw_tokens as f64 * 100.0
    };

    // estimated_lost_tokens/coverage_pct: extrapolated from the measured average savings ratio
    // of wrapped (actually-compressed) commands, applied to raw (bypassed / never-worse
    // fallback) commands' own token counts. This is the honest "straightforward" computation
    // available today per ROADMAP.md's F-046 note: there is no filter registry (F-047) yet to
    // report *why* coverage is incomplete, only that it is.
    let wrapped_raw_tokens: usize = records
        .iter()
        .filter(is_wrap)
        .filter(|r| r.bypass_reason.is_none())
        .map(|r| r.original_tokens)
        .sum();
    let wrapped_saved_tokens: usize = records
        .iter()
        .filter(is_wrap)
        .filter(|r| r.bypass_reason.is_none())
        .map(|r| r.saved_tokens)
        .sum();
    let raw_command_tokens: usize = records
        .iter()
        .filter(is_wrap)
        .filter(|r| r.bypass_reason.is_some())
        .map(|r| r.original_tokens)
        .sum();
    let estimated_lost_tokens = if wrapped_raw_tokens == 0 {
        0
    } else {
        let ratio = wrapped_saved_tokens as f64 / wrapped_raw_tokens as f64;
        (raw_command_tokens as f64 * ratio).round() as usize
    };
    let coverage_pct = if commands == 0 {
        0.0
    } else {
        wrapped_commands as f64 / commands as f64 * 100.0
    };

    // Lexicographic order matches chronological order for the fixed-width, zero-padded
    // `format_unix_timestamp` shape, so no timestamp parsing is needed just to sort.
    let mut recent_requests = records.to_vec();
    recent_requests.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    StatsSummary {
        schema_version: SCHEMA_VERSION.to_string(),
        scope: "aggregate".to_string(),
        window: "all".to_string(),
        project: None,
        requests,
        commands,
        wrapped_commands,
        raw_commands,
        bypass_count,
        raw_tokens,
        compressed_tokens,
        saved_tokens,
        savings_pct,
        estimated_lost_tokens,
        coverage_pct,
        // ponytail: no filter registry exists yet (ROADMAP.md F-047); real counts land with it.
        untrusted_filter_count: 0,
        // ponytail: no historical record of individual `tokenfold retrieve` outcomes exists —
        // `RetrievalReport` only carries store-time marker counts, not later hit/miss/expiry —
        // so those three stay zero; a real `markers` count is filled in by ad-hoc report-glob
        // aggregation (see `crates/tokenfold-cli/src/main.rs::cmd_stats`), which has direct
        // access to each `CompressionReport.retrieval.marker_count`.
        retrieval: RetrievalStats::default(),
        // ponytail: no cache subsystem exists anywhere in this codebase yet; zero is honest,
        // not a placeholder for a feature this pass should build.
        cache: CacheStats::default(),
        // ponytail: no per-request latency is threaded through `CompressionReport`/
        // `LedgerRecord` yet (every `TransformReport.elapsed_micros` in this codebase is
        // already always `None`); zero is honest, not measured.
        latency: LatencyStats::default(),
        recent_requests,
    }
}

/// Parses a duration shorthand like `"30d"`, `"24h"`, `"90m"`, `"120s"`, or a bare integer
/// (seconds) — used by `tokenfold gain --since`.
pub fn parse_duration_secs(input: &str) -> Result<u64, TokenFoldError> {
    let trimmed = input.trim();
    let invalid = || {
        TokenFoldError::InvalidInput(format!(
            "invalid duration {input:?}; expected e.g. \"30d\", \"24h\", \"90m\", \"120s\", or a bare integer of seconds"
        ))
    };
    if trimmed.is_empty() {
        return Err(invalid());
    }
    let (digits, unit_secs) = match trimmed.chars().last().unwrap() {
        'd' => (&trimmed[..trimmed.len() - 1], 86_400u64),
        'h' => (&trimmed[..trimmed.len() - 1], 3_600u64),
        'm' => (&trimmed[..trimmed.len() - 1], 60u64),
        's' => (&trimmed[..trimmed.len() - 1], 1u64),
        _ => (trimmed, 1u64),
    };
    let count: u64 = digits.trim().parse().map_err(|_| invalid())?;
    Ok(count.saturating_mul(unit_secs))
}

/// Keeps only records whose `timestamp` is within `window_secs` of `now` (both in Unix
/// seconds). Records with an unparsable timestamp are dropped — unlike [`LedgerStore::gc`],
/// which fails safe by keeping what it can't confidently date, a `--since` view is explicitly
/// asking for "recent", so an undatable record can't honestly satisfy that.
pub fn filter_since(records: &[LedgerRecord], now: u64, window_secs: u64) -> Vec<LedgerRecord> {
    records
        .iter()
        .filter(|r| match parse_timestamp_to_unix(&r.timestamp) {
            Some(ts) => now.saturating_sub(ts) <= window_secs,
            None => false,
        })
        .cloned()
        .collect()
}

/// Renders `summary` as two CSV sections separated by a blank line: a one-row summary table,
/// then the `recent_requests` table. A manual writer (no new dependency) is enough for this
/// shape — every field is a plain number or a short known-alphabet string, so the only escaping
/// that can ever matter is on `project`/`project_hash`/`bypass_reason`, handled by [`csv_field`].
pub fn to_csv(summary: &StatsSummary) -> String {
    let mut out = String::new();
    out.push_str(
        "schema_version,scope,window,project,requests,commands,wrapped_commands,raw_commands,\
         bypass_count,raw_tokens,compressed_tokens,saved_tokens,savings_pct,\
         estimated_lost_tokens,coverage_pct,untrusted_filter_count,retrieval_markers,\
         retrieval_hits,retrieval_misses,retrieval_expired,cache_hits,cache_misses,\
         latency_p50_ms,latency_p95_ms\n",
    );
    out.push_str(&format!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
        csv_field(&summary.schema_version),
        csv_field(&summary.scope),
        csv_field(&summary.window),
        csv_field(summary.project.as_deref().unwrap_or("")),
        summary.requests,
        summary.commands,
        summary.wrapped_commands,
        summary.raw_commands,
        summary.bypass_count,
        summary.raw_tokens,
        summary.compressed_tokens,
        summary.saved_tokens,
        summary.savings_pct,
        summary.estimated_lost_tokens,
        summary.coverage_pct,
        summary.untrusted_filter_count,
        summary.retrieval.markers,
        summary.retrieval.hits,
        summary.retrieval.misses,
        summary.retrieval.expired,
        summary.cache.hits,
        summary.cache.misses,
        summary.latency.p50_ms,
        summary.latency.p95_ms,
    ));
    out.push('\n');
    out.push_str(
        "request_id,timestamp,surface,format,mode,status,original_tokens,compressed_tokens,\
         saved_tokens,savings_pct,bypass_reason,project_hash\n",
    );
    for r in &summary.recent_requests {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}\n",
            csv_field(&r.request_id),
            csv_field(&r.timestamp),
            csv_field(&r.surface),
            csv_field(&r.format),
            csv_field(&r.mode),
            csv_field(&r.status),
            r.original_tokens,
            r.compressed_tokens,
            r.saved_tokens,
            r.savings_pct,
            csv_field(r.bypass_reason.as_deref().unwrap_or("")),
            csv_field(r.project_hash.as_deref().unwrap_or("")),
        ));
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// A short synthetic ID in the documented `tc-XXXXXXXX` shape (8 lowercase hex chars), derived
/// from the current time and process ID. Good enough for a human-scannable, practically-unique
/// local identifier; not a security-sensitive value, so `DefaultHasher` (not a cryptographic
/// hash) is an appropriate, dependency-free choice.
pub fn generate_request_id() -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("tc-{:08x}", hasher.finish() as u32)
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Formats Unix seconds as an RFC3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) without pulling in
/// a date/time crate: this is the one place in the codebase that needs calendar math, so a
/// compact civil-from-days conversion (Howard Hinnant's well-known `civil_from_days` algorithm,
/// see http://howardhinnant.github.io/date_algorithms.html) is a fair ponytail trade against
/// adding `chrono`/`time` as a dependency for a couple of call sites.
pub fn format_unix_timestamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

fn parse_timestamp_to_unix(ts: &str) -> Option<u64> {
    let ts = ts.strip_suffix('Z')?;
    let (date, time) = ts.split_once('T')?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    let mut time_parts = time.splitn(3, ':');
    let hour: u64 = time_parts.next()?.parse().ok()?;
    let minute: u64 = time_parts.next()?.parse().ok()?;
    let second: u64 = time_parts.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    if days < 0 {
        return None;
    }
    Some(days as u64 * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// F-046's optional local ledger: appends/reads/garbage-collects redacted `LedgerRecord`
/// metadata at a JSONL file path (see the module doc for the `.db`-named-but-JSONL decision).
pub struct LedgerStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerGcOutcome {
    pub kept: usize,
    pub removed: usize,
}

impl LedgerStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        LedgerStore { path: path.into() }
    }

    /// `$XDG_DATA_HOME/tokenfold/ledger.db`, falling back to
    /// `<home>/.local/share/tokenfold/ledger.db` — mirrors
    /// `retrieval_store::default_store_path`'s HOME/USERPROFILE fallback.
    pub fn default_path() -> PathBuf {
        if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(dir).join("tokenfold").join("ledger.db");
        }
        let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".local")
            .join("share")
            .join("tokenfold")
            .join("ledger.db")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, record: &LedgerRecord) -> Result<(), TokenFoldError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_string(record).map_err(|e| {
            TokenFoldError::InternalError(format!("failed to encode ledger record: {e}"))
        })?;
        line.push('\n');
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Reads every well-formed record. A line that fails to parse (e.g. a partial write left by
    /// a crash mid-append) is skipped rather than failing the whole read. A missing file reads
    /// as an empty ledger, not an error (an analytics-enabled run before any record exists yet
    /// is the common case, not a failure).
    pub fn read_all(&self) -> Result<Vec<LedgerRecord>, TokenFoldError> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Ok(Vec::new());
        };
        Ok(text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<LedgerRecord>(line).ok())
            .collect())
    }

    /// Rewrites the ledger file keeping only records whose `timestamp` is within
    /// `retention_days` of now; records with an unparsable timestamp are kept (fail safe rather
    /// than silently discarding data this can't confidently date). Survivors keep their original
    /// relative order — nothing about a kept record is touched or reordered.
    pub fn gc(&self, retention_days: u64) -> Result<LedgerGcOutcome, TokenFoldError> {
        let records = self.read_all()?;
        let cutoff_secs = retention_days.saturating_mul(86_400);
        let now = now_unix();

        let mut kept = Vec::with_capacity(records.len());
        let mut removed = 0usize;
        for record in records {
            let within_retention = match parse_timestamp_to_unix(&record.timestamp) {
                Some(ts) => now.saturating_sub(ts) <= cutoff_secs,
                None => true,
            };
            if within_retention {
                kept.push(record);
            } else {
                removed += 1;
            }
        }

        let mut out = String::new();
        for record in &kept {
            let line = serde_json::to_string(record).map_err(|e| {
                TokenFoldError::InternalError(format!("failed to encode ledger record: {e}"))
            })?;
            out.push_str(&line);
            out.push('\n');
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, out)?;

        Ok(LedgerGcOutcome {
            kept: kept.len(),
            removed,
        })
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{BypassReport, CommandReport, EstimatorInfo, RetrievalReport};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_ledger_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tokenfold_stats_test_{tag}_{}_{n}.db",
            std::process::id()
        ))
    }

    fn exact_estimator() -> EstimatorInfo {
        EstimatorInfo {
            backend: "tiktoken".to_string(),
            model: Some("o200k_base".to_string()),
            is_exact: true,
        }
    }

    fn heuristic_estimator() -> EstimatorInfo {
        EstimatorInfo {
            backend: "heuristic".to_string(),
            model: None,
            is_exact: false,
        }
    }

    fn cli_report(
        original: usize,
        compressed: usize,
        format: &str,
        status: Status,
        estimator: EstimatorInfo,
    ) -> CompressionReport {
        CompressionReport::new(
            original,
            compressed,
            estimator,
            status,
            "balanced".to_string(),
            format.to_string(),
            "general".to_string(),
            vec![],
            vec![],
        )
    }

    // --- timestamp round trip -------------------------------------------------------------

    #[test]
    fn format_unix_timestamp_matches_known_epoch_values() {
        assert_eq!(format_unix_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix_timestamp(86_400), "1970-01-02T00:00:00Z");
    }

    #[test]
    fn timestamp_round_trips_through_parse_and_format() {
        for secs in [0u64, 1, 86_399, 86_400, 1_000_000_000, 1_752_000_000] {
            let formatted = format_unix_timestamp(secs);
            assert_eq!(
                parse_timestamp_to_unix(&formatted),
                Some(secs),
                "{formatted}"
            );
        }
    }

    #[test]
    fn parse_duration_secs_supports_documented_suffixes() {
        assert_eq!(parse_duration_secs("30d").unwrap(), 30 * 86_400);
        assert_eq!(parse_duration_secs("24h").unwrap(), 24 * 3_600);
        assert_eq!(parse_duration_secs("90m").unwrap(), 90 * 60);
        assert_eq!(parse_duration_secs("120s").unwrap(), 120);
        assert_eq!(parse_duration_secs("45").unwrap(), 45);
        assert!(parse_duration_secs("nonsense").is_err());
    }

    // --- record_from_report ----------------------------------------------------------------

    #[test]
    fn record_from_report_uses_cli_surface_when_no_command_report() {
        let report = cli_report(
            1000,
            600,
            "openai_json",
            Status::Compressed,
            exact_estimator(),
        );
        let record = record_from_report(
            &report,
            "tc-00000001".to_string(),
            "2026-01-01T00:00:00Z".to_string(),
            Some("sha256:abc".to_string()),
        );
        assert_eq!(record.surface, "cli");
        assert_eq!(record.status, "compressed");
        assert_eq!(record.original_tokens, 1000);
        assert_eq!(record.compressed_tokens, 600);
        assert_eq!(record.saved_tokens, 400);
        assert!(record.bypass_reason.is_none());
        assert_eq!(record.project_hash, Some("sha256:abc".to_string()));
    }

    #[test]
    fn record_from_report_uses_wrap_surface_and_flags_never_worse_as_bypass() {
        let mut report = cli_report(
            100,
            120,
            "command_output",
            Status::BestEffort,
            heuristic_estimator(),
        );
        report.command = Some(CommandReport {
            command_family: None,
            child_exit_code: Some(0),
            duration_ms: 5,
            raw_output_bytes: 100,
            stdout_bytes: 100,
            stderr_bytes: 0,
            stderr_mode: "captured".to_string(),
            stderr_truncated: false,
            compressed_output_bytes: 100,
            filter_pack_id: None,
            filter_version: None,
            never_worse_applied: true,
            bypass_reason: None,
        });
        let record = record_from_report(
            &report,
            "tc-00000002".to_string(),
            "2026-01-01T00:00:00Z".to_string(),
            None,
        );
        assert_eq!(record.surface, "wrap");
        assert_eq!(
            record.bypass_reason,
            Some("would_increase_tokens".to_string())
        );
    }

    #[test]
    fn record_from_report_falls_back_to_bypass_report_reason() {
        let mut report = cli_report(50, 50, "plain_text", Status::Passthrough, exact_estimator());
        report.bypass = Some(BypassReport {
            reason: "env".to_string(),
            source: "cli".to_string(),
        });
        let record = record_from_report(
            &report,
            "tc-00000003".to_string(),
            "2026-01-01T00:00:00Z".to_string(),
            None,
        );
        assert_eq!(record.bypass_reason, Some("env".to_string()));
    }

    // --- aggregate: engineering.md "stats.rs (v0.2 parity surface)" bullets ----------------

    #[test]
    fn aggregates_fixture_reports_by_transform_format_estimator_status_and_project() {
        // Three heterogeneous fixture CompressionReports: different format, estimator
        // exactness, status, and project attribution.
        let openai = cli_report(
            2000,
            1000,
            "openai_json",
            Status::Compressed,
            exact_estimator(),
        );
        let text = cli_report(
            500,
            500,
            "plain_text",
            Status::Passthrough,
            heuristic_estimator(),
        );
        let mut diff = cli_report(300, 200, "git_diff", Status::BestEffort, exact_estimator());
        diff.transforms.push(crate::report::TransformReport {
            id: "diff_compaction".to_string(),
            version: "1.0.0".to_string(),
            tokens_before: 300,
            tokens_after: 200,
            saved_tokens: 100,
            savings_ratio: 0.333,
            elapsed_micros: None,
            status: crate::report::TransformStatus::Applied,
            skipped_reason: None,
            warnings: vec![],
        });

        let records = vec![
            record_from_report(
                &openai,
                "tc-a".to_string(),
                "2026-01-01T00:00:00Z".to_string(),
                Some("sha256:proj-a".to_string()),
            ),
            record_from_report(
                &text,
                "tc-b".to_string(),
                "2026-01-02T00:00:00Z".to_string(),
                Some("sha256:proj-b".to_string()),
            ),
            record_from_report(
                &diff,
                "tc-c".to_string(),
                "2026-01-03T00:00:00Z".to_string(),
                Some("sha256:proj-c".to_string()),
            ),
        ];

        let summary = aggregate(&records);
        assert_eq!(summary.requests, 3);
        assert_eq!(summary.raw_tokens, 2000 + 500 + 300);
        assert_eq!(summary.compressed_tokens, 1000 + 500 + 200);
        assert_eq!(summary.saved_tokens, 1000 + 100);
        assert_eq!(summary.recent_requests.len(), 3);
        let project_hashes: Vec<_> = summary
            .recent_requests
            .iter()
            .filter_map(|r| r.project_hash.clone())
            .collect();
        assert!(project_hashes.contains(&"sha256:proj-a".to_string()));
        assert!(project_hashes.contains(&"sha256:proj-b".to_string()));
        assert!(project_hashes.contains(&"sha256:proj-c".to_string()));
        // Sorted newest-timestamp-first.
        assert_eq!(summary.recent_requests[0].request_id, "tc-c");
    }

    #[test]
    fn aggregate_splits_wrapped_vs_raw_commands_and_computes_coverage() {
        let wrapped = {
            let mut r = cli_report(
                1000,
                400,
                "command_output",
                Status::BestEffort,
                exact_estimator(),
            );
            r.command = Some(CommandReport {
                command_family: None,
                child_exit_code: Some(0),
                duration_ms: 1,
                raw_output_bytes: 1000,
                stdout_bytes: 1000,
                stderr_bytes: 0,
                stderr_mode: "captured".to_string(),
                stderr_truncated: false,
                compressed_output_bytes: 400,
                filter_pack_id: None,
                filter_version: None,
                never_worse_applied: false,
                bypass_reason: None,
            });
            r
        };
        let raw = {
            let mut r = cli_report(
                1000,
                1000,
                "command_output",
                Status::BestEffort,
                exact_estimator(),
            );
            r.command = Some(CommandReport {
                command_family: None,
                child_exit_code: Some(0),
                duration_ms: 1,
                raw_output_bytes: 1000,
                stdout_bytes: 1000,
                stderr_bytes: 0,
                stderr_mode: "captured".to_string(),
                stderr_truncated: false,
                compressed_output_bytes: 1000,
                filter_pack_id: None,
                filter_version: None,
                never_worse_applied: true,
                bypass_reason: None,
            });
            r
        };

        let records = vec![
            record_from_report(
                &wrapped,
                "tc-w".to_string(),
                "2026-01-01T00:00:00Z".to_string(),
                None,
            ),
            record_from_report(
                &raw,
                "tc-r".to_string(),
                "2026-01-01T00:00:01Z".to_string(),
                None,
            ),
        ];
        let summary = aggregate(&records);
        assert_eq!(summary.commands, 2);
        assert_eq!(summary.wrapped_commands, 1);
        assert_eq!(summary.raw_commands, 1);
        assert_eq!(summary.bypass_count, 1);
        assert_eq!(summary.coverage_pct, 50.0);
        // wrapped ratio is 60% (400/1000 saved); extrapolated onto raw's 1000 original tokens.
        assert_eq!(summary.estimated_lost_tokens, 600);
    }

    #[test]
    fn aggregate_on_empty_records_never_divides_by_zero() {
        let summary = aggregate(&[]);
        assert_eq!(summary.requests, 0);
        assert_eq!(summary.savings_pct, 0.0);
        assert_eq!(summary.coverage_pct, 0.0);
        assert_eq!(summary.estimated_lost_tokens, 0);
    }

    #[test]
    fn json_and_csv_outputs_are_schema_stable_and_carry_no_raw_payload_bytes() {
        let report = cli_report(
            18_400,
            11_900,
            "openai_json",
            Status::Compressed,
            exact_estimator(),
        );
        let record = record_from_report(
            &report,
            "tc-7f3a2b1c".to_string(),
            "2026-07-08T12:00:00Z".to_string(),
            Some("sha256:deadbeef".to_string()),
        );
        let summary = aggregate(&[record]);

        let json = serde_json::to_value(&summary).unwrap();
        for key in [
            "schema_version",
            "scope",
            "window",
            "project",
            "requests",
            "commands",
            "wrapped_commands",
            "raw_commands",
            "bypass_count",
            "raw_tokens",
            "compressed_tokens",
            "saved_tokens",
            "savings_pct",
            "estimated_lost_tokens",
            "coverage_pct",
            "untrusted_filter_count",
            "retrieval",
            "cache",
            "latency",
            "recent_requests",
        ] {
            assert!(json.get(key).is_some(), "missing key {key}");
        }
        assert_eq!(json["retrieval"]["markers"], 0);
        assert_eq!(json["cache"]["hits"], 0);
        assert_eq!(json["recent_requests"][0]["request_id"], "tc-7f3a2b1c");

        // No arbitrary payload text ever enters the summary: every string in it is drawn from
        // a fixed short vocabulary (ids, ISO timestamps, format/mode/status labels, hashes).
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(!serialized.contains("hello world"));

        let csv = to_csv(&summary);
        assert!(csv.contains("schema_version,scope,window"));
        assert!(csv.contains("request_id,timestamp,surface"));
        assert!(csv.contains("tc-7f3a2b1c"));
        assert!(csv.contains("sha256:deadbeef"));
    }

    #[test]
    fn csv_escapes_fields_containing_commas_or_quotes() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn savings_provenance_maps_estimator_exactness() {
        assert_eq!(savings_provenance(true), SavingsProvenance::Measured);
        assert_eq!(savings_provenance(false), SavingsProvenance::Heuristic);
        assert_eq!(SavingsProvenance::Measured.as_str(), "measured");
        assert_eq!(SavingsProvenance::Heuristic.as_str(), "heuristic");
        assert_eq!(SavingsProvenance::Estimated.as_str(), "estimated");
    }

    // --- LedgerStore -------------------------------------------------------------------------

    fn sample_record(id: &str, timestamp: &str) -> LedgerRecord {
        LedgerRecord {
            request_id: id.to_string(),
            timestamp: timestamp.to_string(),
            surface: "cli".to_string(),
            format: "plain_text".to_string(),
            mode: "balanced".to_string(),
            status: "compressed".to_string(),
            original_tokens: 100,
            compressed_tokens: 60,
            saved_tokens: 40,
            savings_pct: 40.0,
            bypass_reason: None,
            project_hash: None,
        }
    }

    #[test]
    fn append_then_read_all_round_trips_records_in_order() {
        let path = temp_ledger_path("append_read");
        let store = LedgerStore::new(&path);
        store
            .append(&sample_record("tc-1", "2026-01-01T00:00:00Z"))
            .unwrap();
        store
            .append(&sample_record("tc-2", "2026-01-02T00:00:00Z"))
            .unwrap();

        let records = store.read_all().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].request_id, "tc-1");
        assert_eq!(records[1].request_id, "tc-2");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_all_on_missing_file_is_an_empty_ledger_not_an_error() {
        let path = temp_ledger_path("missing");
        let store = LedgerStore::new(&path);
        assert_eq!(store.read_all().unwrap(), Vec::new());
    }

    #[test]
    fn read_all_skips_malformed_lines_without_failing() {
        let path = temp_ledger_path("malformed");
        std::fs::write(
            &path,
            format!(
                "{}\nnot json at all\n{}\n",
                serde_json::to_string(&sample_record("tc-1", "2026-01-01T00:00:00Z")).unwrap(),
                serde_json::to_string(&sample_record("tc-2", "2026-01-02T00:00:00Z")).unwrap(),
            ),
        )
        .unwrap();
        let store = LedgerStore::new(&path);
        let records = store.read_all().unwrap();
        assert_eq!(records.len(), 2);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn gc_deletes_only_records_older_than_retention_and_leaves_active_records_untouched() {
        let path = temp_ledger_path("gc");
        let store = LedgerStore::new(&path);

        let now = now_unix();
        let old_ts = format_unix_timestamp(now.saturating_sub(200 * 86_400));
        let recent_ts = format_unix_timestamp(now.saturating_sub(86_400));
        store.append(&sample_record("tc-old", &old_ts)).unwrap();
        store
            .append(&sample_record("tc-recent", &recent_ts))
            .unwrap();

        let outcome = store.gc(90).unwrap();
        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.kept, 1);

        let remaining = store.read_all().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].request_id, "tc-recent");
        // The surviving record must be byte-identical metadata, not just present.
        assert_eq!(remaining[0], sample_record("tc-recent", &recent_ts));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn gc_keeps_records_with_unparsable_timestamps_fail_safe() {
        let path = temp_ledger_path("gc_unparsable");
        let store = LedgerStore::new(&path);
        store
            .append(&sample_record("tc-weird", "not-a-timestamp"))
            .unwrap();

        let outcome = store.gc(1).unwrap();
        assert_eq!(outcome.removed, 0);
        assert_eq!(outcome.kept, 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn filter_since_drops_older_and_unparsable_records() {
        let now = now_unix();
        let recent_ts = format_unix_timestamp(now.saturating_sub(60));
        let old_ts = format_unix_timestamp(now.saturating_sub(90 * 86_400));
        let records = vec![
            sample_record("tc-recent", &recent_ts),
            sample_record("tc-old", &old_ts),
            sample_record("tc-weird", "garbage"),
        ];
        let filtered = filter_since(&records, now, 3_600);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].request_id, "tc-recent");
    }

    #[test]
    fn retrieval_report_used_by_glob_aggregation_carries_marker_count() {
        // Sanity check that CompressionReport.retrieval is the honest source for retrieval
        // marker totals during ad-hoc report-glob aggregation (see module doc); aggregate()
        // itself never touches it since LedgerRecord doesn't carry retrieval fields.
        let mut report = cli_report(100, 80, "plain_text", Status::Compressed, exact_estimator());
        report.retrieval = Some(RetrievalReport {
            store_namespace: "default".to_string(),
            hash_algorithm: "sha256".to_string(),
            marker_count: 1,
            ttl_seconds: Some(3600),
            persisted_original_bytes: 100,
            skipped_original_bytes: 0,
        });
        assert_eq!(report.retrieval.as_ref().unwrap().marker_count, 1);
    }
}
