//! F-047: declarative command-output filter registry (`roadmap.md` F-047, `interfaces.md`
//! §7.2 "Filter Pack Contract" and §7.3 "Filter Trust Contract").
//!
//! Filter packs are TOML documents describing an ordered list of pure text-transform "stages"
//! (`strip_ansi`, `replace`, `keep_lines`, `strip_lines`, `head`, `tail`, `max_lines`,
//! `truncate`, `on_empty`) that run against a wrapped command's captured output before the
//! generic `compress()` pipeline sees it. There is deliberately no stage type that runs a
//! shell command or reads an arbitrary file — [`Stage`] is a closed, exhaustively-matched enum
//! of text transforms only (see the `stage_schema_has_no_shell_or_file_read_variant` test),
//! which is what makes "filters cannot execute shell commands or read arbitrary files" true by
//! construction rather than by policy.
//!
//! Trust: built-in filters (this module's [`built_in_packs`]) are always trusted. Project
//! (`.tokenfold/filters.toml`) and user (`$XDG_CONFIG_HOME/tokenfold/filters.toml`) filters are
//! skipped — not applied, not a hard error — until their exact file content is recorded in the
//! trust store (`$XDG_DATA_HOME/tokenfold/trusted_filters.json`, see [`TrustStore`]), unless the
//! CI escape hatch `TOKENFOLD_TRUST_PROJECT_FILTERS=1` is set (project tier only — see
//! `resolve_matching_filter`'s doc comment).
//!
//! Regex safety: this workspace's `regex` crate is provably linear-time regardless of pattern
//! shape (no backtracking engine is linked in anywhere — see `deny.toml`'s ban on
//! `fancy-regex`/`pcre2`, and `transforms::redaction`'s own module doc), so a nested-quantifier
//! *pattern* cannot actually cause catastrophic backtracking here. [`check_pattern_safety`] is
//! still applied to every user-supplied pattern as defense-in-depth: it rejects overlong
//! patterns and obviously hostile nested-quantifier shapes so this module's safety contract
//! doesn't silently depend on which regex engine happens to be linked in.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::errors::TokenFoldError;
use crate::retrieval_store::hex_sha256;
use crate::token_estimator::{ByteHeuristicEstimator, TokenEstimator};

/// The only `schema_version` this pass understands; anything else fails closed in
/// [`FilterPack::validate`].
pub const SCHEMA_VERSION: &str = "1.0";

/// Defense-in-depth length cap for any user-supplied regex pattern (see module doc).
const MAX_PATTERN_LEN: usize = 200;

// ---------------------------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackMeta {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub name: String,
    pub input: String,
    pub output: String,
    pub expected_token_delta: i64,
}

/// One declarative stage in a filter's pipeline. Internally tagged on `type` in TOML (e.g.
/// `{ type = "head", count = 20 }`). Deliberately a closed set: there is no `exec`/`shell`/
/// `read_file` variant, and adding one would require editing every exhaustive `match` over this
/// enum in this module (see the `stage_schema_has_no_shell_or_file_read_variant` test).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Stage {
    StripAnsi,
    Replace {
        pattern: String,
        replacement: String,
    },
    KeepLines {
        pattern: String,
    },
    StripLines {
        pattern: String,
    },
    Head {
        count: usize,
    },
    Tail {
        count: usize,
    },
    MaxLines {
        limit: usize,
    },
    Truncate {
        max_bytes: usize,
    },
    OnEmpty {
        value: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub id: String,
    pub version: String,
    /// Literal argv prefix this filter fires on (e.g. `["git", "diff"]` matches `git diff`,
    /// `git diff --stat`, `git diff HEAD~1`, ...). "Exact" per the contract means literal
    /// string comparison, not a shell glob or regex — see [`Filter::matches_command`].
    pub match_command: Vec<String>,
    /// Optional additional gate: a regex that must match somewhere in the captured output for
    /// this filter to apply. Subject to the same [`check_pattern_safety`] guard as any other
    /// user-supplied pattern.
    #[serde(default)]
    pub match_output: Option<String>,
    pub stages: Vec<Stage>,
    #[serde(default)]
    pub fixtures: Vec<Fixture>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterPack {
    pub schema_version: String,
    pub pack: PackMeta,
    #[serde(default)]
    pub filters: Vec<Filter>,
}

impl FilterPack {
    pub fn parse(text: &str) -> Result<Self, TokenFoldError> {
        toml::from_str(text)
            .map_err(|e| TokenFoldError::ConfigError(format!("invalid filter pack TOML: {e}")))
    }

    /// Schema-level validation: unknown fields are already rejected by `serde(deny_unknown_fields)`
    /// at parse time; this additionally checks `schema_version` and runs [`check_pattern_safety`]
    /// (plus a real regex compile) over every user-supplied pattern in every stage.
    pub fn validate(&self) -> Result<(), TokenFoldError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(TokenFoldError::ConfigError(format!(
                "unsupported filter schema_version {:?}; expected {SCHEMA_VERSION:?}",
                self.schema_version
            )));
        }
        for filter in &self.filters {
            if let Some(pattern) = &filter.match_output {
                validate_pattern(&filter.id, pattern)?;
            }
            for stage in &filter.stages {
                if let Some(pattern) = stage_pattern(stage) {
                    validate_pattern(&filter.id, pattern)?;
                }
            }
        }
        Ok(())
    }

    /// Runs every filter's inline fixtures and reports whether each one's actual output and
    /// token delta match what the fixture declares.
    pub fn run_fixtures(&self) -> Result<Vec<FixtureCheck>, TokenFoldError> {
        let estimator = ByteHeuristicEstimator;
        let mut results = Vec::new();
        for filter in &self.filters {
            for fixture in &filter.fixtures {
                let actual_bytes = filter.apply(fixture.input.as_bytes())?;
                let actual_output = String::from_utf8_lossy(&actual_bytes).into_owned();
                let in_tokens = estimator.count_bytes(fixture.input.as_bytes()) as i64;
                let out_tokens = estimator.count_bytes(&actual_bytes) as i64;
                results.push(FixtureCheck {
                    filter_id: filter.id.clone(),
                    fixture_name: fixture.name.clone(),
                    output_matches: actual_output == fixture.output,
                    actual_token_delta: in_tokens - out_tokens,
                    expected_token_delta: fixture.expected_token_delta,
                });
            }
        }
        Ok(results)
    }
}

