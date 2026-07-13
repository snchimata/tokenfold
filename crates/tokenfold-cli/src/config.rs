//! `tokenfold.toml` / env-var / flag precedence, per INTERFACES.md Part 3. Only the
//! `[compression]`, `[output]`, `[safety]`, `[retrieval]` (F-045), `[analytics]` (F-046), and
//! `[filters]` (F-047) sections are implemented — the remaining documented sections (`proxy`,
//! `update`, `benchmark`, `estimator`) describe subsystems that don't exist yet (v0.2+); they
//! parse as ignored unknown top-level tables rather than being rejected, so a config file
//! following the full documented schema doesn't break against this subset.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokenfold_core::{CompressionMode, InputFormat, TaskScope, TokenFoldError};

use crate::args::{ModeArg, TaskScopeArg};
use crate::format::FormatArg;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    compression: CompressionSection,
    output: OutputSection,
    safety: SafetySection,
    retrieval: RetrievalSection,
    analytics: AnalyticsSection,
    filters: FiltersSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CompressionSection {
    mode: Option<ModeArg>,
    target_tokens: Option<usize>,
    format: Option<String>,
    task_scope: Option<TaskScopeArg>,
    preserve_latest_user_message: Option<bool>,
    disabled: Vec<String>,
    experimental: bool,
    enable: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OutputSection {
    no_color: bool,
    quiet: bool,
    json: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SafetySection {
    unsafe_disable_redaction: bool,
}

/// F-045 `[retrieval]` — mirrors INTERFACES.md's schema block. `store_originals`/`namespace`
/// are threaded into `CompressionPolicy` (see `build_policy`); `ttl_seconds`/`max_store_bytes`
/// only affect `tokenfold retrieve`'s store construction in this pass — `compress`/`wrap`-time
/// storage always uses `retrieval_store`'s own default TTL/backend/path (documented v0.2 scope
/// cut, see ROADMAP.md's F-045 exit-criterion note).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RetrievalSection {
    store_originals: bool,
    namespace: Option<String>,
    ttl_seconds: Option<u64>,
    max_store_bytes: Option<u64>,
    backend: Option<String>,
    store_path: Option<PathBuf>,
}

/// F-046 `[analytics]` — mirrors INTERFACES.md's schema block exactly (`enabled`, `ledger_db`,
/// `retention_days`, `hash_project_paths`).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AnalyticsSection {
    enabled: Option<bool>,
    ledger_db: Option<PathBuf>,
    retention_days: Option<u64>,
    hash_project_paths: Option<bool>,
}

/// F-047 `[filters]` — mirrors INTERFACES.md's schema block exactly (`enabled`,
/// `project_filters`, `user_filters`, `trust_store`, `trust_project_filters`).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FiltersSection {
    enabled: Option<bool>,
    project_filters: Option<PathBuf>,
    user_filters: Option<PathBuf>,
    trust_store: Option<PathBuf>,
    trust_project_filters: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub mode: Option<ModeArg>,
    pub target_tokens: Option<usize>,
    pub format: Option<FormatArg>,
    pub disable: Vec<String>,
    pub json: bool,
    pub no_color: bool,
    pub quiet: bool,
    pub unsafe_disable_redaction: bool,
    pub experimental: bool,
    pub task_scope: Option<TaskScopeArg>,
    pub enable: Vec<String>,
    pub store_originals: bool,
    pub retrieve_namespace: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Effective {
    pub mode: CompressionMode,
    pub target_tokens: Option<usize>,
    /// `None` means the user did not pin a format: sniff it per-input (see `format::detect_format`).
    pub format: Option<InputFormat>,
    pub task_scope: TaskScope,
    pub disabled: Vec<String>,
    pub experimental: bool,
    pub enable: Vec<String>,
    pub unsafe_disable_redaction: bool,
    pub preserve_latest_user_message: bool,
    pub no_color: bool,
    pub quiet: bool,
    pub json: bool,
    pub retrieval_store_originals: bool,
    pub retrieval_namespace: String,
    pub retrieval_ttl_seconds: Option<u64>,
    /// Resolved and validated (precedence + `deny_unknown_fields`), but not yet consumed by
    /// any command surface in this pass: `tokenfold retrieve gc` (ROADMAP.md F-045) — the only
    /// natural consumer of a size cap — isn't wired up yet. Kept `pub` and tested so a future
    /// `gc` subcommand only needs to read it, not add config plumbing.
    #[allow(dead_code)]
    pub retrieval_max_store_bytes: Option<u64>,
    pub retrieval_backend: String,
    pub retrieval_store_path: Option<PathBuf>,
    pub analytics_enabled: bool,
    pub analytics_ledger_path: PathBuf,
    pub analytics_retention_days: u64,
    pub analytics_hash_project_paths: bool,
    pub filters_enabled: bool,
    pub filters_project_filters_path: PathBuf,
    pub filters_user_filters_path: PathBuf,
    pub filters_trust_store_path: PathBuf,
    pub filters_trust_project_filters: bool,
}

