//! Reversible evidence store and retrieval: content-addressed storage of pre-transform
//! originals, plus the `[retrieval]` `tokenfold.toml` schema block that configures it.
//!
//! Whole-payload storage is optional; recoverable JSON pruning additionally stores each
//! omitted item through `store_batch()` before emitting an inline `$tf_ref` marker.
//! Batches fail closed on ordinary I/O errors. Filesystem operations share an OS file lock;
//! process termination can leave staged/orphan entries, never a successfully returned partial
//! payload. Original bytes containing detected secrets are rejected at the store boundary.
//!
//! Hash algorithm is SHA-256 only in this pass; `blake3` is a documented, rejected scope cut
//! (see [`RetrievalStore::open`]). Backends are `memory` (in-process, used in tests) and
//! `filesystem` (the default persistent backend); `sqlite` is likewise a documented, rejected
//! scope cut.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::errors::TokenFoldError;
use crate::transforms::redaction;

/// `tokenfold.toml`'s documented `[retrieval].ttl_seconds` default (7 days).
pub const DEFAULT_TTL_SECONDS: u64 = 604_800;

/// One retrieval marker's worth of metadata, in the documented marker grammar:
/// `[tokenfold:retrieve hash=<hex> alg=sha256 namespace=<ns> bytes=<n> ttl=<seconds>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalMarker {
    pub hash: String,
    pub alg: &'static str,
    pub namespace: String,
    pub bytes: usize,
    pub ttl_seconds: Option<u64>,
}

/// A validated reference to one stored original. Accepted inputs are a raw SHA-256 hash,
/// the legacy `[tokenfold:retrieve ...]` marker, or the JSON `{"$tf_ref": ...}` marker
/// emitted by lossy JSON pruning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalReference {
    pub hash: String,
    pub namespace: Option<String>,
}

pub fn parse_retrieval_reference(reference: &str) -> Result<RetrievalReference, TokenFoldError> {
    let reference = reference.trim();
    if reference.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(reference).map_err(|error| {
            TokenFoldError::InvalidInput(format!("invalid retrieval JSON marker: {error}"))
        })?;
        let marker = value.get("$tf_ref").unwrap_or(&value);
        let hash = marker
            .get("hash")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                TokenFoldError::InvalidInput(
                    "retrieval JSON marker has no string hash field".into(),
                )
            })?;
        let alg = marker
            .get("alg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("sha256");
        let namespace = marker
            .get("namespace")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        return validate_reference(hash, alg, namespace);
    }
    if reference.contains("tokenfold:retrieve") {
        let hash = extract_marker_field(reference, "hash").ok_or_else(|| {
            TokenFoldError::InvalidInput("retrieval marker has no hash=<hex> field".into())
        })?;
        let alg = extract_marker_field(reference, "alg").unwrap_or_else(|| "sha256".into());
        return validate_reference(&hash, &alg, extract_marker_field(reference, "namespace"));
    }
    validate_reference(reference, "sha256", None)
}

fn validate_reference(
    hash: &str,
    alg: &str,
    namespace: Option<String>,
) -> Result<RetrievalReference, TokenFoldError> {
    if alg != "sha256" {
        return Err(TokenFoldError::InvalidInput(format!(
            "unsupported retrieval hash algorithm {alg:?}; expected \"sha256\""
        )));
    }
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(TokenFoldError::InvalidInput(format!(
            "{hash:?} is not a valid SHA-256 hex hash"
        )));
    }
    if namespace
        .as_deref()
        .is_some_and(|value| !is_safe_path_component(value))
    {
        return Err(TokenFoldError::InvalidInput(format!(
            "invalid retrieval namespace: {:?}",
            namespace.as_deref().unwrap_or_default()
        )));
    }
    Ok(RetrievalReference {
        hash: hash.to_ascii_lowercase(),
        namespace,
    })
}