fn validate_pattern(filter_id: &str, pattern: &str) -> Result<(), TokenFoldError> {
    check_pattern_safety(pattern)?;
    Regex::new(pattern).map_err(|e| {
        TokenFoldError::ConfigError(format!(
            "filter {filter_id:?} has an invalid regex pattern {pattern:?}: {e}"
        ))
    })?;
    Ok(())
}

fn stage_pattern(stage: &Stage) -> Option<&str> {
    match stage {
        Stage::Replace { pattern, .. }
        | Stage::KeepLines { pattern }
        | Stage::StripLines { pattern } => Some(pattern.as_str()),
        Stage::StripAnsi
        | Stage::Head { .. }
        | Stage::Tail { .. }
        | Stage::MaxLines { .. }
        | Stage::Truncate { .. }
        | Stage::OnEmpty { .. } => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FixtureCheck {
    pub filter_id: String,
    pub fixture_name: String,
    pub output_matches: bool,
    pub actual_token_delta: i64,
    pub expected_token_delta: i64,
}

impl FixtureCheck {
    pub fn passed(&self) -> bool {
        self.output_matches && self.actual_token_delta == self.expected_token_delta
    }
}

// ---------------------------------------------------------------------------------------------
// Regex complexity guard (R-017 defense-in-depth; see module doc)
// ---------------------------------------------------------------------------------------------

fn check_pattern_safety(pattern: &str) -> Result<(), TokenFoldError> {
    if pattern.len() > MAX_PATTERN_LEN {
        return Err(TokenFoldError::InvalidInput(format!(
            "filter regex pattern exceeds the {MAX_PATTERN_LEN}-byte safety limit: {pattern:?}"
        )));
    }
    if has_nested_quantifier(pattern) {
        return Err(TokenFoldError::InvalidInput(format!(
            "filter regex pattern {pattern:?} looks like a nested quantifier (ReDoS-shaped) and is rejected"
        )));
    }
    Ok(())
}

/// Flags the classic `(x+)+` / `(x*)*` / `(x+)*` / `(x{n,m})+` nested-quantifier shape: a group
/// that itself ends in a quantifier, immediately followed by another quantifier. See the module
/// doc for why this is defense-in-depth rather than a correctness requirement on this crate's
/// `regex` engine.
fn has_nested_quantifier(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] != b')' {
            continue;
        }
        let inner_quantified = matches!(bytes[i - 1], b'+' | b'*' | b'}');
        let outer_quantified = matches!(bytes.get(i + 1), Some(b'+') | Some(b'*') | Some(b'{'));
        if inner_quantified && outer_quantified {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Stage execution engine
// ---------------------------------------------------------------------------------------------

impl Filter {
    /// Literal argv-prefix match: `argv` must start with exactly `self.match_command`'s
    /// elements, in order. Not a glob or regex — see the struct doc.
    pub fn matches_command(&self, argv: &[String]) -> bool {
        !self.match_command.is_empty()
            && argv.len() >= self.match_command.len()
            && argv[..self.match_command.len()] == self.match_command[..]
    }

    /// `matches_command` plus the optional `match_output` gate against the command's raw
    /// captured output.
    pub fn matches(&self, argv: &[String], raw_output: &[u8]) -> bool {
        if !self.matches_command(argv) {
            return false;
        }
        match &self.match_output {
            None => true,
            Some(pattern) => Regex::new(pattern)
                .map(|re| re.is_match(&String::from_utf8_lossy(raw_output)))
                .unwrap_or(false),
        }
    }

    /// Applies every stage in declared order to `input` (treated as best-effort UTF-8, mirroring
    /// `transforms::redaction`'s `from_utf8_lossy` convention).
    pub fn apply(&self, input: &[u8]) -> Result<Vec<u8>, TokenFoldError> {
        let mut text = String::from_utf8_lossy(input).into_owned();
        for stage in &self.stages {
            text = apply_stage(stage, &text)?;
        }
        Ok(text.into_bytes())
    }
}

fn strip_ansi_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").expect("strip_ansi regex is a fixed valid literal")
    })
}