#[derive(Debug)]
pub struct Resolved {
    pub effective: Effective,
    pub config_path: Option<PathBuf>,
}

pub fn resolve(
    overrides: &CliOverrides,
    explicit_config: Option<&Path>,
) -> Result<Resolved, TokenFoldError> {
    let (cfg, config_path) = load(explicit_config)?;

    let mode = match overrides.mode {
        Some(m) => m.to_core(),
        None => match env_string("TOKENFOLD_COMPRESSION_MODE") {
            Some(s) => ModeArg::parse(&s)
                .map_err(TokenFoldError::ConfigError)?
                .to_core(),
            None => cfg
                .compression
                .mode
                .map(ModeArg::to_core)
                .unwrap_or(CompressionMode::Balanced),
        },
    };

    let target_tokens = if let Some(t) = overrides.target_tokens {
        Some(t)
    } else if let Some(raw) = env_string("TOKENFOLD_COMPRESSION_TARGET_TOKENS") {
        Some(raw.trim().parse::<usize>().map_err(|_| {
            TokenFoldError::ConfigError(format!(
                "invalid integer for TOKENFOLD_COMPRESSION_TARGET_TOKENS: {raw:?}"
            ))
        })?)
    } else {
        cfg.compression.target_tokens
    };

    let format = if let Some(f) = overrides.format {
        Some(f.to_input_format())
    } else if let Some(s) = env_string("TOKENFOLD_COMPRESSION_FORMAT") {
        Some(
            FormatArg::parse(&s)
                .map_err(TokenFoldError::ConfigError)?
                .to_input_format(),
        )
    } else if let Some(s) = &cfg.compression.format {
        Some(
            FormatArg::parse(s)
                .map_err(TokenFoldError::ConfigError)?
                .to_input_format(),
        )
    } else {
        None
    };
    // `auto` is the same as "unset": both mean "sniff the format from content".
    let format = format.filter(|f| !matches!(f, InputFormat::Auto));

    let task_scope = if let Some(t) = overrides.task_scope {
        t.to_core()
    } else if let Some(s) = env_string("TOKENFOLD_COMPRESSION_TASK_SCOPE") {
        TaskScopeArg::parse(&s)
            .map_err(TokenFoldError::ConfigError)?
            .to_core()
    } else if let Some(t) = cfg.compression.task_scope {
        t.to_core()
    } else {
        TaskScope::All
    };

    let disabled = if !overrides.disable.is_empty() {
        overrides.disable.clone()
    } else if let Some(v) = env_csv("TOKENFOLD_COMPRESSION_DISABLED") {
        v
    } else {
        cfg.compression.disabled.clone()
    };

    let enable = if !overrides.enable.is_empty() {
        overrides.enable.clone()
    } else if let Some(v) = env_csv("TOKENFOLD_COMPRESSION_ENABLE") {
        v
    } else {
        cfg.compression.enable.clone()
    };

    let experimental = resolve_bool(
        overrides.experimental,
        "TOKENFOLD_COMPRESSION_EXPERIMENTAL",
        cfg.compression.experimental,
    )?;

    let unsafe_disable_redaction = resolve_bool(
        overrides.unsafe_disable_redaction,
        "TOKENFOLD_SAFETY_UNSAFE_DISABLE_REDACTION",
        cfg.safety.unsafe_disable_redaction,
    )?;

    let preserve_latest_user_message = if let Some(v) =
        env_bool("TOKENFOLD_COMPRESSION_PRESERVE_LATEST_USER_MESSAGE").transpose()?
    {
        v
    } else {
        cfg.compression.preserve_latest_user_message.unwrap_or(true)
    };

    // `NO_COLOR` (bare, presence-only per no-color.org) and `TOKENFOLD_OUTPUT_NO_COLOR`
    // (explicit true/false) share the "environment" precedence tier; the explicit form wins
    // over the bare one when both are set.
    let no_color = if overrides.no_color {
        true
    } else if let Some(v) = env_bool("TOKENFOLD_OUTPUT_NO_COLOR").transpose()? {
        v
    } else if std::env::var_os("NO_COLOR").is_some() {
        true
    } else {
        cfg.output.no_color
    };

    let quiet = resolve_bool(overrides.quiet, "TOKENFOLD_OUTPUT_QUIET", cfg.output.quiet)?;

    let json = resolve_bool(overrides.json, "TOKENFOLD_OUTPUT_JSON", cfg.output.json)?;

    let retrieval_store_originals = resolve_bool(
        overrides.store_originals,
        "TOKENFOLD_RETRIEVAL_STORE_ORIGINALS",
        cfg.retrieval.store_originals,
    )?;

    let retrieval_namespace = if let Some(ns) = &overrides.retrieve_namespace {
        ns.clone()
    } else if let Some(ns) = env_string("TOKENFOLD_RETRIEVAL_NAMESPACE") {
        ns
    } else if let Some(ns) = &cfg.retrieval.namespace {
        ns.clone()
    } else {
        "default".to_string()
    };

    let retrieval_ttl_seconds = if let Some(raw) = env_string("TOKENFOLD_RETRIEVAL_TTL_SECONDS") {
        Some(raw.trim().parse::<u64>().map_err(|_| {
            TokenFoldError::ConfigError(format!(
                "invalid integer for TOKENFOLD_RETRIEVAL_TTL_SECONDS: {raw:?}"
            ))
        })?)
    } else {
        Some(
            cfg.retrieval
                .ttl_seconds
                .unwrap_or(tokenfold_core::retrieval_store::DEFAULT_TTL_SECONDS),
        )
    };

    let retrieval_max_store_bytes =
        if let Some(raw) = env_string("TOKENFOLD_RETRIEVAL_MAX_STORE_BYTES") {
            Some(raw.trim().parse::<u64>().map_err(|_| {
                TokenFoldError::ConfigError(format!(
                    "invalid integer for TOKENFOLD_RETRIEVAL_MAX_STORE_BYTES: {raw:?}"
                ))
            })?)
        } else {
            cfg.retrieval.max_store_bytes
        };

    let retrieval_backend = if let Some(raw) = env_string("TOKENFOLD_RETRIEVAL_BACKEND") {
        raw
    } else {
        cfg.retrieval
            .backend
            .clone()
            .unwrap_or_else(|| "filesystem".to_string())
    };

    let retrieval_store_path = if let Some(raw) = env_string("TOKENFOLD_RETRIEVAL_STORE_PATH") {
        Some(PathBuf::from(raw))
    } else {
        cfg.retrieval.store_path.clone()
    };

    let analytics_enabled = if let Some(v) = env_bool("TOKENFOLD_ANALYTICS_ENABLED").transpose()? {
        v
    } else {
        cfg.analytics.enabled.unwrap_or(true)
    };

    let analytics_ledger_path = if let Some(raw) = env_string("TOKENFOLD_ANALYTICS_LEDGER_DB") {
        PathBuf::from(raw)
    } else {
        cfg.analytics
            .ledger_db
            .clone()
            .unwrap_or_else(tokenfold_core::stats::LedgerStore::default_path)
    };

    let analytics_retention_days =
        if let Some(raw) = env_string("TOKENFOLD_ANALYTICS_RETENTION_DAYS") {
            raw.trim().parse::<u64>().map_err(|_| {
                TokenFoldError::ConfigError(format!(
                    "invalid integer for TOKENFOLD_ANALYTICS_RETENTION_DAYS: {raw:?}"
                ))
            })?
        } else {
            cfg.analytics.retention_days.unwrap_or(90)
        };

    let analytics_hash_project_paths =
        if let Some(v) = env_bool("TOKENFOLD_ANALYTICS_HASH_PROJECT_PATHS").transpose()? {
            v
        } else {
            cfg.analytics.hash_project_paths.unwrap_or(true)
        };

    let filters_enabled = if let Some(v) = env_bool("TOKENFOLD_FILTERS_ENABLED").transpose()? {
        v
    } else {
        cfg.filters.enabled.unwrap_or(true)
    };

    let filters_project_filters_path =
        if let Some(raw) = env_string("TOKENFOLD_FILTERS_PROJECT_FILTERS") {
            PathBuf::from(raw)
        } else {
            cfg.filters
                .project_filters
                .clone()
                .unwrap_or_else(|| PathBuf::from(".tokenfold/filters.toml"))
        };

    let filters_user_filters_path = if let Some(raw) = env_string("TOKENFOLD_FILTERS_USER_FILTERS")
    {
        PathBuf::from(raw)
    } else {
        cfg.filters
            .user_filters
            .clone()
            .unwrap_or_else(tokenfold_core::filters::default_user_filters_path)
    };

    let filters_trust_store_path = if let Some(raw) = env_string("TOKENFOLD_FILTERS_TRUST_STORE") {
        PathBuf::from(raw)
    } else {
        cfg.filters
            .trust_store
            .clone()
            .unwrap_or_else(tokenfold_core::filters::TrustStore::default_path)
    };

    // `TOKENFOLD_TRUST_PROJECT_FILTERS=1` is the literal, contract-documented CI override
    // (INTERFACES.md §7.3) — deliberately NOT namespaced as
    // `TOKENFOLD_FILTERS_TRUST_PROJECT_FILTERS` so it matches the documented name exactly.
    let filters_trust_project_filters =
        if let Some(v) = env_bool("TOKENFOLD_TRUST_PROJECT_FILTERS").transpose()? {
            v
        } else {
            cfg.filters.trust_project_filters.unwrap_or(false)
        };

    Ok(Resolved {
        effective: Effective {
            mode,
            target_tokens,
            format,
            task_scope,
            disabled,
            experimental,
            enable,
            unsafe_disable_redaction,
            preserve_latest_user_message,
            no_color,
            quiet,
            json,
            retrieval_store_originals,
            retrieval_namespace,
            retrieval_ttl_seconds,
            retrieval_max_store_bytes,
            retrieval_backend,
            retrieval_store_path,
            analytics_enabled,
            analytics_ledger_path,
            analytics_retention_days,
            analytics_hash_project_paths,
            filters_enabled,
            filters_project_filters_path,
            filters_user_filters_path,
            filters_trust_store_path,
            filters_trust_project_filters,
        },
        config_path,
    })
}

