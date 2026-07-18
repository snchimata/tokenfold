//! F-054 / F-055: external RTK composition, preflight, and secure raw capture.
//!
//! RTK is an *external, optional* executable. tokenfold never bundles it, never edits the
//! user's RTK configuration, and always falls open to the tokenfold-only path when RTK is
//! missing or incompatible. See INTERFACES.md §1.9 and ENGINEERING.md "v0.3 RTK Composition".
//!
//! The invocation contract is `rtk <child argv...>`: RTK runs the child, performs its own
//! command-specific filtering, and writes the filtered result to stdout. tokenfold then treats
//! that as the input to its generic compression pipeline. Once RTK has been spawned, its output
//! and exit status are authoritative — tokenfold never reruns the child (it may have side
//! effects).

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokenfold_core::TokenFoldError;

/// Env var overriding the RTK executable path (skips PATH resolution). Also the seam tests use
/// to point at a fake `rtk`.
const RTK_BIN_ENV: &str = "TOKENFOLD_RTK_BIN";
/// Env var that disables RTK integration entirely (doctor reports `status="disabled"`).
const RTK_DISABLED_ENV: &str = "TOKENFOLD_RTK_DISABLED";
/// Env var tokenfold sets to hand RTK a fresh per-run capture directory for CCR.
const RTK_TEE_DIR_ENV: &str = "RTK_TEE_DIR";
/// Marker file an RTK integrator may drop in the tee dir to signal a truncated capture.
const TRUNCATED_MARKER: &str = "truncated";

/// Doctor-facing RTK health, serialized additively under `doctor --json`'s `rtk` object.
pub struct RtkDoctor {
    pub status: String, // "disabled" | "available" | "missing" | "incompatible"
    pub path: Option<String>,
    pub version: Option<String>,
    pub compatible: bool,
    pub raw_capture: String, // "complete" | "partial" | "unavailable" | "not_applicable"
    pub notes: Vec<String>,
}

/// Result of resolving + version-checking RTK before the child runs.
pub enum Preflight {
    /// RTK is present and compatible; safe to compose.
    Ready { path: PathBuf, version: String },
    /// Fall open to the tokenfold-only path. `status` is `"unavailable"` or `"incompatible"`.
    FailOpen { status: String, note: String },
}

/// Completeness of a CCR raw capture.
pub struct RawCapture {
    pub bytes: Vec<u8>,
    pub completeness: String, // "complete" | "partial" | "unavailable"
}