fn extract_marker_field(marker: &str, field: &str) -> Option<String> {
    let needle = format!("{field}=");
    let start = marker.find(&needle)? + needle.len();
    let rest = &marker[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ']')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Result of a retrieval lookup. Deliberately has no "partial" variant: a caller either gets
/// the exact original bytes back, or an explicit reason it did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrievalOutcome {
    Found(Vec<u8>),
    Missing,
    Expired,
    /// The caller asked for a namespace it is not authorized to read.
    ///
    /// Reported instead of `Missing` so a host can tell "I may not look here" from "it is not
    /// there". It never reveals whether the hash exists: an unauthorized read must not become an
    /// existence oracle for another namespace.
    Unauthorized,
    /// The entry exists, but restoring it would exceed the caller's restored-context budget.
    ///
    /// The original is returned whole or not at all. A truncated JSON row handed back as if it
    /// were the original would silently corrupt the recovered context, so the budget is enforced
    /// by refusing, never by cutting.
    OverBudget {
        /// Size of the whole entry, which is what restoring it would cost.
        bytes: usize,
        /// The budget that was exceeded.
        limit_bytes: usize,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcOutcome {
    pub expired_removed: usize,
    pub evicted_removed: usize,
    /// Entries that were past their TTL but were kept because an unexpired lease still
    /// promised them (or because they are legacy/unversioned entries, which are always
    /// treated conservatively). Surfaced so a caller can tell "nothing was removable"
    /// from "something was protected".
    pub retained_protected: usize,
    /// Eviction candidates skipped because they were leased or legacy.
    pub eviction_skipped_protected: usize,
}

/// One outstanding promise that an entry stays retrievable until `retain_until_unix`,
/// regardless of the entry's own TTL.
///
/// A lease is what makes a reference durable: the process that created the entry may exit,
/// and its TTL may elapse, but a lease keeps the original recoverable for the holder. Leases
/// are released explicitly and independently of their promised expiry — a holder that no
/// longer needs the original gives it back before the clock runs out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// Opaque, caller-supplied holder identity (a session id, a request id). It is never
    /// logged and never interpreted; it only lets a holder release its own lease.
    pub holder: String,
    /// The retention floor. While `now < retain_until_unix`, this entry is not removable,
    /// even if `stored_at_unix + ttl_seconds` has already passed.
    pub retain_until_unix: u64,
}

/// The current retention-metadata schema version written by this build.
///
/// Version 0 is reserved for entries written before versioned metadata existed: they carry no
/// leases and no quota history, so this build treats them conservatively (see
/// [`EntryMeta::is_legacy`]) rather than guessing.
pub const META_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EntryMeta {
    /// Absent in entries written before versioned metadata; `#[serde(default)]` maps those
    /// to 0 so an old store directory loads without a migration step.
    #[serde(default)]
    version: u32,
    stored_at_unix: u64,
    ttl_seconds: Option<u64>,
    bytes: usize,
    /// Absent in pre-versioned entries, which is exactly why those are treated conservatively.
    #[serde(default)]
    leases: Vec<Lease>,
}

impl EntryMeta {
    /// True for entries written before versioned metadata existed.
    ///
    /// A legacy entry cannot be known to be unleased, because the field that records leases did
    /// not exist when it was written. Deleting one on quota pressure could break a reference
    /// this build has no record of, so legacy entries are only ever removed by their own TTL —
    /// and only after that TTL has actually elapsed.
    fn is_legacy(&self) -> bool {
        self.version < META_VERSION
    }

    /// True while an unexpired lease still promises this entry.
    fn has_active_lease(&self, now: u64) -> bool {
        self.leases
            .iter()
            .any(|lease| lease.retain_until_unix > now)
    }

    /// True when this entry must not be deleted: an unexpired lease promised it.
    fn is_leased(&self, now: u64) -> bool {
        self.has_active_lease(now)
    }
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
        self.store_batch(&[(bytes, namespace, ttl_seconds)])
            .map(|mut markers| markers.remove(0))
    }

    /// Stores a set as one publication unit, subject to an optional admission quota.
    ///
    /// Admission runs **before** publication: if admitting this batch would push live stored
    /// bytes past `max_store_bytes`, the whole batch is rejected with
    /// [`TokenFoldError::QuotaExceeded`] and nothing is written. Admission never evicts to make
    /// room — evicting here would destroy a reference some other holder still depends on, which
    /// is exactly the failure a quota is supposed to prevent. Running GC first, or raising the
    /// quota, is the caller's deliberate choice.
    ///
    /// Bytes for entries already present under the same hash are not counted twice: re-storing
    /// identical content is idempotent and must not be charged against the quota twice.
    pub fn store_batch_within(
        &self,
        entries: &[(&[u8], &str, Option<u64>)],
        max_store_bytes: Option<u64>,
    ) -> Result<Vec<RetrievalMarker>, TokenFoldError> {
        let prepared: Vec<_> = entries
            .iter()
            .map(|(bytes, namespace, ttl_seconds)| {
                validate_store_input(bytes, namespace)?;
                let hash = hex_sha256(bytes);
                let meta = EntryMeta {
                    version: META_VERSION,
                    stored_at_unix: now_unix(),
                    ttl_seconds: *ttl_seconds,
                    bytes: bytes.len(),
                    leases: Vec::new(),
                };
                let marker = RetrievalMarker {
                    hash,
                    alg: "sha256",
                    namespace: (*namespace).to_string(),
                    bytes: bytes.len(),
                    ttl_seconds: *ttl_seconds,
                };
                Ok((bytes.to_vec(), meta, marker))
            })
            .collect::<Result<_, TokenFoldError>>()?;

        // Admission-before-publication. Only content that is not already stored counts as new.
        if let Some(cap) = max_store_bytes {
            let existing = self.live_bytes()?;
            let increment = self.admitted_increment(&prepared)?;
            if existing.saturating_add(increment) > cap {
                return Err(TokenFoldError::QuotaExceeded {
                    limit_bytes: cap,
                    requested_bytes: existing.saturating_add(increment),
                });
            }
        }

        match self {
            RetrievalStore::Memory(map) => {
                let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
                for (bytes, meta, marker) in &prepared {
                    guard.insert(
                        (marker.namespace.clone(), marker.hash.clone()),
                        MemoryEntry {
                            bytes: bytes.clone(),
                            meta: meta.clone(),
                        },
                    );
                }
            }
            RetrievalStore::Filesystem { root } => {
                std::fs::create_dir_all(root)?;
                let _lock = lock_store(root)?;
                let mut created = Vec::new();
                for (bytes, meta, marker) in &prepared {
                    match store_filesystem_entry(root, bytes, meta, marker) {
                        Ok(true) => created.push((marker.namespace.clone(), marker.hash.clone())),
                        Ok(false) => {}
                        Err(error) => {
                            for (namespace, hash) in created {
                                let dir = root.join(namespace);
                                std::fs::remove_file(dir.join(format!("{hash}.meta.json"))).ok();
                                std::fs::remove_file(dir.join(format!("{hash}.bin"))).ok();
                            }
                            return Err(error);
                        }
                    }
                }
            }
        }

        Ok(prepared.into_iter().map(|(_, _, marker)| marker).collect())
    }

    /// The unlimited-quota form of [`RetrievalStore::store_batch_within`], kept byte-for-byte
    /// compatible with the pre-EP-05 signature and behavior.
    ///
    /// Stores a set as one publication unit. On failure, no newly-created entry remains.
    pub fn store_batch(
        &self,
        entries: &[(&[u8], &str, Option<u64>)],
    ) -> Result<Vec<RetrievalMarker>, TokenFoldError> {
        self.store_batch_within(entries, None)
    }

    /// Total bytes currently held by entries this store would still consider live.
    ///
    /// Expired-but-leased and legacy entries are counted: both are still occupying disk, and
    /// pretending otherwise would let a quota be met by silently over-writing protected data.
    fn live_bytes(&self) -> Result<u64, TokenFoldError> {
        Ok(match self {
            RetrievalStore::Memory(map) => {
                let guard = map.lock().unwrap_or_else(|e| e.into_inner());
                guard.values().map(|e| e.meta.bytes as u64).sum()
            }
            RetrievalStore::Filesystem { root } => {
                let mut total = 0u64;
                for (meta_path, meta) in read_all_metadata(root)? {
                    let _ = meta_path;
                    total = total.saturating_add(meta.bytes as u64);
                }
                total
            }
        })
    }

    /// Net new bytes `prepared` would add, ignoring content already present under the same key.
    fn admitted_increment(
        &self,
        prepared: &[(Vec<u8>, EntryMeta, RetrievalMarker)],
    ) -> Result<u64, TokenFoldError> {
        match self {
            RetrievalStore::Memory(map) => {
                let guard = map.lock().unwrap_or_else(|e| e.into_inner());
                let mut increment = 0u64;
                for (_, meta, marker) in prepared {
                    let key = (marker.namespace.clone(), marker.hash.clone());
                    if !guard.contains_key(&key) {
                        increment = increment.saturating_add(meta.bytes as u64);
                    }
                }
                Ok(increment)
            }
            RetrievalStore::Filesystem { root } => {
                let mut increment = 0u64;
                for (_, meta, marker) in prepared {
                    let meta_path = root
                        .join(&marker.namespace)
                        .join(format!("{}.meta.json", marker.hash));
                    let present = std::fs::read(&meta_path)
                        .ok()
                        .and_then(|raw| serde_json::from_slice::<EntryMeta>(&raw).ok())
                        .is_some();
                    if !present {
                        increment = increment.saturating_add(meta.bytes as u64);
                    }
                }
                Ok(increment)
            }
        }
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
                    // A lease is a promise that the original stays retrievable, so an entry that
                    // is past its TTL but still leased is served, not reported as gone. GC keeps
                    // the bytes for exactly this reason; refusing them here would make the
                    // promise unkeepable.
                    Some(entry) if is_expired(&entry.meta) && !entry.meta.is_leased(now_unix()) => {
                        RetrievalOutcome::Expired
                    }
                    Some(entry) => RetrievalOutcome::Found(entry.bytes.clone()),
                }
            }
            RetrievalStore::Filesystem { root } => {
                if !root.is_dir() {
                    return RetrievalOutcome::Missing;
                }
                let Ok(_lock) = lock_store(root) else {
                    return RetrievalOutcome::Missing;
                };
                let dir = root.join(namespace);
                let meta_path = dir.join(format!("{hash}.meta.json"));
                let data_path = dir.join(format!("{hash}.bin"));
                let Ok(meta_bytes) = std::fs::read(&meta_path) else {
                    return RetrievalOutcome::Missing;
                };
                let Ok(meta) = serde_json::from_slice::<EntryMeta>(&meta_bytes) else {
                    return RetrievalOutcome::Missing;
                };
                if is_expired(&meta) && !meta.is_leased(now_unix()) {
                    return RetrievalOutcome::Expired;
                }
                match std::fs::read(&data_path) {
                    Ok(bytes) => RetrievalOutcome::Found(bytes),
                    Err(_) => RetrievalOutcome::Missing,
                }
            }
        }
    }

    /// Promises that `hash` stays retrievable in `namespace` until `retain_for_seconds` from now,
    /// for `holder`.
    ///
    /// This is the durability primitive: the entry survives its own TTL, and survives the exit of
    /// whichever process stored it, because the promise lives in the entry's metadata rather than
    /// in a caller's memory. Acquiring a lease for an entry that does not exist is
    /// [`TokenFoldError::InvalidInput`] — a lease cannot conjure content, it can only protect
    /// content that is really there.
    ///
    /// Re-acquiring for the same holder and hash is idempotent and extends the existing promise
    /// to the later of the two expiry times, so a retrying holder never shortens its own window.
    pub fn acquire_lease(
        &self,
        hash: &str,
        namespace: &str,
        holder: &str,
        retain_for_seconds: u64,
    ) -> Result<Lease, TokenFoldError> {
        if !is_safe_path_component(hash) || !is_safe_path_component(namespace) {
            return Err(TokenFoldError::InvalidInput(format!(
                "invalid retrieval lease target: hash {hash:?} namespace {namespace:?}"
            )));
        }
        if holder.is_empty() {
            return Err(TokenFoldError::InvalidInput(
                "retrieval lease holder must not be empty".into(),
            ));
        }
        let lease = Lease {
            holder: holder.to_string(),
            retain_until_unix: now_unix().saturating_add(retain_for_seconds),
        };

        match self {
            RetrievalStore::Memory(map) => {
                let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
                let entry = guard
                    .get_mut(&(namespace.to_string(), hash.to_string()))
                    .ok_or_else(|| missing_lease_target(hash, namespace))?;
                merge_lease(&mut entry.meta, lease);
                Ok(entry
                    .meta
                    .leases
                    .iter()
                    .find(|held| held.holder == holder)
                    .cloned()
                    .expect("merge_lease always leaves this holder's lease present"))
            }
            RetrievalStore::Filesystem { root } => {
                let _lock = lock_store(root)?;
                let meta_path = root.join(namespace).join(format!("{hash}.meta.json"));
                let Ok(raw) = std::fs::read(&meta_path) else {
                    return Err(missing_lease_target(hash, namespace));
                };
                let mut meta: EntryMeta = serde_json::from_slice(&raw).map_err(|e| {
                    TokenFoldError::InternalError(format!(
                        "failed to decode retrieval metadata for lease: {e}"
                    ))
                })?;
                let effective = merge_lease(&mut meta, lease);
                write_metadata_atomically(&meta_path, &meta)?;
                Ok(effective)
            }
        }
    }

    /// Releases `holder`'s lease on `hash`, independently of whether the promise has expired.
    ///
    /// Releasing early is the normal way to give an entry back: the holder no longer needs the
    /// original, so it becomes subject to its own TTL and to GC again immediately. Returns
    /// `true` if a lease was actually removed, `false` if that holder held none — releasing is
    /// idempotent from the caller's point of view either way.
    pub fn release_lease(
        &self,
        hash: &str,
        namespace: &str,
        holder: &str,
    ) -> Result<bool, TokenFoldError> {
        if !is_safe_path_component(hash) || !is_safe_path_component(namespace) {
            return Ok(false);
        }

        match self {
            RetrievalStore::Memory(map) => {
                let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
                let Some(entry) = guard.get_mut(&(namespace.to_string(), hash.to_string())) else {
                    return Ok(false);
                };
                let before = entry.meta.leases.len();
                entry.meta.leases.retain(|lease| lease.holder != holder);
                Ok(entry.meta.leases.len() != before)
            }
            RetrievalStore::Filesystem { root } => {
                let _lock = lock_store(root)?;
                let meta_path = root.join(namespace).join(format!("{hash}.meta.json"));
                let Ok(raw) = std::fs::read(&meta_path) else {
                    return Ok(false);
                };
                let Ok(mut meta) = serde_json::from_slice::<EntryMeta>(&raw) else {
                    return Ok(false);
                };
                let before = meta.leases.len();
                meta.leases.retain(|lease| lease.holder != holder);
                if meta.leases.len() == before {
                    return Ok(false);
                }
                write_metadata_atomically(&meta_path, &meta)?;
                Ok(true)
            }
        }
    }

    /// [`RetrievalStore::retrieve`] under an explicit authorization and restored-context budget.
    ///
    /// This is the host-facing retrieval path, and it adds two refusals the plain `retrieve` does
    /// not have:
    ///
    /// * `authorized_namespaces` — when it is non-empty, only those namespaces may be read. An
    ///   unauthorized request returns [`RetrievalOutcome::Unauthorized`] without revealing
    ///   whether the hash exists, so a host cannot probe another namespace's contents.
    /// * `max_restore_bytes` — a bound on how much one retrieval may restore. Exceeding it returns
    ///   [`RetrievalOutcome::OverBudget`]; the entry is never truncated, because a partial JSON
    ///   row presented as the original is worse than no answer at all.
    ///
    /// An empty `authorized_namespaces` means unrestricted, preserving the pre-EP-05 behavior for
    /// every existing caller.
    pub fn retrieve_authorized(
        &self,
        hash: &str,
        namespace: &str,
        authorized_namespaces: &[String],
        max_restore_bytes: Option<usize>,
    ) -> RetrievalOutcome {
        if !authorized_namespaces.is_empty()
            && !authorized_namespaces
                .iter()
                .any(|allowed| allowed == namespace)
        {
            // Checked before the lookup on purpose: refusing first means an unauthorized caller
            // cannot distinguish "exists" from "does not exist" from the returned variant.
            return RetrievalOutcome::Unauthorized;
        }
        match self.retrieve(hash, namespace) {
            RetrievalOutcome::Found(bytes) => match max_restore_bytes {
                Some(limit) if bytes.len() > limit => RetrievalOutcome::OverBudget {
                    bytes: bytes.len(),
                    limit_bytes: limit,
                },
                _ => RetrievalOutcome::Found(bytes),
            },
            other => other,
        }
    }

    /// Deletes entries whose `ttl_seconds` has elapsed *and* that no unexpired lease still promises
    /// (entries stored with `ttl_seconds: None` never expire), then — if `max_store_bytes` is given
    /// and total remaining stored bytes still exceed it — evicts the oldest-`stored_at` entries first
    /// until under the cap.
    ///
    /// A leased entry survives both phases: a lease is a promise that outlives the process which
    /// made it, so a second process running GC cannot take away a reference an active session is
    /// still relying on. Legacy (unversioned) entries survive the eviction phase but not the
    /// expiry phase, because they cannot be shown to be unleased.
    pub fn gc(&self, max_store_bytes: Option<u64>) -> Result<GcOutcome, TokenFoldError> {
        let now = now_unix();
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
                    match guard.get(&key) {
                        // An expired entry a lease still promises is kept, and counted as such so
                        // "nothing was removable" is distinguishable from "nothing to remove".
                        Some(entry) if !is_removable(&entry.meta, now) => {
                            outcome.retained_protected += 1;
                        }
                        _ => {
                            guard.remove(&key);
                            outcome.expired_removed += 1;
                        }
                    }
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
                            // Skip protected entries without giving up: an older protected entry
                            // must not stop a younger evictable one from being reclaimed.
                            let Some(entry) = guard.get(&key) else {
                                continue;
                            };
                            if !is_evictable(&entry.meta, now) {
                                outcome.eviction_skipped_protected += 1;
                                continue;
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
                let _lock = lock_store(root)?;

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
                            // Expired but still promised: keep the bytes so the holder's reference
                            // survives, and say so in the outcome rather than deleting silently.
                            if !is_removable(&meta, now) {
                                outcome.retained_protected += 1;
                                live.push((meta_path, data_path, meta));
                                continue;
                            }
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
                            // Same rule as the memory backend: a protected entry is skipped, and
                            // the sweep continues to the next candidate rather than stopping.
                            if !is_evictable(&meta, now) {
                                outcome.eviction_skipped_protected += 1;
                                continue;
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

/// Reads every readable `*.meta.json` under `root`, as `(meta_path, meta)` pairs.
///
/// A missing or unreadable directory yields an empty list rather than an error: a store that has
/// never been written is empty, not broken. Unparseable individual metadata files are skipped
/// for the same reason GC skips them — a half-written or foreign file must not make the whole
/// store unreadable.
fn read_all_metadata(root: &Path) -> Result<Vec<(PathBuf, EntryMeta)>, TokenFoldError> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
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
            if name.strip_suffix(".meta.json").is_none() {
                continue;
            }
            let Ok(raw) = std::fs::read(&meta_path) else {
                continue;
            };
            if let Ok(meta) = serde_json::from_slice::<EntryMeta>(&raw) {
                out.push((meta_path, meta));
            }
        }
    }
    Ok(out)
}