fn apply_stage(stage: &Stage, text: &str) -> Result<String, TokenFoldError> {
    match stage {
        Stage::StripAnsi => Ok(strip_ansi_regex().replace_all(text, "").into_owned()),
        Stage::Replace {
            pattern,
            replacement,
        } => {
            check_pattern_safety(pattern)?;
            let re = compile_pattern(pattern)?;
            Ok(re.replace_all(text, replacement.as_str()).into_owned())
        }
        Stage::KeepLines { pattern } => {
            check_pattern_safety(pattern)?;
            let re = compile_pattern(pattern)?;
            Ok(text
                .lines()
                .filter(|line| re.is_match(line))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Stage::StripLines { pattern } => {
            check_pattern_safety(pattern)?;
            let re = compile_pattern(pattern)?;
            Ok(text
                .lines()
                .filter(|line| !re.is_match(line))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Stage::Head { count } => Ok(text.lines().take(*count).collect::<Vec<_>>().join("\n")),
        Stage::Tail { count } => {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(*count);
            Ok(lines[start..].join("\n"))
        }
        Stage::MaxLines { limit } => {
            let lines: Vec<&str> = text.lines().collect();
            if lines.len() <= *limit {
                Ok(text.to_string())
            } else {
                let removed = lines.len() - limit;
                let mut kept = lines[..*limit].join("\n");
                kept.push_str(&format!(
                    "\n... [tokenfold: truncated {removed} more lines]"
                ));
                Ok(kept)
            }
        }
        Stage::Truncate { max_bytes } => {
            if text.len() <= *max_bytes {
                Ok(text.to_string())
            } else {
                let mut end = *max_bytes;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                let mut truncated = text[..end].to_string();
                truncated.push_str("... [tokenfold: truncated]");
                Ok(truncated)
            }
        }
        Stage::OnEmpty { value } => {
            if text.is_empty() {
                Ok(value.clone())
            } else {
                Ok(text.to_string())
            }
        }
    }
}

fn compile_pattern(pattern: &str) -> Result<Regex, TokenFoldError> {
    Regex::new(pattern).map_err(|e| {
        TokenFoldError::InvalidInput(format!("invalid regex pattern {pattern:?}: {e}"))
    })
}

// ---------------------------------------------------------------------------------------------
// never_worse guard
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeverWorseOutcome {
    pub bytes: Vec<u8>,
    pub used_filtered: bool,
}

/// Compares `raw` and `filtered` by the same cheap heuristic byte-count estimator `compress()`
/// itself falls back to (`ByteHeuristicEstimator`); returns `filtered` only if it is strictly
/// smaller, otherwise falls back to `raw` untouched.
pub fn never_worse(raw: &[u8], filtered: &[u8]) -> NeverWorseOutcome {
    let estimator = ByteHeuristicEstimator;
    let raw_tokens = estimator.count_bytes(raw);
    let filtered_tokens = estimator.count_bytes(filtered);
    if filtered_tokens < raw_tokens {
        NeverWorseOutcome {
            bytes: filtered.to_vec(),
            used_filtered: true,
        }
    } else {
        NeverWorseOutcome {
            bytes: raw.to_vec(),
            used_filtered: false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Trust store (interfaces.md §7.3)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrustedFilterEntry {
    pub path: String,
    pub sha256: String,
    pub schema_version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pub entries: Vec<TrustedFilterEntry>,
}

impl TrustStore {
    /// `$XDG_DATA_HOME/tokenfold/trusted_filters.json`, falling back to
    /// `<home>/.local/share/tokenfold/trusted_filters.json` — mirrors
    /// `retrieval_store::default_store_path`'s HOME/USERPROFILE fallback.
    pub fn default_path() -> PathBuf {
        if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(dir)
                .join("tokenfold")
                .join("trusted_filters.json");
        }
        let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".local")
            .join("share")
            .join("tokenfold")
            .join("trusted_filters.json")
    }

    /// A missing or unparsable trust store fails closed to "nothing is trusted" rather than a
    /// hard error: a corrupt/missing trust file must never crash an otherwise-successful `wrap`
    /// run, it just means project/user-tier filters get skipped this run (same as if they had
    /// never been trusted).
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return TrustStore::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), TokenFoldError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| {
            TokenFoldError::InternalError(format!("failed to encode trust store: {e}"))
        })?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Content changes invalidate trust: this recomputes the SHA-256 of `content` and compares
    /// against the recorded hash rather than trusting a path alone.
    pub fn is_trusted(&self, canonical_path: &Path, content: &[u8], schema_version: &str) -> bool {
        let path_str = canonical_path.to_string_lossy();
        let hash = hex_sha256(content);
        self.entries
            .iter()
            .any(|e| e.path == path_str && e.sha256 == hash && e.schema_version == schema_version)
    }

    pub fn trust(&mut self, canonical_path: &Path, content: &[u8], schema_version: &str) {
        let path_str = canonical_path.to_string_lossy().into_owned();
        let hash = hex_sha256(content);
        self.entries.retain(|e| e.path != path_str);
        self.entries.push(TrustedFilterEntry {
            path: path_str,
            sha256: hash,
            schema_version: schema_version.to_string(),
        });
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `$XDG_CONFIG_HOME/tokenfold/filters.toml`, falling back to
/// `<home>/.config/tokenfold/filters.toml`.
pub fn default_user_filters_path() -> PathBuf {
    let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")));
    match xdg_config_home {
        Some(dir) => dir.join("tokenfold").join("filters.toml"),
        None => PathBuf::from(".config")
            .join("tokenfold")
            .join("filters.toml"),
    }
}

// ---------------------------------------------------------------------------------------------
// Precedence resolution: project -> user -> built-in
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterTier {
    Project,
    User,
    BuiltIn,
}

impl FilterTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            FilterTier::Project => "project",
            FilterTier::User => "user",
            FilterTier::BuiltIn => "built_in",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MatchedFilter {
    pub tier: FilterTier,
    pub pack_id: String,
    pub pack_version: String,
    pub filter: Filter,
}

/// Inputs to [`resolve_matching_filter`], grouped to avoid an unwieldy positional argument list.
pub struct FilterLookup<'a> {
    pub argv: &'a [String],
    pub raw_output: &'a [u8],
    pub enabled: bool,
    pub project_filters_path: Option<&'a Path>,
    pub user_filters_path: Option<&'a Path>,
    pub trust_store_path: &'a Path,
    /// `TOKENFOLD_TRUST_PROJECT_FILTERS=1` (interfaces.md §7.3): bypasses the trust-store check
    /// for the *project* tier only (its literal name is the documented CI override, not a
    /// generic "trust everything" switch) — user-tier filters always require an explicit
    /// `filters trust` regardless of this flag.
    pub trust_project_filters: bool,
}

/// Reads a filter pack file from disk and returns it only if it parses, passes
/// [`FilterPack::validate`], and (unless `bypass_trust`) is recorded in `trust_store` under its
/// current content hash. Returns `None` — not an error — for a missing file, a parse/validation
/// failure, or an untrusted pack: callers treat all three the same way, "this tier has nothing
/// usable right now," per the fail-closed-but-not-fatal contract.
fn load_trusted_pack(
    path: &Path,
    trust_store: &TrustStore,
    bypass_trust: bool,
) -> Option<FilterPack> {
    let bytes = std::fs::read(path).ok()?;
    let pack = FilterPack::parse(&String::from_utf8_lossy(&bytes)).ok()?;
    pack.validate().ok()?;
    if bypass_trust {
        return Some(pack);
    }
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if trust_store.is_trusted(&canonical, &bytes, &pack.schema_version) {
        Some(pack)
    } else {
        None
    }
}

/// Deterministic precedence resolution: project -> user -> built-in. Within each tier, the
/// first filter (in file/declaration order) whose `match_command` (plus optional
/// `match_output`) matches wins. Built-in filters are always trusted; project/user filters are
/// silently skipped (not an error) when their file is missing, fails to parse/validate, or isn't
/// recorded in the trust store.
pub fn resolve_matching_filter(lookup: &FilterLookup) -> Option<MatchedFilter> {
    if !lookup.enabled {
        return None;
    }
    let trust_store = TrustStore::load(lookup.trust_store_path);

    if let Some(path) = lookup.project_filters_path
        && let Some(pack) = load_trusted_pack(path, &trust_store, lookup.trust_project_filters)
        && let Some(filter) = pack
            .filters
            .iter()
            .find(|f| f.matches(lookup.argv, lookup.raw_output))
    {
        return Some(MatchedFilter {
            tier: FilterTier::Project,
            pack_id: pack.pack.id.clone(),
            pack_version: pack.pack.version.clone(),
            filter: filter.clone(),
        });
    }

    if let Some(path) = lookup.user_filters_path
        && let Some(pack) = load_trusted_pack(path, &trust_store, false)
        && let Some(filter) = pack
            .filters
            .iter()
            .find(|f| f.matches(lookup.argv, lookup.raw_output))
    {
        return Some(MatchedFilter {
            tier: FilterTier::User,
            pack_id: pack.pack.id.clone(),
            pack_version: pack.pack.version.clone(),
            filter: filter.clone(),
        });
    }

    for pack in built_in_packs() {
        if let Some(filter) = pack
            .filters
            .iter()
            .find(|f| f.matches(lookup.argv, lookup.raw_output))
        {
            return Some(MatchedFilter {
                tier: FilterTier::BuiltIn,
                pack_id: pack.pack.id.clone(),
                pack_version: pack.pack.version.clone(),
                filter: filter.clone(),
            });
        }
    }

    None
}

/// Reads and parses (but does not validate or check trust for) a filter pack file. Used by
/// CLI-side `filters list`/`verify`, which need to show untrusted/invalid packs too, not just
/// the ones [`resolve_matching_filter`] would actually apply.
pub fn parse_pack_file(path: &Path) -> Result<FilterPack, TokenFoldError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        TokenFoldError::InvalidInput(format!("failed to read {}: {e}", path.display()))
    })?;
    FilterPack::parse(&text)
}

// ---------------------------------------------------------------------------------------------
// Built-ins (D-002 dogfooding scope: git diff, git status, cargo build/test log framing)
// ---------------------------------------------------------------------------------------------

/// D-002's resolved first consumer is solo dogfooding (git diffs, build/test logs, agent
/// tool-call JSON) — these three built-ins deliberately cover only that, not a broad command
/// library. Encoded as Rust struct literals rather than parsed TOML strings: for packs this
/// small, embedding multi-line fixture text in Rust raw strings and building the structs
/// directly is less code and avoids nested-TOML-array-of-tables escaping entirely (the task's
/// own "whichever is less code" framing).
pub fn built_in_packs() -> &'static [FilterPack] {
    static PACKS: OnceLock<Vec<FilterPack>> = OnceLock::new();
    PACKS.get_or_init(|| vec![git_diff_pack(), git_status_pack(), build_test_log_pack()])
}

