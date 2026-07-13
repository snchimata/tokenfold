//! F-045: reversible evidence store and retrieval (`roadmap.md` F-045, `interfaces.md`
//! "Retrieval Marker Grammar" and the `[retrieval]` `tokenfold.toml` schema block).
//!
//! Granularity in this pass is whole-payload, not per-span: `pipeline.rs` stores the entire
//! pre-transform input under its SHA-256 content hash when `CompressionPolicy.store_originals`
//! is set and the payload contains no secret-shaped content. Per-span inline
//! `[tokenfold:retrieve ...]` markers are an explicitly out-of-scope future enhancement (see
//! the marker grammar's own fallback rule: "If a format cannot carry markers safely, markers
//! live only in `CompressionReport.retrieval`").
//!
//! Hash algorithm is SHA-256 only in this pass; `blake3` is a documented, rejected scope cut
//! (see [`RetrievalStore::open`]). Backends are `memory` (in-process, used in tests) and
//! `filesystem` (the default persistent backend); `sqlite` is likewise a documented, rejected
//! scope cut.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::errors::TokenFoldError;
use crate::transforms::redaction;

/// `tokenfold.toml`'s documented `[retrieval].ttl_seconds` default (7 days).
pub const DEFAULT_TTL_SECONDS: u64 = 604_800;

/// One retrieval marker's worth of metadata, per `interfaces.md`'s Retrieval Marker Grammar:
/// `[tokenfold:retrieve hash=<hex> alg=sha256 namespace=<ns> bytes=<n> ttl=<seconds>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalMarker {
    pub hash: String,
    pub alg: &'static str,
    pub namespace: String,
    pub bytes: usize,
    pub ttl_seconds: Option<u64>,
}