fn validate_store_input(bytes: &[u8], namespace: &str) -> Result<(), TokenFoldError> {
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
    Ok(())
}

fn lock_store(root: &Path) -> Result<File, TokenFoldError> {
    std::fs::create_dir_all(root)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".tokenfold.lock"))?;
    file.lock()?;
    Ok(file)
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_sidecar(path: &Path, label: &str) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("entry");
    path.with_file_name(format!(".{name}.{}.{}.{label}", std::process::id(), id))
}

fn stage_file(path: &Path, bytes: &[u8]) -> Result<PathBuf, TokenFoldError> {
    let temp = unique_sidecar(path, "tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        std::fs::remove_file(&temp).ok();
        return Err(error.into());
    }
    Ok(temp)
}

fn publish_file(temp: &Path, destination: &Path) -> Result<(), TokenFoldError> {
    let backup = unique_sidecar(destination, "bak");
    let had_destination = destination.exists();
    if had_destination {
        std::fs::rename(destination, &backup)?;
    }
    if let Err(error) = std::fs::rename(temp, destination) {
        if had_destination {
            std::fs::rename(&backup, destination).ok();
        }
        std::fs::remove_file(temp).ok();
        return Err(error.into());
    }
    if had_destination {
        std::fs::remove_file(backup).ok();
    }
    Ok(())
}