fn git_diff_pack() -> FilterPack {
    let input = concat!(
        "diff --git a/src/lib.rs b/src/lib.rs\n",
        "index 83db48f..bf269c9 100644\n",
        "--- a/src/lib.rs\n",
        "+++ b/src/lib.rs\n",
        "@@ -1,3 +1,4 @@\n",
        "+pub mod filters;\n",
        " pub mod budget;\n",
        " pub mod errors;\n",
        " pub mod input;"
    );
    let output = concat!(
        "diff --git a/src/lib.rs b/src/lib.rs\n",
        "--- a/src/lib.rs\n",
        "+++ b/src/lib.rs\n",
        "@@ -1,3 +1,4 @@\n",
        "+pub mod filters;\n",
        " pub mod budget;\n",
        " pub mod errors;\n",
        " pub mod input;"
    );
    FilterPack {
        schema_version: SCHEMA_VERSION.to_string(),
        pack: PackMeta {
            id: "git-diff".to_string(),
            version: "1.0.0".to_string(),
        },
        filters: vec![Filter {
            id: "git-diff-default".to_string(),
            version: "1.0.0".to_string(),
            match_command: vec!["git".to_string(), "diff".to_string()],
            match_output: None,
            stages: vec![
                Stage::StripAnsi,
                Stage::StripLines {
                    pattern: r"^index [0-9a-f]".to_string(),
                },
                Stage::MaxLines { limit: 500 },
            ],
            fixtures: vec![Fixture {
                name: "strips-index-line".to_string(),
                input: input.to_string(),
                output: output.to_string(),
                expected_token_delta: 7,
            }],
        }],
    }
}