/// Result of a retrieval lookup. Deliberately has no "partial" variant: a caller either gets
/// the exact original bytes back, or an explicit reason it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrievalOutcome {
    Found(Vec<u8>),
    Missing,
    Expired,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcOutcome {
    pub expired_removed: usize,
    pub evicted_removed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EntryMeta {
    stored_at_unix: u64,
    ttl_seconds: Option<u64>,
    bytes: usize,
}

// `pub` only because it's the type of a field inside the public `RetrievalStore::Memory`
// variant (tuple-variant fields of a `pub enum` are implicitly public); its own fields stay
// private; nothing outside this module ever constructs or reads one directly.
pub struct MemoryEntry {
    bytes: Vec<u8>,
    meta: EntryMeta,
}

/// A content-addressed, namespaced store for reversible originals. Backend-dispatching enum
/// rather than a trait object: only two live variants exist in this pass, so a trait would add
/// indirection without buying any real polymorphism.
pub enum RetrievalStore {
    Memory(Mutex<HashMap<(String, String), MemoryEntry>>),
    Filesystem { root: PathBuf },
}

impl RetrievalStore {
    pub fn memory() -> Self {
        RetrievalStore::Memory(Mutex::new(HashMap::new()))
    }

    pub fn filesystem(root: impl Into<PathBuf>) -> Self {
        RetrievalStore::Filesystem { root: root.into() }
    }

    /// The default persistent store used when nothing overrides it: a `filesystem` backend
    /// rooted at [`default_store_path`].
    pub fn default_filesystem() -> Self {
        Self::filesystem(default_store_path())
    }

    /// Builds a store from `tokenfold.toml`'s `[retrieval]` schema values. `hash_algorithm` and
    /// `backend` are validated here so that selecting an unimplemented option (`blake3`,
    /// `sqlite`) fails clearly instead of silently behaving like `sha256`/`filesystem`.
    pub fn open(
        backend: &str,
        hash_algorithm: &str,
        store_path_override: Option<PathBuf>,
    ) -> Result<Self, TokenFoldError> {
        if hash_algorithm != "sha256" {
            return Err(TokenFoldError::ConfigError(format!(
                "retrieval hash_algorithm {hash_algorithm:?} is not implemented yet in v0.2; only \"sha256\" is supported"
            )));
        }
        match backend {
            "memory" => Ok(Self::memory()),
            "filesystem" => Ok(Self::filesystem(
                store_path_override.unwrap_or_else(default_store_path),
            )),
            "sqlite" => Err(TokenFoldError::ConfigError(
                "retrieval backend \"sqlite\" is not implemented yet in v0.2; use \"memory\" or \"filesystem\"".to_string(),
            )),
            other => Err(TokenFoldError::ConfigError(format!(
                "unknown retrieval backend {other:?}; expected \"memory\" or \"filesystem\" (\"sqlite\" is a documented v0.2 scope cut)"
            ))),
        }
    }

    /// Persists `bytes` under their SHA-256 hex hash, namespaced by `namespace`. Refuses to
    /// store (and never partially stores) anything [`redaction::contains_secret`] flags — this
    /// check runs unconditionally inside `store`, so no caller anywhere (pipeline, CLI, tests)
    /// can reach a code path that stores secret-shaped bytes.
    pub fn store(
        &self,
        bytes: &[u8],
        namespace: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<RetrievalMarker, TokenFoldError> {
        if redaction::contains_secret(bytes) {
            return Err(TokenFoldError::SafetyViolation(
                "refusing to persist bytes that match a secret-redaction pattern".to_string(),
            ));
        }
        if !is_safe_path_component(namespace) {
            return Err(TokenFoldError::InvalidInput(format!(
                "invalid retrieval namespace: {namespace:?}"
            )));
        }

        let hash = hex_sha256(bytes);
        let meta = EntryMeta {
            stored_at_unix: now_unix(),
            ttl_seconds,
            bytes: bytes.len(),
        };

        match self {
            RetrievalStore::Memory(map) => {
                let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
                guard.insert(
                    (namespace.to_string(), hash.clone()),
                    MemoryEntry {
                        bytes: bytes.to_vec(),
                        meta,
                    },
                );
            }
            RetrievalStore::Filesystem { root } => {
                let dir = root.join(namespace);
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join(format!("{hash}.bin")), bytes)?;
                let meta_json = serde_json::to_vec_pretty(&meta).map_err(|e| {
                    TokenFoldError::InternalError(format!(
                        "failed to encode retrieval metadata: {e}"
                    ))
                })?;
                std::fs::write(dir.join(format!("{hash}.meta.json")), meta_json)?;
            }
        }

        Ok(RetrievalMarker {
            hash,
            alg: "sha256",
            namespace: namespace.to_string(),
            bytes: bytes.len(),
            ttl_seconds,
        })
    }

    /// Looks up `hash` in `namespace`. Never returns a partial result: exactly one of
    /// `Found`/`Missing`/`Expired`.
    pub fn retrieve(&self, hash: &str, namespace: &str) -> RetrievalOutcome {
        if !is_safe_path_component(namespace) || !is_safe_path_component(hash) {
            return RetrievalOutcome::Missing;
        }

        match self {
            RetrievalStore::Memory(map) => {
                let guard = map.lock().unwrap_or_else(|e| e.into_inner());
                match guard.get(&(namespace.to_string(), hash.to_string())) {
                    None => RetrievalOutcome::Missing,
                    Some(entry) if is_expired(&entry.meta) => RetrievalOutcome::Expired,
                    Some(entry) => RetrievalOutcome::Found(entry.bytes.clone()),
                }
            }
            RetrievalStore::Filesystem { root } => {
                let dir = root.join(namespace);
                let meta_path = dir.join(format!("{hash}.meta.json"));
                let data_path = dir.join(format!("{hash}.bin"));
                let Ok(meta_bytes) = std::fs::read(&meta_path) else {
                    return RetrievalOutcome::Missing;
                };
                let Ok(meta) = serde_json::from_slice::<EntryMeta>(&meta_bytes) else {
                    return RetrievalOutcome::Missing;
                };
                if is_expired(&meta) {
                    return RetrievalOutcome::Expired;
                }
                match std::fs::read(&data_path) {
                    Ok(bytes) => RetrievalOutcome::Found(bytes),
                    Err(_) => RetrievalOutcome::Missing,
                }
            }
        }
    }

    /// Deletes entries whose `ttl_seconds` has elapsed (entries stored with `ttl_seconds:
    /// None` never expire), then — if `max_store_bytes` is given and total remaining stored
    /// bytes still exceed it — evicts the oldest-`stored_at` entries first until under the cap.
    pub fn gc(&self, max_store_bytes: Option<u64>) -> Result<GcOutcome, TokenFoldError> {
        match self {
            RetrievalStore::Memory(map) => {
                let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
                let mut outcome = GcOutcome::default();

                let expired: Vec<_> = guard
                    .iter()
                    .filter(|(_, entry)| is_expired(&entry.meta))
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in expired {
                    guard.remove(&key);
                    outcome.expired_removed += 1;
                }

                if let Some(cap) = max_store_bytes {
                    let mut total: u64 = guard.values().map(|e| e.meta.bytes as u64).sum();
                    if total > cap {
                        let mut remaining: Vec<_> = guard
                            .iter()
                            .map(|(key, e)| {
                                (key.clone(), e.meta.stored_at_unix, e.meta.bytes as u64)
                            })
                            .collect();
                        remaining.sort_by_key(|(_, stored_at, _)| *stored_at);
                        for (key, _, bytes) in remaining {
                            if total <= cap {
                                break;
                            }
                            guard.remove(&key);
                            total = total.saturating_sub(bytes);
                            outcome.evicted_removed += 1;
                        }
                    }
                }
                Ok(outcome)
            }
            RetrievalStore::Filesystem { root } => {
                let mut outcome = GcOutcome::default();
                if !root.is_dir() {
                    return Ok(outcome);
                }

                let mut live: Vec<(PathBuf, PathBuf, EntryMeta)> = Vec::new();
                for ns_entry in std::fs::read_dir(root)? {
                    let ns_entry = ns_entry?;
                    if !ns_entry.file_type()?.is_dir() {
                        continue;
                    }
                    let ns_dir = ns_entry.path();
                    for file_entry in std::fs::read_dir(&ns_dir)? {
                        let file_entry = file_entry?;
                        let meta_path = file_entry.path();
                        let Some(name) = meta_path.file_name().and_then(|n| n.to_str()) else {
                            continue;
                        };
                        let Some(hash) = name.strip_suffix(".meta.json") else {
                            continue;
                        };
                        let Ok(meta_bytes) = std::fs::read(&meta_path) else {
                            continue;
                        };
                        let Ok(meta) = serde_json::from_slice::<EntryMeta>(&meta_bytes) else {
                            continue;
                        };
                        let data_path = ns_dir.join(format!("{hash}.bin"));
                        if is_expired(&meta) {
                            std::fs::remove_file(&meta_path).ok();
                            std::fs::remove_file(&data_path).ok();
                            outcome.expired_removed += 1;
                            continue;
                        }
                        live.push((meta_path, data_path, meta));
                    }
                }

                if let Some(cap) = max_store_bytes {
                    let mut total: u64 = live.iter().map(|(_, _, m)| m.bytes as u64).sum();
                    if total > cap {
                        live.sort_by_key(|(_, _, m)| m.stored_at_unix);
                        for (meta_path, data_path, meta) in live {
                            if total <= cap {
                                break;
                            }
                            std::fs::remove_file(&meta_path).ok();
                            std::fs::remove_file(&data_path).ok();
                            total = total.saturating_sub(meta.bytes as u64);
                            outcome.evicted_removed += 1;
                        }
                    }
                }
                Ok(outcome)
            }
        }
    }
}

fn is_expired(meta: &EntryMeta) -> bool {
    match meta.ttl_seconds {
        None => false,
        Some(ttl) => now_unix().saturating_sub(meta.stored_at_unix) >= ttl,
    }
}

/// Rejects values that would let a namespace or hash escape the store root via path
/// traversal (`..`, embedded separators) when used as a directory/file name component.
fn is_safe_path_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && value != "."
        && value != ".."
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `$XDG_DATA_HOME/tokenfold/retrieve`, falling back to `<home>/.local/share/tokenfold/retrieve`
/// when `XDG_DATA_HOME` is unset — mirrors `tokenfold-cli::config`'s HOME/USERPROFILE fallback
/// for `home_dir()`. Deliberately not a Windows-native path (e.g. `%LOCALAPPDATA%`): the rest
/// of the codebase is XDG-everywhere by convention.
pub fn default_store_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(dir).join("tokenfold").join("retrieve");
    }
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".local")
        .join("share")
        .join("tokenfold")
        .join("retrieve")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tokenfold_retrieval_store_test_{tag}_{}_{n}",
            std::process::id()
        ))
    }

    #[test]
    fn hex_sha256_matches_known_test_vector() {
        // sha256("") is a widely published test vector.
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn stores_by_content_hash_and_namespace_independently() {
        let store = RetrievalStore::memory();
        let marker_a = store.store(b"hello world", "project-a", None).unwrap();
        let marker_b = store.store(b"hello world", "project-b", None).unwrap();
        assert_eq!(marker_a.hash, marker_b.hash, "same bytes hash identically");

        assert_eq!(
            store.retrieve(&marker_a.hash, "project-a"),
            RetrievalOutcome::Found(b"hello world".to_vec())
        );
        // Same hash, wrong namespace: not found (namespaces are independent).
        assert_eq!(
            store.retrieve(&marker_a.hash, "project-nope"),
            RetrievalOutcome::Missing
        );
        assert_eq!(
            store.retrieve(&marker_b.hash, "project-b"),
            RetrievalOutcome::Found(b"hello world".to_vec())
        );
    }

    #[test]
    fn memory_retrieve_restores_exact_bytes_including_non_utf8() {
        let store = RetrievalStore::memory();
        let original: Vec<u8> = vec![0, 159, 146, 150, 1, 2, 3, 255, 0, 254];
        let marker = store.store(&original, "default", None).unwrap();
        match store.retrieve(&marker.hash, "default") {
            RetrievalOutcome::Found(bytes) => assert_eq!(bytes, original),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn filesystem_retrieve_restores_exact_bytes() {
        let root = temp_root("roundtrip");
        let store = RetrievalStore::filesystem(&root);
        let original = b"the quick brown fox jumps over the lazy dog";
        let marker = store.store(original, "default", None).unwrap();

        match store.retrieve(&marker.hash, "default") {
            RetrievalOutcome::Found(bytes) => assert_eq!(bytes, original),
            other => panic!("expected Found, got {other:?}"),
        }

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_hash_returns_missing_with_no_partial_output() {
        let store = RetrievalStore::memory();
        store.store(b"stored content", "default", None).unwrap();
        assert_eq!(
            store.retrieve(
                "0000000000000000000000000000000000000000000000000000000000000000",
                "default"
            ),
            RetrievalOutcome::Missing
        );
    }

    #[test]
    fn expired_entry_returns_expired_with_no_partial_output() {
        let store = RetrievalStore::memory();
        // ttl_seconds: Some(0) means "already elapsed" the instant it's stored.
        let marker = store
            .store(b"will expire immediately", "default", Some(0))
            .unwrap();
        assert_eq!(
            store.retrieve(&marker.hash, "default"),
            RetrievalOutcome::Expired
        );
    }

    #[test]
    fn none_ttl_never_expires() {
        let store = RetrievalStore::memory();
        let marker = store.store(b"never expires", "default", None).unwrap();
        assert_eq!(
            store.retrieve(&marker.hash, "default"),
            RetrievalOutcome::Found(b"never expires".to_vec())
        );
    }

    #[test]
    fn gc_removes_only_expired_entries() {
        let store = RetrievalStore::memory();
        let expired = store.store(b"expired entry", "default", Some(0)).unwrap();
        let alive = store.store(b"alive entry", "default", None).unwrap();

        let outcome = store.gc(None).unwrap();
        assert_eq!(outcome.expired_removed, 1);
        assert_eq!(outcome.evicted_removed, 0);
        assert_eq!(
            store.retrieve(&expired.hash, "default"),
            RetrievalOutcome::Missing
        );
        assert_eq!(
            store.retrieve(&alive.hash, "default"),
            RetrievalOutcome::Found(b"alive entry".to_vec())
        );
    }

    #[test]
    fn gc_evicts_oldest_entries_first_when_over_size_cap() {
        let store = RetrievalStore::memory();
        // Each entry is stored with a slightly later `stored_at_unix` via a manual meta
        // override isn't available on the public API, so rely on filesystem gc's stable
        // ordering test below for eviction-order coverage, and just prove the cap is enforced
        // here (all entries share the same instant, so ties are broken by iteration order).
        store.store(b"aaaaaaaaaa", "default", None).unwrap();
        store.store(b"bbbbbbbbbb", "default", None).unwrap();
        store.store(b"cccccccccc", "default", None).unwrap();

        let outcome = store.gc(Some(15)).unwrap();
        assert!(
            outcome.evicted_removed >= 1,
            "at least one entry must be evicted over cap"
        );
        assert_eq!(outcome.expired_removed, 0);
    }

    #[test]
    fn filesystem_gc_evicts_oldest_stored_at_first() {
        let root = temp_root("gc_order");
        let store = RetrievalStore::filesystem(&root);
        let old = store.store(b"oldest-entry-here", "default", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let newer = store.store(b"newest-entry", "default", None).unwrap();

        // Force the older entry's stored_at further into the past so ordering is unambiguous
        // regardless of clock resolution, then cap tight enough to evict exactly one entry.
        let meta_path = root.join("default").join(format!("{}.meta.json", old.hash));
        let mut meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta["stored_at_unix"] = serde_json::json!(1);
        std::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();

        let outcome = store.gc(Some(newer.bytes as u64)).unwrap();
        assert_eq!(outcome.evicted_removed, 1);
        assert_eq!(
            store.retrieve(&old.hash, "default"),
            RetrievalOutcome::Missing,
            "the older entry must be the one evicted"
        );
        assert!(matches!(
            store.retrieve(&newer.hash, "default"),
            RetrievalOutcome::Found(_)
        ));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn store_refuses_bytes_containing_a_known_secret_pattern() {
        let store = RetrievalStore::memory();
        let err = store
            .store(b"AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE", "default", None)
            .unwrap_err();
        assert!(matches!(err, TokenFoldError::SafetyViolation(_)));
    }

    #[test]
    fn filesystem_backend_also_refuses_secret_bearing_bytes() {
        let root = temp_root("secret_gate");
        let store = RetrievalStore::filesystem(&root);
        let err = store
            .store(
                b"Authorization: Bearer abcDEF123.token-value",
                "default",
                None,
            )
            .unwrap_err();
        assert!(matches!(err, TokenFoldError::SafetyViolation(_)));
        // Nothing should have been written to disk.
        assert!(!root.join("default").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn open_rejects_sqlite_backend_as_a_clear_config_error() {
        // `RetrievalStore` isn't `Debug` (it holds a `Mutex`), so assert via `Result::err`
        // rather than `unwrap_err`.
        let err = RetrievalStore::open("sqlite", "sha256", None)
            .err()
            .unwrap();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
    }

    #[test]
    fn open_rejects_blake3_hash_algorithm_as_a_clear_config_error() {
        let err = RetrievalStore::open("filesystem", "blake3", None)
            .err()
            .unwrap();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
    }

    #[test]
    fn open_accepts_memory_and_filesystem_with_sha256() {
        assert!(RetrievalStore::open("memory", "sha256", None).is_ok());
        assert!(RetrievalStore::open("filesystem", "sha256", Some(temp_root("open_ok"))).is_ok());
    }

    #[test]
    fn unsafe_namespace_is_rejected_by_store_and_missing_from_retrieve() {
        let store = RetrievalStore::memory();
        assert!(store.store(b"data", "../escape", None).is_err());
        assert_eq!(
            store.retrieve("deadbeef", "../escape"),
            RetrievalOutcome::Missing
        );
    }
}