fn store_filesystem_entry(
    root: &Path,
    bytes: &[u8],
    meta: &EntryMeta,
    marker: &RetrievalMarker,
) -> Result<bool, TokenFoldError> {
    let dir = root.join(&marker.namespace);
    std::fs::create_dir_all(&dir)?;
    let data_path = dir.join(format!("{}.bin", marker.hash));
    let meta_path = dir.join(format!("{}.meta.json", marker.hash));
    let existed = data_path.is_file() && meta_path.is_file();
    if !existed {
        std::fs::remove_file(&data_path).ok();
        std::fs::remove_file(&meta_path).ok();
    }
    let meta_json = serde_json::to_vec_pretty(meta).map_err(|e| {
        TokenFoldError::InternalError(format!("failed to encode retrieval metadata: {e}"))
    })?;
    let data_temp = stage_file(&data_path, bytes)?;
    let meta_temp = match stage_file(&meta_path, &meta_json) {
        Ok(path) => path,
        Err(error) => {
            std::fs::remove_file(data_temp).ok();
            return Err(error);
        }
    };
    if let Err(error) = publish_file(&data_temp, &data_path) {
        std::fs::remove_file(meta_temp).ok();
        return Err(error);
    }
    if let Err(error) = publish_file(&meta_temp, &meta_path) {
        if !existed {
            std::fs::remove_file(data_path).ok();
        }
        return Err(error);
    }
    Ok(!existed)
}