fn discover_config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TOKENFOLD_CONFIG") {
        return Some(PathBuf::from(p));
    }
    for candidate in ["tokenfold.toml", ".tokenfoldrc"] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")));
    if let Some(dir) = xdg_config_home {
        let p = dir.join("tokenfold").join("config.toml");
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(home) = home_dir() {
        let p = home.join(".tokenfoldrc");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn load(explicit_path: Option<&Path>) -> Result<(RawConfig, Option<PathBuf>), TokenFoldError> {
    let path = match explicit_path {
        Some(p) => Some(p.to_path_buf()),
        None => discover_config_path(),
    };
    let Some(path) = path else {
        return Ok((RawConfig::default(), None));
    };
    let text = std::fs::read_to_string(&path).map_err(|e| {
        TokenFoldError::ConfigError(format!("failed to read {}: {e}", path.display()))
    })?;
    let cfg: RawConfig = toml::from_str(&text).map_err(|e| {
        TokenFoldError::ConfigError(format!("invalid config at {}: {e}", path.display()))
    })?;
    Ok((cfg, Some(path)))
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn env_csv(name: &str) -> Option<Vec<String>> {
    env_string(name).map(|raw| {
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

fn env_bool(name: &str) -> Option<Result<bool, TokenFoldError>> {
    env_string(name).map(|raw| parse_bool(name, &raw))
}

/// `flags > env > config` for a boolean setting. The CLI layer only ever contributes `true`
/// (bare presence flags like `--quiet` can't express "explicitly false"), so a lower layer's
/// value only surfaces when the higher layer wasn't set at all — critically, an env var that's
/// explicitly `false` still overrides a `true` set in the config file.
fn resolve_bool(cli_true: bool, env_name: &str, config_val: bool) -> Result<bool, TokenFoldError> {
    if cli_true {
        return Ok(true);
    }
    if let Some(v) = env_bool(env_name).transpose()? {
        return Ok(v);
    }
    Ok(config_val)
}

fn parse_bool(name: &str, raw: &str) -> Result<bool, TokenFoldError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(TokenFoldError::ConfigError(format!(
            "invalid boolean for {name}: {raw:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env vars are process-global; serialize every test that touches them (or writes a shared
    // temp config file) so parallel `cargo test` threads don't race each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn resolve_bool_cli_flag_wins_over_everything() {
        let _g = lock();
        unsafe {
            std::env::remove_var("TOKENFOLD_TEST_BOOL");
        }
        assert!(resolve_bool(true, "TOKENFOLD_TEST_BOOL", false).unwrap());
    }

    #[test]
    fn resolve_bool_env_overrides_config_in_both_directions() {
        let _g = lock();
        unsafe {
            std::env::set_var("TOKENFOLD_TEST_BOOL", "false");
        }
        assert!(!resolve_bool(false, "TOKENFOLD_TEST_BOOL", true).unwrap());
        unsafe {
            std::env::set_var("TOKENFOLD_TEST_BOOL", "true");
        }
        assert!(resolve_bool(false, "TOKENFOLD_TEST_BOOL", false).unwrap());
        unsafe {
            std::env::remove_var("TOKENFOLD_TEST_BOOL");
        }
    }

    #[test]
    fn resolve_bool_falls_back_to_config_when_unset() {
        let _g = lock();
        unsafe {
            std::env::remove_var("TOKENFOLD_TEST_BOOL");
        }
        assert!(resolve_bool(false, "TOKENFOLD_TEST_BOOL", true).unwrap());
    }

    #[test]
    fn parse_bool_accepts_documented_spellings() {
        for v in ["1", "true", "YES", "On"] {
            assert!(parse_bool("x", v).unwrap());
        }
        for v in ["0", "false", "NO", "Off"] {
            assert!(!parse_bool("x", v).unwrap());
        }
        assert!(parse_bool("x", "maybe").is_err());
    }

    #[test]
    fn env_csv_trims_and_drops_empty_entries() {
        let _g = lock();
        unsafe {
            std::env::set_var("TOKENFOLD_TEST_CSV", " a, b ,,c");
        }
        assert_eq!(
            env_csv("TOKENFOLD_TEST_CSV").unwrap(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        unsafe {
            std::env::remove_var("TOKENFOLD_TEST_CSV");
        }
    }

    #[test]
    fn resolve_reads_a_config_file_and_env_overrides_it() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_config.toml");
        std::fs::write(
            &path,
            "[compression]\nmode = \"aggressive\"\ntarget_tokens = 50\n[output]\njson = true\n",
        )
        .unwrap();

        let resolved = resolve(&CliOverrides::default(), Some(&path)).unwrap();
        assert_eq!(resolved.effective.mode, CompressionMode::Aggressive);
        assert_eq!(resolved.effective.target_tokens, Some(50));
        assert!(resolved.effective.json);

        unsafe {
            std::env::set_var("TOKENFOLD_OUTPUT_JSON", "false");
            std::env::set_var("TOKENFOLD_COMPRESSION_MODE", "conservative");
        }
        let resolved = resolve(&CliOverrides::default(), Some(&path)).unwrap();
        assert!(!resolved.effective.json);
        assert_eq!(resolved.effective.mode, CompressionMode::Conservative);
        unsafe {
            std::env::remove_var("TOKENFOLD_OUTPUT_JSON");
            std::env::remove_var("TOKENFOLD_COMPRESSION_MODE");
        }

        let overrides = CliOverrides {
            mode: Some(ModeArg::Balanced),
            ..Default::default()
        };
        let resolved = resolve(&overrides, Some(&path)).unwrap();
        assert_eq!(resolved.effective.mode, CompressionMode::Balanced);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn resolve_rejects_unknown_field_as_config_error() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_config_typo.toml");
        std::fs::write(&path, "[compression]\nmdoe = \"balanced\"\n").unwrap();
        let err = resolve(&CliOverrides::default(), Some(&path)).unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn resolve_rejects_disabling_secret_redaction_via_disable_list() {
        // secret_redaction rejection itself is enforced by CompressionPolicyBuilder::build,
        // but `resolve()` must still surface the id through unmodified so that check can fire.
        let overrides = CliOverrides {
            disable: vec!["secret_redaction".to_string()],
            ..Default::default()
        };
        let resolved = resolve(&overrides, None).unwrap();
        assert_eq!(
            resolved.effective.disabled,
            vec!["secret_redaction".to_string()]
        );
    }

    #[test]
    fn retrieval_defaults_when_section_is_absent() {
        let _g = lock();
        let resolved = resolve(&CliOverrides::default(), None).unwrap();
        assert!(!resolved.effective.retrieval_store_originals);
        assert_eq!(resolved.effective.retrieval_namespace, "default");
        assert_eq!(
            resolved.effective.retrieval_ttl_seconds,
            Some(tokenfold_core::retrieval_store::DEFAULT_TTL_SECONDS)
        );
        assert_eq!(resolved.effective.retrieval_max_store_bytes, None);
        assert_eq!(resolved.effective.retrieval_backend, "filesystem");
        assert_eq!(resolved.effective.retrieval_store_path, None);
    }

    #[test]
    fn retrieval_section_parses_from_config_file() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_retrieval_config.toml");
        std::fs::write(
            &path,
            "[retrieval]\nstore_originals = true\nnamespace = \"proj\"\nttl_seconds = 100\nmax_store_bytes = 5000\nbackend = \"memory\"\n",
        )
        .unwrap();

        let resolved = resolve(&CliOverrides::default(), Some(&path)).unwrap();
        assert!(resolved.effective.retrieval_store_originals);
        assert_eq!(resolved.effective.retrieval_namespace, "proj");
        assert_eq!(resolved.effective.retrieval_ttl_seconds, Some(100));
        assert_eq!(resolved.effective.retrieval_max_store_bytes, Some(5000));
        assert_eq!(resolved.effective.retrieval_backend, "memory");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn retrieval_cli_flags_and_env_win_over_config_in_precedence_order() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_retrieval_precedence.toml");
        std::fs::write(
            &path,
            "[retrieval]\nstore_originals = false\nnamespace = \"from-config\"\n",
        )
        .unwrap();

        // env beats config.
        unsafe {
            std::env::set_var("TOKENFOLD_RETRIEVAL_STORE_ORIGINALS", "true");
            std::env::set_var("TOKENFOLD_RETRIEVAL_NAMESPACE", "from-env");
        }
        let resolved = resolve(&CliOverrides::default(), Some(&path)).unwrap();
        assert!(resolved.effective.retrieval_store_originals);
        assert_eq!(resolved.effective.retrieval_namespace, "from-env");

        // a CLI override beats env.
        let overrides = CliOverrides {
            retrieve_namespace: Some("from-flag".to_string()),
            ..Default::default()
        };
        let resolved = resolve(&overrides, Some(&path)).unwrap();
        assert_eq!(resolved.effective.retrieval_namespace, "from-flag");

        unsafe {
            std::env::remove_var("TOKENFOLD_RETRIEVAL_STORE_ORIGINALS");
            std::env::remove_var("TOKENFOLD_RETRIEVAL_NAMESPACE");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn retrieval_section_rejects_unknown_field() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_retrieval_typo.toml");
        std::fs::write(&path, "[retrieval]\nnaemspace = \"typo\"\n").unwrap();
        let err = resolve(&CliOverrides::default(), Some(&path)).unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn analytics_defaults_when_section_is_absent() {
        let _g = lock();
        let resolved = resolve(&CliOverrides::default(), None).unwrap();
        assert!(resolved.effective.analytics_enabled);
        assert_eq!(resolved.effective.analytics_retention_days, 90);
        assert!(resolved.effective.analytics_hash_project_paths);
        assert_eq!(
            resolved.effective.analytics_ledger_path,
            tokenfold_core::stats::LedgerStore::default_path()
        );
    }

    #[test]
    fn analytics_section_parses_from_config_file() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_analytics_config.toml");
        std::fs::write(
            &path,
            "[analytics]\nenabled = false\nledger_db = \"/tmp/custom_ledger.db\"\nretention_days = 30\nhash_project_paths = false\n",
        )
        .unwrap();

        let resolved = resolve(&CliOverrides::default(), Some(&path)).unwrap();
        assert!(!resolved.effective.analytics_enabled);
        assert_eq!(
            resolved.effective.analytics_ledger_path,
            PathBuf::from("/tmp/custom_ledger.db")
        );
        assert_eq!(resolved.effective.analytics_retention_days, 30);
        assert!(!resolved.effective.analytics_hash_project_paths);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn analytics_env_wins_over_config_in_precedence_order() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_analytics_precedence.toml");
        std::fs::write(&path, "[analytics]\nenabled = true\nretention_days = 90\n").unwrap();

        unsafe {
            std::env::set_var("TOKENFOLD_ANALYTICS_ENABLED", "false");
            std::env::set_var("TOKENFOLD_ANALYTICS_RETENTION_DAYS", "10");
        }
        let resolved = resolve(&CliOverrides::default(), Some(&path)).unwrap();
        assert!(!resolved.effective.analytics_enabled);
        assert_eq!(resolved.effective.analytics_retention_days, 10);

        unsafe {
            std::env::remove_var("TOKENFOLD_ANALYTICS_ENABLED");
            std::env::remove_var("TOKENFOLD_ANALYTICS_RETENTION_DAYS");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn analytics_section_rejects_unknown_field() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_analytics_typo.toml");
        std::fs::write(&path, "[analytics]\nenalbed = true\n").unwrap();
        let err = resolve(&CliOverrides::default(), Some(&path)).unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn filters_defaults_when_section_is_absent() {
        let _g = lock();
        let resolved = resolve(&CliOverrides::default(), None).unwrap();
        assert!(resolved.effective.filters_enabled);
        assert_eq!(
            resolved.effective.filters_project_filters_path,
            PathBuf::from(".tokenfold/filters.toml")
        );
        assert_eq!(
            resolved.effective.filters_user_filters_path,
            tokenfold_core::filters::default_user_filters_path()
        );
        assert_eq!(
            resolved.effective.filters_trust_store_path,
            tokenfold_core::filters::TrustStore::default_path()
        );
        assert!(!resolved.effective.filters_trust_project_filters);
    }

    #[test]
    fn filters_section_parses_from_config_file() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_filters_config.toml");
        std::fs::write(
            &path,
            "[filters]\nenabled = false\nproject_filters = \"custom/filters.toml\"\nuser_filters = \"/tmp/user_filters.toml\"\ntrust_store = \"/tmp/trust.json\"\ntrust_project_filters = true\n",
        )
        .unwrap();

        let resolved = resolve(&CliOverrides::default(), Some(&path)).unwrap();
        assert!(!resolved.effective.filters_enabled);
        assert_eq!(
            resolved.effective.filters_project_filters_path,
            PathBuf::from("custom/filters.toml")
        );
        assert_eq!(
            resolved.effective.filters_user_filters_path,
            PathBuf::from("/tmp/user_filters.toml")
        );
        assert_eq!(
            resolved.effective.filters_trust_store_path,
            PathBuf::from("/tmp/trust.json")
        );
        assert!(resolved.effective.filters_trust_project_filters);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn filters_env_wins_over_config_in_precedence_order() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_filters_precedence.toml");
        std::fs::write(&path, "[filters]\nenabled = true\n").unwrap();

        unsafe {
            std::env::set_var("TOKENFOLD_FILTERS_ENABLED", "false");
            // Contract-documented literal name, not the `TOKENFOLD_FILTERS_*` namespace.
            std::env::set_var("TOKENFOLD_TRUST_PROJECT_FILTERS", "1");
        }
        let resolved = resolve(&CliOverrides::default(), Some(&path)).unwrap();
        assert!(!resolved.effective.filters_enabled);
        assert!(resolved.effective.filters_trust_project_filters);

        unsafe {
            std::env::remove_var("TOKENFOLD_FILTERS_ENABLED");
            std::env::remove_var("TOKENFOLD_TRUST_PROJECT_FILTERS");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn filters_section_rejects_unknown_field() {
        let _g = lock();
        let path = std::env::temp_dir().join("tokenfold_cli_test_filters_typo.toml");
        std::fs::write(&path, "[filters]\nenalbed = true\n").unwrap();
        let err = resolve(&CliOverrides::default(), Some(&path)).unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
        std::fs::remove_file(&path).ok();
    }
}