fn git_status_pack() -> FilterPack {
    let input = concat!(
        "On branch main\n",
        "Your branch is up to date with 'origin/main'.\n",
        "\n",
        "Changes not staged for commit:\n",
        "  (use \"git add <file>...\" to update what will be committed)\n",
        "  (use \"git restore <file>...\" to discard changes in working directory)\n",
        "\tmodified:   src/lib.rs\n",
        "\n",
        "no changes added to commit (use \"git add\" and/or \"git commit -a\")"
    );
    let output = concat!(
        "On branch main\n",
        "Your branch is up to date with 'origin/main'.\n",
        "\n",
        "Changes not staged for commit:\n",
        "\tmodified:   src/lib.rs\n",
        "\n",
        "no changes added to commit (use \"git add\" and/or \"git commit -a\")"
    );
    FilterPack {
        schema_version: SCHEMA_VERSION.to_string(),
        pack: PackMeta {
            id: "git-status".to_string(),
            version: "1.0.0".to_string(),
        },
        filters: vec![Filter {
            id: "git-status-default".to_string(),
            version: "1.0.0".to_string(),
            match_command: vec!["git".to_string(), "status".to_string()],
            match_output: None,
            stages: vec![
                Stage::StripAnsi,
                Stage::StripLines {
                    pattern: r"^\s*\(use ".to_string(),
                },
                Stage::MaxLines { limit: 200 },
            ],
            fixtures: vec![Fixture {
                name: "strips-use-hint-lines".to_string(),
                input: input.to_string(),
                output: output.to_string(),
                expected_token_delta: 33,
            }],
        }],
    }
}