/// True when this entry's own TTL has elapsed.
///
/// Deliberately ignores leases: an expired-but-leased entry is still *retained* (see
/// [`is_removable`]), it just is no longer fresh. Callers decide between "expired" and
/// "retained because promised" explicitly rather than conflating the two here.
fn is_expired(meta: &EntryMeta) -> bool {
    match meta.ttl_seconds {
        None => false,
        Some(ttl) => now_unix().saturating_sub(meta.stored_at_unix) >= ttl,
    }
}

/// True when GC is allowed to delete this entry.
///
/// An entry is protected from expiry-driven deletion when an unexpired lease still promises it:
/// the lease is a floor under the TTL, not an alternative to it. The entry still reports as
/// `Expired` to a reader — it is genuinely past its TTL — but it is not thrown away.
fn is_removable(meta: &EntryMeta, now: u64) -> bool {
    is_expired(meta) && !meta.is_leased(now)
}

/// True when size-pressure eviction may delete this entry.
///
/// Protection is broader than for expiry: a leased entry is protected, and so is a legacy
/// (unversioned) entry, because a legacy entry predates the lease field and cannot be shown to be
/// unleased. Deleting one would break a reference this build has no record of.
fn is_evictable(meta: &EntryMeta, now: u64) -> bool {
    !meta.is_leased(now) && !meta.is_legacy()
}

/// Adds `lease` to `meta`, keeping the longer promise if the holder already has one, and returns
/// the lease that is actually in force afterwards.
///
/// Idempotent per (holder, hash): a retried acquire must never *shorten* a window the holder
/// already holds, because doing so would silently expire a reference the caller still believes is
/// safe. The returned value is therefore the effective promise, which may be longer than the one
/// requested — never shorter.
fn merge_lease(meta: &mut EntryMeta, lease: Lease) -> Lease {
    if let Some(existing) = meta
        .leases
        .iter_mut()
        .find(|held| held.holder == lease.holder)
    {
        existing.retain_until_unix = existing.retain_until_unix.max(lease.retain_until_unix);
        return existing.clone();
    }
    meta.leases.push(lease.clone());
    lease
}

fn missing_lease_target(hash: &str, namespace: &str) -> TokenFoldError {
    TokenFoldError::InvalidInput(format!(
        "cannot lease hash {hash} in namespace {namespace:?}: no such stored entry"
    ))
}