/// Outcome of running RTK as the filtering stage.
pub struct RtkRun {
    /// Filtered output (RTK stdout, with stderr appended — same convention as bare `wrap`).
    pub output: Vec<u8>,
    pub exit_code: Option<i32>,
    pub duration_ms: f64,
    pub version: String,
    /// Present only when CCR raw capture was requested.
    pub raw_capture: Option<RawCapture>,
    /// `"applied"` when RTK produced output, `"failed"` when it could not be launched at all.
    pub stage_status: String,
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Extracts the first `MAJOR.MINOR[.PATCH]` token from `--version` output (tolerates a leading
/// `v` and vendor prefixes like `rtk 0.4.1`).
fn parse_version(text: &str) -> Option<String> {
    for raw in text.split(|c: char| c.is_whitespace()) {
        let tok = raw.trim().trim_start_matches('v');
        let mut parts = tok.split('.');
        let (Some(a), Some(b)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !a.is_empty()
            && a.bytes().all(|c| c.is_ascii_digit())
            && !b.is_empty()
            && b.bytes().all(|c| c.is_ascii_digit())
        {
            // Keep major.minor.patch if present; otherwise major.minor.
            let patch = parts
                .next()
                .filter(|p| p.bytes().all(|c| c.is_ascii_digit()));
            return Some(match patch {
                Some(p) if !p.is_empty() => format!("{a}.{b}.{p}"),
                _ => format!("{a}.{b}"),
            });
        }
    }
    None
}

/// ponytail: any parseable version is accepted in v0.3 — RTK's stable wire contract isn't
/// pinned yet, so an unparseable/absent version is the only "incompatible" signal. Raise this
/// floor once RTK ships a compatibility guarantee.
fn is_compatible(_version: &str) -> bool {
    true
}

/// Locates the RTK executable: `TOKENFOLD_RTK_BIN` override first, then a PATH scan for
/// `rtk` (plus `PATHEXT` variants on Windows).
fn resolve_rtk() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(RTK_BIN_ENV) {
        let p = PathBuf::from(explicit);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string())
            .split(';')
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("rtk{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Runs `<rtk> --version` and returns the parsed version, or `None` if it can't be run or
/// produces no parseable version.
fn query_version(path: &Path) -> Option<String> {
    let out = std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()?;
    // Some tools print the version to stderr; check both, stdout first.
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_version(&stdout).or_else(|| parse_version(&String::from_utf8_lossy(&out.stderr)))
}

/// Preflight resolution + version/compat check. Runs **before** the child so a fail-open never
/// leaves a side-effectful command half-run.
pub fn preflight() -> Preflight {
    if is_disabled() {
        return Preflight::FailOpen {
            status: "unavailable".to_string(),
            note: format!("RTK disabled via {RTK_DISABLED_ENV}"),
        };
    }
    let Some(path) = resolve_rtk() else {
        return Preflight::FailOpen {
            status: "unavailable".to_string(),
            note: "rtk executable not found on PATH".to_string(),
        };
    };
    match query_version(&path) {
        Some(version) if is_compatible(&version) => Preflight::Ready { path, version },
        Some(version) => Preflight::FailOpen {
            status: "incompatible".to_string(),
            note: format!("rtk {version} is not a supported version"),
        },
        None => Preflight::FailOpen {
            status: "incompatible".to_string(),
            note: "could not determine rtk version".to_string(),
        },
    }
}

fn is_disabled() -> bool {
    std::env::var_os(RTK_DISABLED_ENV).is_some_and(|v| v != "0" && !v.is_empty())
}

/// Creates a fresh, per-run, owner-only directory for RTK's raw tee. Deleted after ingestion by
/// [`CaptureDir`]'s `Drop`.
fn make_capture_dir() -> Result<PathBuf, TokenFoldError> {
    let dir =
        std::env::temp_dir().join(format!("tokenfold-rtk-{}-{}", std::process::id(), nanos()));
    std::fs::create_dir(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    // ponytail: Windows relies on the per-user temp dir's default ACL rather than an explicit
    // 0700; tighten with a SetNamedSecurityInfo call only if a shared-temp threat model demands.
    Ok(dir)
}

/// RAII guard that removes the transient capture directory no matter how the run exits — the
/// transient tee must never outlive the run (INTERFACES.md §1.9).
struct CaptureDir(PathBuf);

impl Drop for CaptureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Reads the raw capture RTK wrote to the tee dir. Completeness is conservative: a non-empty
/// capture is `"complete"` unless RTK left a `truncated` marker; an empty/absent capture is
/// `"unavailable"`. ponytail: filesystem tee can't self-report mid-file truncation, so
/// completeness leans on the marker convention; a real RTK fd/side-channel supersedes this.
fn read_capture(dir: &Path, filtered_len: usize) -> RawCapture {
    let truncated_marker = dir.join(TRUNCATED_MARKER).exists();
    let mut bytes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        // Deterministic concatenation order; skip the marker file itself.
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.file_name().is_some_and(|n| n != TRUNCATED_MARKER))
            .collect();
        files.sort();
        for f in files {
            if let Ok(mut content) = std::fs::read(&f) {
                bytes.append(&mut content);
            }
        }
    }
    let completeness = if bytes.is_empty() {
        "unavailable"
    } else if truncated_marker || bytes.len() < filtered_len {
        // Filtering reduces size, so a raw capture smaller than the filtered output is a strong
        // truncation signal.
        "partial"
    } else {
        "complete"
    };
    RawCapture {
        bytes,
        completeness: completeness.to_string(),
    }
}

/// Runs RTK over the child argv. `capture_raw` opts into the CCR tee handshake.
///
/// The child argv is passed to RTK verbatim as separate process arguments (no shell), so
/// argument boundaries and special characters are preserved exactly.
pub fn run(
    path: &Path,
    version: &str,
    child_argv: &[String],
    capture_raw: bool,
) -> Result<RtkRun, TokenFoldError> {
    let mut cmd = std::process::Command::new(path);
    cmd.args(child_argv);

    // Own the capture dir for the whole run; it is deleted when `_capture_dir` drops.
    let capture_dir = if capture_raw {
        let dir = make_capture_dir()?;
        cmd.env(RTK_TEE_DIR_ENV, &dir);
        Some(dir)
    } else {
        None
    };
    let _capture_dir = capture_dir.as_ref().map(|d| CaptureDir(d.clone()));

    let start = Instant::now();
    let output = cmd.output().map_err(|e| {
        TokenFoldError::InvalidInput(format!("failed to launch rtk `{}`: {e}", path.display()))
    })?;
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Same stdout-then-stderr concatenation convention as bare `wrap`.
    let mut filtered = output.stdout;
    filtered.extend_from_slice(&output.stderr);

    let raw_capture = capture_dir
        .as_ref()
        .map(|dir| read_capture(dir, filtered.len()));

    Ok(RtkRun {
        output: filtered,
        exit_code: output.status.code(),
        duration_ms,
        version: version.to_string(),
        raw_capture,
        stage_status: "applied".to_string(),
    })
}

/// Best-effort doctor probe. Never edits RTK configuration; when RTK is present it notes that a
/// complete CCR capture requires the user to enable RTK's `[tee]` section.
pub fn doctor_probe() -> RtkDoctor {
    if is_disabled() {
        return RtkDoctor {
            status: "disabled".to_string(),
            path: None,
            version: None,
            compatible: false,
            raw_capture: "not_applicable".to_string(),
            notes: vec![format!("RTK integration disabled via {RTK_DISABLED_ENV}")],
        };
    }
    let Some(path) = resolve_rtk() else {
        return RtkDoctor {
            status: "missing".to_string(),
            path: None,
            version: None,
            compatible: false,
            raw_capture: "not_applicable".to_string(),
            notes: vec![
                "rtk not found on PATH; wrap --rtk falls open to the tokenfold-only path"
                    .to_string(),
            ],
        };
    };
    let path_str = path.display().to_string();
    match query_version(&path) {
        Some(version) if is_compatible(&version) => RtkDoctor {
            status: "available".to_string(),
            path: Some(path_str),
            version: Some(version),
            compatible: true,
            // Tee is off by default; we can't read RTK's config, so raw capture is not ready
            // until the user opts in. Report honestly rather than claim readiness.
            raw_capture: "unavailable".to_string(),
            notes: vec![
                "enable RTK [tee] (enabled=true, mode=\"always\") to allow CCR raw capture; \
                 tokenfold supplies a per-run RTK_TEE_DIR"
                    .to_string(),
            ],
        },
        Some(version) => RtkDoctor {
            status: "incompatible".to_string(),
            path: Some(path_str),
            version: Some(version),
            compatible: false,
            raw_capture: "not_applicable".to_string(),
            notes: vec!["rtk version is not supported".to_string()],
        },
        None => RtkDoctor {
            status: "incompatible".to_string(),
            path: Some(path_str),
            version: None,
            compatible: false,
            raw_capture: "not_applicable".to_string(),
            notes: vec!["`rtk --version` produced no parseable version".to_string()],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_version_shapes() {
        assert_eq!(parse_version("rtk 0.4.1").as_deref(), Some("0.4.1"));
        assert_eq!(parse_version("v1.2").as_deref(), Some("1.2"));
        assert_eq!(
            parse_version("rtk version 2.10.0\n").as_deref(),
            Some("2.10.0")
        );
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn capture_completeness_classification() {
        let dir = make_capture_dir().unwrap();
        let _g = CaptureDir(dir.clone());

        // Absent capture -> unavailable.
        assert_eq!(read_capture(&dir, 10).completeness, "unavailable");

        // Non-empty, at least as large as filtered -> complete.
        std::fs::write(dir.join("raw.log"), b"the full raw output").unwrap();
        assert_eq!(read_capture(&dir, 5).completeness, "complete");

        // Smaller than filtered -> partial (truncation signal).
        assert_eq!(read_capture(&dir, 9999).completeness, "partial");

        // Explicit truncation marker -> partial.
        std::fs::write(dir.join(TRUNCATED_MARKER), b"").unwrap();
        assert_eq!(read_capture(&dir, 1).completeness, "partial");
    }

    #[test]
    fn capture_dir_is_removed_on_drop() {
        let dir = make_capture_dir().unwrap();
        assert!(dir.exists());
        {
            let _g = CaptureDir(dir.clone());
        }
        assert!(!dir.exists());
    }
}