fn build_test_log_pack() -> FilterPack {
    let input = concat!(
        "   Compiling tokenfold-core v0.1.0 (/workspace/tokenfold/crates/tokenfold-core)\n",
        "   Compiling tokenfold-cli v0.1.0 (/workspace/tokenfold/crates/tokenfold-cli)\n",
        "    Finished test [unoptimized + debuginfo] target(s) in 4.21s\n",
        "     Running unittests src/lib.rs\n",
        "\n",
        "running 2 tests\n",
        "test filters::tests::builtin_git_diff_fixture_passes ... ok\n",
        "test filters::tests::builtin_git_status_fixture_passes ... ok\n",
        "\n",
        "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s"
    );
    let output = concat!(
        "    Finished test [unoptimized + debuginfo] target(s) in 4.21s\n",
        "     Running unittests src/lib.rs\n",
        "\n",
        "running 2 tests\n",
        "test filters::tests::builtin_git_diff_fixture_passes ... ok\n",
        "test filters::tests::builtin_git_status_fixture_passes ... ok\n",
        "\n",
        "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s"
    );
    FilterPack {
        schema_version: SCHEMA_VERSION.to_string(),
        pack: PackMeta {
            id: "build-test-log".to_string(),
            version: "1.0.0".to_string(),
        },
        filters: vec![Filter {
            // Generic cargo build/test log filter (D-002 dogfooding scope: this repo's own
            // `cargo test`/`cargo build`); `match_command = ["cargo"]` fires on any cargo
            // subcommand rather than one filter per subcommand, keeping this a single built-in.
            id: "cargo-build-test-log".to_string(),
            version: "1.0.0".to_string(),
            match_command: vec!["cargo".to_string()],
            match_output: None,
            stages: vec![
                Stage::StripAnsi,
                Stage::StripLines {
                    pattern: r"^\s*(Compiling|Downloaded|Fresh|Downloading) ".to_string(),
                },
                Stage::MaxLines { limit: 300 },
            ],
            fixtures: vec![Fixture {
                name: "strips-compiling-noise-lines".to_string(),
                input: input.to_string(),
                output: output.to_string(),
                expected_token_delta: 39,
            }],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tokenfold_filters_test_{tag}_{}_{n}",
            std::process::id()
        ))
    }

    // --- schema validation --------------------------------------------------------------

    #[test]
    fn parse_rejects_unknown_top_level_field() {
        let toml_str = r#"
schema_version = "1.0"
oops = true

[pack]
id = "x"
version = "1.0.0"
"#;
        let err = FilterPack::parse(toml_str).unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
    }

    #[test]
    fn parse_rejects_unknown_stage_field() {
        let toml_str = r#"
schema_version = "1.0"

[pack]
id = "x"
version = "1.0.0"

[[filters]]
id = "f"
version = "1.0.0"
match_command = ["echo"]

[[filters.stages]]
type = "head"
count = 5
shell = "rm -rf /"
"#;
        let err = FilterPack::parse(toml_str).unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
    }

    #[test]
    fn validate_rejects_unsupported_schema_version() {
        let pack = FilterPack {
            schema_version: "2.0".to_string(),
            pack: PackMeta {
                id: "x".to_string(),
                version: "1.0.0".to_string(),
            },
            filters: vec![],
        };
        assert!(pack.validate().is_err());
    }

    #[test]
    fn validate_rejects_unsafe_nested_quantifier_regex() {
        let pack = FilterPack {
            schema_version: SCHEMA_VERSION.to_string(),
            pack: PackMeta {
                id: "x".to_string(),
                version: "1.0.0".to_string(),
            },
            filters: vec![Filter {
                id: "f".to_string(),
                version: "1.0.0".to_string(),
                match_command: vec!["echo".to_string()],
                match_output: None,
                stages: vec![Stage::StripLines {
                    pattern: "(a+)+".to_string(),
                }],
                fixtures: vec![],
            }],
        };
        assert!(pack.validate().is_err());
    }

    #[test]
    fn validate_accepts_a_well_formed_pack() {
        assert!(git_diff_pack().validate().is_ok());
        assert!(git_status_pack().validate().is_ok());
        assert!(build_test_log_pack().validate().is_ok());
    }

    // --- pattern safety guard ------------------------------------------------------------

    #[test]
    fn pattern_safety_rejects_overlong_pattern() {
        let long_pattern = "a".repeat(MAX_PATTERN_LEN + 1);
        assert!(check_pattern_safety(&long_pattern).is_err());
    }

    #[test]
    fn pattern_safety_rejects_nested_quantifier_shapes() {
        for p in ["(a+)+", "(a*)*", "(a+)*", "(x{2,3})+"] {
            assert!(
                check_pattern_safety(p).is_err(),
                "expected rejection for {p}"
            );
        }
    }

    #[test]
    fn pattern_safety_allows_reasonable_patterns() {
        for p in [
            "^index [0-9a-f]",
            r"^\s*\(use ",
            r"^\s*(Compiling|Downloaded) ",
        ] {
            assert!(check_pattern_safety(p).is_ok(), "expected {p} to pass");
        }
    }

    /// Mirrors `transforms::redaction`'s own ReDoS canary: proves that applying a filter stage
    /// (via this workspace's linear-time `regex` engine) over deliberately pathological *input
    /// text* completes within a bounded wall-clock budget, regardless of how "bait"-shaped the
    /// text is. The pattern itself is a normal, safety-guard-passing regex — see module doc for
    /// why the pattern side is guarded separately.
    #[test]
    fn redos_canary_completes_within_time_budget() {
        let filter = Filter {
            id: "canary".to_string(),
            version: "1.0.0".to_string(),
            match_command: vec!["echo".to_string()],
            match_output: None,
            stages: vec![Stage::Replace {
                pattern: r"a+b".to_string(),
                replacement: "X".to_string(),
            }],
            fixtures: vec![],
        };

        let long_run_of_a = "a".repeat(40_000);
        let start = std::time::Instant::now();
        filter.apply(long_run_of_a.as_bytes()).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));

        let nested_quantifier_bait = "a".repeat(20_000) + "!";
        let start = std::time::Instant::now();
        filter.apply(nested_quantifier_bait.as_bytes()).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    // --- stage execution -------------------------------------------------------------------

    #[test]
    fn stage_strip_ansi_removes_escape_codes() {
        let text = "\x1b[31mred text\x1b[0m plain";
        assert_eq!(
            apply_stage(&Stage::StripAnsi, text).unwrap(),
            "red text plain"
        );
    }

    #[test]
    fn stage_replace_substitutes_pattern() {
        let stage = Stage::Replace {
            pattern: r"\d+".to_string(),
            replacement: "N".to_string(),
        };
        assert_eq!(
            apply_stage(&stage, "line 123 and 456").unwrap(),
            "line N and N"
        );
    }

    #[test]
    fn stage_keep_lines_keeps_only_matches() {
        let stage = Stage::KeepLines {
            pattern: "^ERROR".to_string(),
        };
        let text = "INFO ok\nERROR bad\nINFO fine\nERROR worse";
        assert_eq!(apply_stage(&stage, text).unwrap(), "ERROR bad\nERROR worse");
    }

    #[test]
    fn stage_strip_lines_removes_matches() {
        let stage = Stage::StripLines {
            pattern: "^DEBUG".to_string(),
        };
        let text = "DEBUG noisy\nINFO keep\nDEBUG more noise\nWARN keep";
        assert_eq!(apply_stage(&stage, text).unwrap(), "INFO keep\nWARN keep");
    }

    #[test]
    fn stage_head_keeps_first_n_lines() {
        let stage = Stage::Head { count: 2 };
        assert_eq!(apply_stage(&stage, "a\nb\nc\nd").unwrap(), "a\nb");
    }

    #[test]
    fn stage_tail_keeps_last_n_lines() {
        let stage = Stage::Tail { count: 2 };
        assert_eq!(apply_stage(&stage, "a\nb\nc\nd").unwrap(), "c\nd");
    }

    #[test]
    fn stage_max_lines_is_noop_under_limit() {
        let stage = Stage::MaxLines { limit: 10 };
        assert_eq!(apply_stage(&stage, "a\nb\nc").unwrap(), "a\nb\nc");
    }

    #[test]
    fn stage_max_lines_truncates_and_reports_count_over_limit() {
        let stage = Stage::MaxLines { limit: 2 };
        let out = apply_stage(&stage, "a\nb\nc\nd").unwrap();
        assert_eq!(out, "a\nb\n... [tokenfold: truncated 2 more lines]");
    }

    #[test]
    fn stage_truncate_is_noop_under_limit() {
        let stage = Stage::Truncate { max_bytes: 100 };
        assert_eq!(apply_stage(&stage, "short").unwrap(), "short");
    }

    #[test]
    fn stage_truncate_cuts_at_byte_limit_with_marker() {
        let stage = Stage::Truncate { max_bytes: 5 };
        let out = apply_stage(&stage, "abcdefghij").unwrap();
        assert_eq!(out, "abcde... [tokenfold: truncated]");
    }

    #[test]
    fn stage_on_empty_substitutes_only_when_empty() {
        let stage = Stage::OnEmpty {
            value: "(nothing to show)".to_string(),
        };
        assert_eq!(apply_stage(&stage, "").unwrap(), "(nothing to show)");
        assert_eq!(apply_stage(&stage, "still here").unwrap(), "still here");
    }

    #[test]
    fn filter_apply_runs_stages_in_declared_order() {
        let filter = Filter {
            id: "multi".to_string(),
            version: "1.0.0".to_string(),
            match_command: vec!["echo".to_string()],
            match_output: None,
            stages: vec![
                Stage::StripLines {
                    pattern: "^DEBUG".to_string(),
                },
                Stage::Head { count: 1 },
            ],
            fixtures: vec![],
        };
        let out = filter
            .apply(b"DEBUG noisy\nINFO first\nINFO second")
            .unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "INFO first");
    }

    #[test]
    fn matches_command_requires_argv_prefix() {
        let filter = Filter {
            id: "f".to_string(),
            version: "1.0.0".to_string(),
            match_command: vec!["git".to_string(), "diff".to_string()],
            match_output: None,
            stages: vec![],
            fixtures: vec![],
        };
        assert!(filter.matches_command(&["git".to_string(), "diff".to_string()]));
        assert!(filter.matches_command(&[
            "git".to_string(),
            "diff".to_string(),
            "--stat".to_string()
        ]));
        assert!(!filter.matches_command(&["git".to_string(), "status".to_string()]));
        assert!(!filter.matches_command(&["git".to_string()]));
    }

    #[test]
    fn matches_also_gates_on_match_output_when_present() {
        let filter = Filter {
            id: "f".to_string(),
            version: "1.0.0".to_string(),
            match_command: vec!["git".to_string(), "diff".to_string()],
            match_output: Some("^diff --git".to_string()),
            stages: vec![],
            fixtures: vec![],
        };
        let argv = vec!["git".to_string(), "diff".to_string()];
        assert!(filter.matches(&argv, b"diff --git a/x b/x\n..."));
        assert!(!filter.matches(&argv, b"nothing relevant here"));
    }

    // --- built-in fixtures -----------------------------------------------------------------

    #[test]
    fn builtin_git_diff_fixture_passes() {
        let pack = git_diff_pack();
        let checks = pack.run_fixtures().unwrap();
        assert_eq!(checks.len(), 1);
        assert!(checks[0].passed(), "{checks:?}");
    }

    #[test]
    fn builtin_git_status_fixture_passes() {
        let pack = git_status_pack();
        let checks = pack.run_fixtures().unwrap();
        assert_eq!(checks.len(), 1);
        assert!(checks[0].passed(), "{checks:?}");
    }

    #[test]
    fn builtin_build_test_log_fixture_passes() {
        let pack = build_test_log_pack();
        let checks = pack.run_fixtures().unwrap();
        assert_eq!(checks.len(), 1);
        assert!(checks[0].passed(), "{checks:?}");
    }

    #[test]
    fn built_in_packs_cover_exactly_the_dogfooding_scope() {
        let ids: Vec<&str> = built_in_packs()
            .iter()
            .map(|p| p.pack.id.as_str())
            .collect();
        assert_eq!(ids, vec!["git-diff", "git-status", "build-test-log"]);
    }

    // --- never_worse -------------------------------------------------------------------------

    #[test]
    fn never_worse_keeps_filtered_when_smaller() {
        let raw = b"a very long raw command output that is definitely long".repeat(3);
        let filtered = b"short".to_vec();
        let outcome = never_worse(&raw, &filtered);
        assert!(outcome.used_filtered);
        assert_eq!(outcome.bytes, filtered);
    }

    #[test]
    fn never_worse_falls_back_to_raw_when_filtered_saves_nothing() {
        let raw = b"same size text".to_vec();
        let filtered = b"same size txt!".to_vec(); // same length, no savings
        let outcome = never_worse(&raw, &filtered);
        assert!(!outcome.used_filtered);
        assert_eq!(outcome.bytes, raw);
    }

    #[test]
    fn never_worse_falls_back_to_raw_when_filtered_is_larger() {
        let raw = b"short".to_vec();
        let filtered = b"this got longer somehow".to_vec();
        let outcome = never_worse(&raw, &filtered);
        assert!(!outcome.used_filtered);
        assert_eq!(outcome.bytes, raw);
    }

    // --- trust store -------------------------------------------------------------------------

    #[test]
    fn trust_store_round_trips_through_disk() {
        let path = temp_path("trust_roundtrip.json");
        let mut store = TrustStore::load(&path); // missing file -> empty, not an error
        assert!(store.entries.is_empty());

        let canonical = PathBuf::from("/some/canonical/filters.toml");
        store.trust(&canonical, b"pack contents", "1.0");
        store.save(&path).unwrap();

        let reloaded = TrustStore::load(&path);
        assert!(reloaded.is_trusted(&canonical, b"pack contents", "1.0"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn trust_store_content_change_invalidates_trust() {
        let mut store = TrustStore::default();
        let path = PathBuf::from("/x/filters.toml");
        store.trust(&path, b"original content", "1.0");
        assert!(store.is_trusted(&path, b"original content", "1.0"));
        assert!(!store.is_trusted(&path, b"tampered content", "1.0"));
    }

    #[test]
    fn trust_store_schema_version_mismatch_is_not_trusted() {
        let mut store = TrustStore::default();
        let path = PathBuf::from("/x/filters.toml");
        store.trust(&path, b"content", "1.0");
        assert!(!store.is_trusted(&path, b"content", "2.0"));
    }

    #[test]
    fn corrupt_trust_store_file_fails_closed_to_empty() {
        let path = temp_path("corrupt_trust.json");
        std::fs::write(&path, "not valid json {{{").unwrap();
        let store = TrustStore::load(&path);
        assert!(store.entries.is_empty());
        std::fs::remove_file(&path).ok();
    }

    // --- precedence resolution -----------------------------------------------------------

    fn write_pack(
        path: &Path,
        pack_id: &str,
        filter_id: &str,
        match_command: &[&str],
        stage_marker: &str,
    ) {
        let match_cmd = match_command
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let toml_str = format!(
            r#"
schema_version = "1.0"

[pack]
id = "{pack_id}"
version = "1.0.0"

[[filters]]
id = "{filter_id}"
version = "1.0.0"
match_command = [{match_cmd}]

[[filters.stages]]
type = "replace"
pattern = "MARKER"
replacement = "{stage_marker}"
"#
        );
        std::fs::write(path, toml_str).unwrap();
    }

    #[test]
    fn resolve_matching_filter_prefers_project_over_user_over_builtin() {
        let project_path = temp_path("precedence_project.toml");
        let user_path = temp_path("precedence_user.toml");
        let trust_path = temp_path("precedence_trust.json");

        write_pack(
            &project_path,
            "project-pack",
            "f",
            &["demo"],
            "FROM_PROJECT",
        );
        write_pack(&user_path, "user-pack", "f", &["demo"], "FROM_USER");

        let mut trust_store = TrustStore::default();
        let project_bytes = std::fs::read(&project_path).unwrap();
        let project_canonical = std::fs::canonicalize(&project_path).unwrap();
        trust_store.trust(&project_canonical, &project_bytes, SCHEMA_VERSION);
        let user_bytes = std::fs::read(&user_path).unwrap();
        let user_canonical = std::fs::canonicalize(&user_path).unwrap();
        trust_store.trust(&user_canonical, &user_bytes, SCHEMA_VERSION);
        trust_store.save(&trust_path).unwrap();

        let argv = vec!["demo".to_string()];
        let lookup = FilterLookup {
            argv: &argv,
            raw_output: b"",
            enabled: true,
            project_filters_path: Some(&project_path),
            user_filters_path: Some(&user_path),
            trust_store_path: &trust_path,
            trust_project_filters: false,
        };
        let matched = resolve_matching_filter(&lookup).expect("project filter should match");
        assert_eq!(matched.tier, FilterTier::Project);
        assert_eq!(matched.pack_id, "project-pack");

        // Remove the project file entirely: user tier should now win.
        std::fs::remove_file(&project_path).ok();
        let matched = resolve_matching_filter(&lookup).expect("user filter should match");
        assert_eq!(matched.tier, FilterTier::User);
        assert_eq!(matched.pack_id, "user-pack");

        // Remove the user file too: falls through to a real built-in (git diff) for a
        // different, actually-matching argv, proving the built-in tier is reachable.
        std::fs::remove_file(&user_path).ok();
        let git_argv = vec!["git".to_string(), "diff".to_string()];
        let git_lookup = FilterLookup {
            argv: &git_argv,
            raw_output: b"",
            ..lookup
        };
        let matched = resolve_matching_filter(&git_lookup).expect("built-in should match");
        assert_eq!(matched.tier, FilterTier::BuiltIn);
        assert_eq!(matched.pack_id, "git-diff");

        std::fs::remove_file(&trust_path).ok();
    }

    #[test]
    fn untrusted_project_filter_is_skipped_not_fatal() {
        let project_path = temp_path("untrusted_project.toml");
        let trust_path = temp_path("untrusted_trust.json");
        write_pack(
            &project_path,
            "project-pack",
            "f",
            &["demo"],
            "FROM_PROJECT",
        );
        // No trust recorded at all.

        let argv = vec!["demo".to_string()];
        let lookup = FilterLookup {
            argv: &argv,
            raw_output: b"",
            enabled: true,
            project_filters_path: Some(&project_path),
            user_filters_path: None,
            trust_store_path: &trust_path,
            trust_project_filters: false,
        };
        assert!(resolve_matching_filter(&lookup).is_none());

        std::fs::remove_file(&project_path).ok();
    }

    #[test]
    fn trust_project_filters_bypass_skips_the_trust_check_for_project_tier_only() {
        let project_path = temp_path("bypass_project.toml");
        let trust_path = temp_path("bypass_trust.json");
        write_pack(
            &project_path,
            "project-pack",
            "f",
            &["demo"],
            "FROM_PROJECT",
        );

        let argv = vec!["demo".to_string()];
        let lookup = FilterLookup {
            argv: &argv,
            raw_output: b"",
            enabled: true,
            project_filters_path: Some(&project_path),
            user_filters_path: None,
            trust_store_path: &trust_path,
            trust_project_filters: true,
        };
        let matched = resolve_matching_filter(&lookup).expect("bypass should trust project tier");
        assert_eq!(matched.tier, FilterTier::Project);

        std::fs::remove_file(&project_path).ok();
    }

    #[test]
    fn disabled_filters_never_match_anything() {
        let argv = vec!["git".to_string(), "diff".to_string()];
        let trust_path = temp_path("disabled_trust.json");
        let lookup = FilterLookup {
            argv: &argv,
            raw_output: b"",
            enabled: false,
            project_filters_path: None,
            user_filters_path: None,
            trust_store_path: &trust_path,
            trust_project_filters: false,
        };
        assert!(resolve_matching_filter(&lookup).is_none());
    }

    // --- "cannot execute shell commands or read arbitrary files" by construction ------------

    #[test]
    fn stage_schema_has_no_shell_or_file_read_variant() {
        // Exhaustive match with no `_` wildcard: if a shell/file-read `Stage` variant were ever
        // added, this would fail to compile until it was explicitly named here, which is the
        // proof this test is after.
        fn stage_name(stage: &Stage) -> &'static str {
            match stage {
                Stage::StripAnsi => "strip_ansi",
                Stage::Replace { .. } => "replace",
                Stage::KeepLines { .. } => "keep_lines",
                Stage::StripLines { .. } => "strip_lines",
                Stage::Head { .. } => "head",
                Stage::Tail { .. } => "tail",
                Stage::MaxLines { .. } => "max_lines",
                Stage::Truncate { .. } => "truncate",
                Stage::OnEmpty { .. } => "on_empty",
            }
        }
        let all_names = [
            stage_name(&Stage::StripAnsi),
            stage_name(&Stage::Replace {
                pattern: String::new(),
                replacement: String::new(),
            }),
            stage_name(&Stage::KeepLines {
                pattern: String::new(),
            }),
            stage_name(&Stage::StripLines {
                pattern: String::new(),
            }),
            stage_name(&Stage::Head { count: 0 }),
            stage_name(&Stage::Tail { count: 0 }),
            stage_name(&Stage::MaxLines { limit: 0 }),
            stage_name(&Stage::Truncate { max_bytes: 0 }),
            stage_name(&Stage::OnEmpty {
                value: String::new(),
            }),
        ];
        assert_eq!(all_names.len(), 9);
        for forbidden in [
            "exec",
            "shell",
            "command",
            "read_file",
            "read",
            "run",
            "spawn",
        ] {
            assert!(
                !all_names.contains(&forbidden),
                "Stage schema must never gain a {forbidden:?} variant"
            );
        }
    }
}