/// Rewrites a `*.meta.json` in place using the same stage-and-publish path as a store, so a lease
/// update is never observable as a half-written file by a concurrent reader or GC.
fn write_metadata_atomically(meta_path: &Path, meta: &EntryMeta) -> Result<(), TokenFoldError> {
    let json = serde_json::to_vec_pretty(meta)
        .map_err(|e| TokenFoldError::InternalError(format!("failed to encode metadata: {e}")))?;
    let temp = stage_file(meta_path, &json)?;
    if let Err(error) = publish_file(&temp, meta_path) {
        std::fs::remove_file(&temp).ok();
        return Err(error);
    }
    Ok(())
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
    fn filesystem_batch_failure_commits_no_new_entries() {
        let root = temp_root("batch_rollback");
        let store = RetrievalStore::filesystem(&root);
        let entries = [
            (b"safe first".as_slice(), "default", None),
            (
                b"Authorization: Bearer abcDEF123.token-value".as_slice(),
                "default",
                None,
            ),
        ];
        assert!(store.store_batch(&entries).is_err());
        assert_eq!(
            store.retrieve(&hex_sha256(b"safe first"), "default"),
            RetrievalOutcome::Missing
        );
        std::fs::remove_dir_all(&root).ok();
    }

    // --- EP-05 Part A: durable references, admission, and lifetime --------------
    //
    // The scenarios below are the EP-05 admission/GC matrix: an active reference outliving the
    // creating process, an exhausted quota, a second process running GC, and a session resuming
    // afterwards. Each asserts the *reference property* the plan asks for -- an unexpired promised
    // reference stays retrievable, or the original simply stayed inline.

    /// Stores `content` with its TTL already elapsed, returning the marker. Used to prove that
    /// only a lease -- not the TTL -- is what keeps a promised reference alive.
    fn store_expired_then_lease(root: &Path, content: &[u8], namespace: &str) -> RetrievalMarker {
        RetrievalStore::filesystem(root)
            .store(content, namespace, Some(0))
            .unwrap()
    }

    /// Rewrites an entry's metadata exactly as a pre-EP-05 build wrote it: no `version`, no
    /// `leases`.
    fn downgrade_to_legacy_metadata(
        root: &Path,
        hash: &str,
        ttl_seconds: Option<u64>,
        bytes: usize,
    ) {
        let meta_path = root.join("default").join(format!("{hash}.meta.json"));
        std::fs::write(
            &meta_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "stored_at_unix": now_unix(),
                "ttl_seconds": ttl_seconds,
                "bytes": bytes,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn an_unexpired_lease_survives_expiry_and_a_second_process_running_gc() {
        // The core durability property: process A stores and leases an entry, then goes away.
        // Process B opens the same store root and runs GC. The reference A was promised must
        // still be there, because the promise was written to disk, not kept in A's memory.
        let root = temp_root("lease_survives_gc");
        let content = b"promised original";
        let marker = store_expired_then_lease(&root, content, "default");

        // Process A: lease it, then drop the store handle entirely.
        {
            let process_a = RetrievalStore::filesystem(&root);
            process_a
                .acquire_lease(&marker.hash, "default", "session-a", 3600)
                .unwrap();
        }

        // Process B: an independent store over the same root, as a second process would have.
        let process_b = RetrievalStore::filesystem(&root);
        let outcome = process_b.gc(Some(0)).unwrap();
        assert_eq!(
            outcome.retained_protected, 1,
            "GC should report the promised entry as protected, not silently keep it"
        );
        assert_eq!(outcome.expired_removed, 0);

        // The original is byte-exact, and the session that was promised it can resume.
        assert_eq!(
            process_b.retrieve(&marker.hash, "default"),
            RetrievalOutcome::Found(content.to_vec()),
            "a promised reference must outlive the creating process and a foreign GC"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_released_lease_returns_the_entry_to_its_own_ttl() {
        // Releasing is independent of the promised expiry: the holder gives the entry back
        // early and it becomes collectable again immediately.
        let root = temp_root("lease_release");
        let marker = store_expired_then_lease(&root, b"released original", "default");

        let store = RetrievalStore::filesystem(&root);
        store
            .acquire_lease(&marker.hash, "default", "session-a", 3600)
            .unwrap();
        assert_eq!(store.gc(Some(0)).unwrap().expired_removed, 0);

        assert!(
            store
                .release_lease(&marker.hash, "default", "session-a")
                .unwrap()
        );
        // Releasing twice is not an error, it just reports that nothing was held.
        assert!(
            !store
                .release_lease(&marker.hash, "default", "session-a")
                .unwrap()
        );

        assert_eq!(store.gc(Some(0)).unwrap().expired_removed, 1);
        assert_eq!(
            store.retrieve(&marker.hash, "default"),
            RetrievalOutcome::Missing
        );
        std::fs::remove_dir_all(&root).ok();
    }
    #[test]
    fn filesystem_store_and_gc_do_not_expose_partial_entries() {
        let root = temp_root("store_gc_race");
        let store = std::sync::Arc::new(RetrievalStore::filesystem(&root));
        let writer = {
            let store = std::sync::Arc::clone(&store);
            std::thread::spawn(move || {
                for i in 0..50 {
                    let bytes = format!("entry-{i}-{}", "x".repeat(256));
                    let marker = store.store(bytes.as_bytes(), "default", None).unwrap();
                    assert_eq!(
                        store.retrieve(&marker.hash, "default"),
                        RetrievalOutcome::Found(bytes.into_bytes())
                    );
                }
            })
        };
        let collector = {
            let store = std::sync::Arc::clone(&store);
            std::thread::spawn(move || {
                for _ in 0..50 {
                    store.gc(None).unwrap();
                }
            })
        };
        writer.join().unwrap();
        collector.join().unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn one_holders_lease_does_not_survive_another_holders_release() {
        // Leases are per holder: session B giving its own lease back must not un-promise the
        // entry for session A.
        let root = temp_root("lease_per_holder");
        let content = b"two holders";
        let marker = store_expired_then_lease(&root, content, "default");
        let store = RetrievalStore::filesystem(&root);
        store
            .acquire_lease(&marker.hash, "default", "session-a", 3600)
            .unwrap();
        store
            .acquire_lease(&marker.hash, "default", "session-b", 3600)
            .unwrap();

        assert!(
            store
                .release_lease(&marker.hash, "default", "session-b")
                .unwrap()
        );
        assert_eq!(store.gc(Some(0)).unwrap().expired_removed, 0);
        assert_eq!(
            store.retrieve(&marker.hash, "default"),
            RetrievalOutcome::Found(content.to_vec())
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_expired_lease_no_longer_protects_the_entry() {
        // A lease is a floor under the TTL, not a waiver of it: once the promise itself elapses,
        // the entry is collectable again with no further action.
        let store = RetrievalStore::memory();
        let marker = store
            .store(b"promise already elapsed", "default", Some(0))
            .unwrap();
        store
            .acquire_lease(&marker.hash, "default", "session-a", 0)
            .unwrap();

        assert_eq!(store.gc(None).unwrap().expired_removed, 1);
        assert_eq!(
            store.retrieve(&marker.hash, "default"),
            RetrievalOutcome::Missing
        );
    }

    #[test]
    fn reacquiring_a_lease_never_shortens_an_existing_promise() {
        // A retrying holder must not be able to shrink its own window by re-acquiring with a
        // shorter retention, which would silently expire a reference it still believes is safe.
        let root = temp_root("lease_extend");
        let marker = store_expired_then_lease(&root, b"long promise", "default");
        let store = RetrievalStore::filesystem(&root);

        let long = store
            .acquire_lease(&marker.hash, "default", "session-a", 3600)
            .unwrap();
        let short = store
            .acquire_lease(&marker.hash, "default", "session-a", 1)
            .unwrap();
        assert!(
            short.retain_until_unix >= long.retain_until_unix,
            "re-acquiring shortened the promise from {} to {}",
            long.retain_until_unix,
            short.retain_until_unix
        );
        assert_eq!(store.gc(Some(0)).unwrap().expired_removed, 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn gc_never_evicts_a_leased_entry_but_still_reclaims_others() {
        // Protection must not become a blanket refusal: with an over-cap store, the unprotected
        // entries are still reclaimed while the promised one is kept and reported.
        let store = RetrievalStore::memory();
        let leased_content = b"leased payload";
        let leased = store.store(leased_content, "default", None).unwrap();
        store
            .acquire_lease(&leased.hash, "default", "session-a", 3600)
            .unwrap();
        store
            .store(b"unprotected payload one", "default", None)
            .unwrap();
        store
            .store(b"unprotected payload two", "default", None)
            .unwrap();

        // A cap of 1 byte forces maximum pressure.
        let outcome = store.gc(Some(1)).unwrap();
        assert_eq!(
            outcome.evicted_removed, 2,
            "both unprotected entries should be reclaimed"
        );
        assert_eq!(outcome.eviction_skipped_protected, 1);
        assert_eq!(
            store.retrieve(&leased.hash, "default"),
            RetrievalOutcome::Found(leased_content.to_vec())
        );
    }

    #[test]
    fn an_over_quota_write_is_rejected_without_evicting_protected_data() {
        // Admission-before-publication: the quota refuses the write, and refusing must not cost
        // anyone else's reference. The existing entry survives and nothing new lands.
        let root = temp_root("quota_reject");
        let store = RetrievalStore::filesystem(&root);
        let existing = b"already stored";
        store.store(existing, "default", None).unwrap();

        let cap = (existing.len() as u64) + 4;
        let err = store
            .store_batch_within(
                &[(b"a much larger payload".as_slice(), "default", None)],
                Some(cap),
            )
            .unwrap_err();
        match err {
            TokenFoldError::QuotaExceeded {
                limit_bytes,
                requested_bytes,
            } => {
                assert_eq!(limit_bytes, cap);
                assert!(requested_bytes > cap);
            }
            other => panic!("expected a quota rejection, got {other:?}"),
        }

        // The pre-existing entry is untouched, and the refused payload is absent.
        assert_eq!(
            store.retrieve(&hex_sha256(existing), "default"),
            RetrievalOutcome::Found(existing.to_vec())
        );
        assert_eq!(
            store.retrieve(&hex_sha256(b"a much larger payload"), "default"),
            RetrievalOutcome::Missing
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_quota_exhaustion_mid_batch_rejects_the_whole_batch() {
        // store_batch is one publication unit: a batch that does not fit must not land
        // partially, or the caller is left with an unknown half-published set of references.
        let store = RetrievalStore::memory();
        let entries = [
            (b"first entry bytes".as_slice(), "default", None),
            (b"second entry bytes".as_slice(), "default", None),
        ];
        assert!(matches!(
            store.store_batch_within(&entries, Some(10)),
            Err(TokenFoldError::QuotaExceeded { .. })
        ));
        assert_eq!(
            store.retrieve(&hex_sha256(b"first entry bytes"), "default"),
            RetrievalOutcome::Missing
        );
        assert_eq!(
            store.retrieve(&hex_sha256(b"second entry bytes"), "default"),
            RetrievalOutcome::Missing
        );
    }

    #[test]
    fn restoring_identical_content_is_not_charged_twice_against_the_quota() {
        // Storage is content-addressed and idempotent. Charging an already-present entry again
        // would make a legitimate re-store look like quota growth and spuriously reject writes.
        let store = RetrievalStore::memory();
        let content = b"idempotent content";
        let size = content.len() as u64;
        store.store(content, "default", None).unwrap();

        // Exactly enough room for one copy, not two.
        assert!(
            store
                .store_batch_within(&[(content, "default", None)], Some(size))
                .is_ok()
        );
        assert_eq!(
            store.retrieve(&hex_sha256(content), "default"),
            RetrievalOutcome::Found(content.to_vec())
        );
    }

    #[test]
    fn a_legacy_unversioned_entry_is_never_evicted_before_its_ttl() {
        // An entry written before versioned metadata existed carries no lease field, so it
        // cannot be shown to be unleased. Under size pressure it must be kept until its own TTL
        // actually elapses, rather than deleted on the assumption that nobody needs it.
        let root = temp_root("legacy_conservative");
        let content = b"written before versioned metadata";
        let marker = store_expired_then_lease(&root, content, "default");
        downgrade_to_legacy_metadata(&root, &marker.hash, None, content.len());

        let store = RetrievalStore::filesystem(&root);
        let outcome = store.gc(Some(0)).unwrap();
        assert_eq!(outcome.evicted_removed, 0);
        assert_eq!(outcome.eviction_skipped_protected, 1);
        assert_eq!(
            store.retrieve(&marker.hash, "default"),
            RetrievalOutcome::Found(content.to_vec()),
            "a legacy entry must survive size pressure until its own TTL elapses"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_legacy_entry_is_still_removed_once_its_own_ttl_elapses() {
        // "Conservative until expiry" is not "kept forever": once the TTL has genuinely passed,
        // a legacy entry is collectable like any other.
        let root = temp_root("legacy_expiry");
        let content = b"legacy and expired";
        let marker = store_expired_then_lease(&root, content, "default");
        downgrade_to_legacy_metadata(&root, &marker.hash, Some(0), content.len());

        let store = RetrievalStore::filesystem(&root);
        assert_eq!(store.gc(None).unwrap().expired_removed, 1);
        assert_eq!(
            store.retrieve(&marker.hash, "default"),
            RetrievalOutcome::Missing
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn memory_and_filesystem_agree_on_the_lease_lifetime_matrix() {
        // The same contract on both backends, so a caller cannot get different durability
        // depending on which one is configured.
        for store in [
            RetrievalStore::memory(),
            RetrievalStore::filesystem(temp_root("parity")),
        ] {
            let content = b"backend parity payload";
            let marker = store.store(content, "default", Some(0)).unwrap();
            store
                .acquire_lease(&marker.hash, "default", "holder", 3600)
                .unwrap();
            assert_eq!(store.gc(Some(0)).unwrap().expired_removed, 0);
            assert_eq!(
                store.retrieve(&marker.hash, "default"),
                RetrievalOutcome::Found(content.to_vec())
            );
            assert!(
                store
                    .release_lease(&marker.hash, "default", "holder")
                    .unwrap()
            );
            assert_eq!(store.gc(Some(0)).unwrap().expired_removed, 1);
            if let RetrievalStore::Filesystem { root } = store {
                std::fs::remove_dir_all(&root).ok();
            }
        }
    }

    #[test]
    fn leasing_an_entry_that_is_not_there_is_refused() {
        // A lease protects content; it cannot invent it. Silently "succeeding" here would let a
        // caller believe a reference is durable when nothing was ever stored.
        let store = RetrievalStore::memory();
        let err = store
            .acquire_lease(&hex_sha256(b"never stored"), "default", "session-a", 60)
            .unwrap_err();
        assert!(matches!(err, TokenFoldError::InvalidInput(_)));
    }

    #[test]
    fn an_unsafe_namespace_cannot_be_leased_or_traversed() {
        let store = RetrievalStore::memory();
        let marker = store.store(b"safe content", "default", None).unwrap();
        assert!(
            store
                .acquire_lease(&marker.hash, "../escape", "session-a", 60)
                .is_err()
        );
        assert!(
            !store
                .release_lease(&marker.hash, "../escape", "session-a")
                .unwrap()
        );
    }

    #[test]
    fn an_empty_lease_holder_is_refused() {
        // An anonymous lease could never be released by anyone, so it would pin an entry for its
        // whole promised window with no way to give it back.
        let store = RetrievalStore::memory();
        let marker = store.store(b"some content", "default", None).unwrap();
        assert!(
            store
                .acquire_lease(&marker.hash, "default", "", 60)
                .is_err()
        );
    }

    #[test]
    fn an_unauthorized_namespace_is_refused_without_revealing_existence() {
        // A host must not be able to use retrieval to probe another namespace. Both a hash that
        // exists and one that does not must produce the *same* answer, or the variant itself
        // becomes an existence oracle.
        let store = RetrievalStore::memory();
        let present = store
            .store(b"someone else's data", "other-tenant", None)
            .unwrap();
        let absent_hash = hex_sha256(b"never stored anywhere");
        let allowed = vec!["my-tenant".to_string()];

        let for_present = store.retrieve_authorized(&present.hash, "other-tenant", &allowed, None);
        let for_absent = store.retrieve_authorized(&absent_hash, "other-tenant", &allowed, None);

        assert_eq!(for_present, RetrievalOutcome::Unauthorized);
        assert_eq!(
            for_absent,
            RetrievalOutcome::Unauthorized,
            "an unauthorized lookup must not distinguish existing from absent"
        );
    }

    #[test]
    fn an_authorized_namespace_still_retrieves_normally() {
        // Authorization is an addition to the contract, not a replacement: an allowed namespace
        // keeps working exactly as before, including the missing/expired distinctions.
        let store = RetrievalStore::memory();
        let marker = store.store(b"my own data", "my-tenant", None).unwrap();
        let allowed = vec!["my-tenant".to_string()];

        assert_eq!(
            store.retrieve_authorized(&marker.hash, "my-tenant", &allowed, None),
            RetrievalOutcome::Found(b"my own data".to_vec())
        );
        assert_eq!(
            store.retrieve_authorized(&hex_sha256(b"nope"), "my-tenant", &allowed, None),
            RetrievalOutcome::Missing
        );
    }

    #[test]
    fn an_empty_authorization_list_preserves_the_pre_ep05_behavior() {
        // Backward compatibility: an unconfigured host must not suddenly start refusing
        // retrievals it used to be able to perform.
        let store = RetrievalStore::memory();
        let marker = store.store(b"unrestricted", "any-namespace", None).unwrap();
        assert_eq!(
            store.retrieve_authorized(&marker.hash, "any-namespace", &[], None),
            RetrievalOutcome::Found(b"unrestricted".to_vec())
        );
    }

    #[test]
    fn an_oversized_restore_is_refused_and_never_truncated() {
        // Bounded retrieval returns the original whole or not at all. A truncated JSON row handed
        // back as the original would corrupt the recovered context silently.
        let store = RetrievalStore::memory();
        let original = b"{\"row\":1,\"payload\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}";
        let marker = store.store(original, "default", None).unwrap();

        let outcome = store.retrieve_authorized(&marker.hash, "default", &[], Some(10));
        assert_eq!(
            outcome,
            RetrievalOutcome::OverBudget {
                bytes: original.len(),
                limit_bytes: 10,
            },
            "an over-budget restore must refuse, not return a prefix"
        );

        // Exactly at the limit is allowed: the bound is inclusive.
        assert_eq!(
            store.retrieve_authorized(&marker.hash, "default", &[], Some(original.len())),
            RetrievalOutcome::Found(original.to_vec())
        );
    }

    #[test]
    fn a_budget_applies_after_authorization_not_before() {
        // Order matters for what leaks: an unauthorized caller must get `Unauthorized` even when
        // the entry would also have blown the budget, so the budget never discloses size to a
        // caller who has no right to read the entry.
        let store = RetrievalStore::memory();
        let marker = store
            .store(b"a fairly large private payload", "other", None)
            .unwrap();
        let allowed = vec!["mine".to_string()];
        assert_eq!(
            store.retrieve_authorized(&marker.hash, "other", &allowed, Some(1)),
            RetrievalOutcome::Unauthorized
        );
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

    #[test]
    fn parses_all_supported_retrieval_reference_forms() {
        let hash = "A".repeat(64);
        let raw = parse_retrieval_reference(&hash).unwrap();
        assert_eq!(raw.hash, "a".repeat(64));
        assert_eq!(raw.namespace, None);

        let legacy = parse_retrieval_reference(&format!(
            "[tokenfold:retrieve hash={hash} alg=sha256 namespace=project bytes=1]"
        ))
        .unwrap();
        assert_eq!(legacy.namespace.as_deref(), Some("project"));

        let json = parse_retrieval_reference(&format!(
            r#"{{"$tf_ref":{{"hash":"{hash}","alg":"sha256","namespace":"project"}}}}"#
        ))
        .unwrap();
        assert_eq!(json, legacy);
    }

    #[test]
    fn rejects_malformed_retrieval_references() {
        assert!(parse_retrieval_reference("deadbeef").is_err());
        assert!(
            parse_retrieval_reference(&format!(
                "[tokenfold:retrieve hash={} alg=blake3]",
                "a".repeat(64)
            ))
            .is_err()
        );
        assert!(
            parse_retrieval_reference(&format!(
                r#"{{"$tf_ref":{{"hash":"{}","namespace":"../escape"}}}}"#,
                "a".repeat(64)
            ))
            .is_err()
        );
    }
}
