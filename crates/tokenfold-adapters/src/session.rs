//! A trusted session ledger for append-only observation commits.
//!
//! # Why this exists
//!
//! The observation path is stateless: on every request it sees the host's whole
//! transcript and rewrites whatever tool results are still eligible. That is enough
//! to save tokens, but it is *not* enough to promise cross-turn prefix stability,
//! because the proxy cannot tell these three situations apart:
//!
//! 1. the host replayed the original result (safe to fold again), or
//! 2. the host committed the transformed observation and is replaying the proxy's
//!    own output back (folding that again would fold a frame inside a frame), or
//! 3. the host rewrote history on its own (out of contract entirely).
//!
//! A ledger that remembers which observation bodies *this session* already emitted
//! turns (1) and (2) apart, so the proxy can refuse to re-fold committed work and
//! can say, from evidence, that a later turn re-emitted byte-identical
//! observations. Situation (3) is still refused: the ledger only ever recognizes
//! content this proxy itself produced.
//!
//! # Trust boundary
//!
//! A session id is *supplied by the host* and is therefore untrusted input. It is
//! only ever used as an opaque key and never logged verbatim. By default commitments are
//! process-local. `open_persistent` opts into an exclusively locked, bounded snapshot of
//! SHA-256 session/content fingerprints: no raw transcript or session id is written.
//! Corrupt, future-dated or incompatible snapshots refuse startup. A failed snapshot write
//! clears trust and stops new commitments rather than claiming durability.
//! This is observation idempotence, not history compaction or provider cache qualification.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokenfold_core::retrieval_store::hex_sha256;

const MAX_COMMITMENTS: usize = 256;
const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;

/// FNV-1a 64. A stable, dependency-free content fingerprint.
///
/// Kept for API compatibility. Actual commitments now use full SHA-256 fingerprints.
pub fn digest(content: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Whether this request may be told anything about cross-turn stability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTrust {
    /// No usable session id, or the session is unknown or expired. The caller must not
    /// claim prefix stability and must not treat anything as committed.
    Untrusted,
    /// A live session this proxy has folded for before.
    Trusted,
}

#[derive(Debug, Default)]
struct State {
    /// SHA-256 session key -> the digests this session has already been sent.
    committed: HashMap<String, BTreeSet<String>>,
    /// SHA-256 session key -> when it was last written, for the TTL sweep.
    last_seen: HashMap<String, Instant>,
    persistence_failed: bool,
}

/// Remembers which observation bodies each session has already been sent.
#[derive(Debug)]
pub struct SessionLedger {
    ttl: Duration,
    max_sessions: usize,
    state: Mutex<State>,
    persistence: Option<Persistence>,
}

impl SessionLedger {
    /// A ledger that forgets a session after `ttl` and holds at most `max_sessions`.
    ///
    /// Both bounds are fail-closed: an unbounded map keyed by a host-supplied id is a
    /// memory-growth lever, and a commitment outliving its session would be a claim
    /// about a conversation this proxy no longer has.
    pub fn new(ttl: Duration, max_sessions: usize) -> Self {
        Self {
            ttl,
            max_sessions,
            state: Mutex::new(State::default()),
            persistence: None,
        }
    }

    /// Host opt-in persistence. One process owns the file; shared writers are refused.
    pub fn open_persistent(
        path: impl Into<PathBuf>,
        ttl: Duration,
        max_sessions: usize,
    ) -> Result<Self, String> {
        let persistence = Persistence::open(path.into())?;
        let state = persistence.load(ttl, max_sessions)?;
        Ok(Self {
            ttl,
            max_sessions,
            state: Mutex::new(state),
            persistence: Some(persistence),
        })
    }

    /// The trust level for `session_id`, sweeping expired sessions first.
    pub fn trust(&self, session_id: Option<&str>, now: Instant) -> SessionTrust {
        let Some(id) = session_key(session_id) else {
            return SessionTrust::Untrusted;
        };
        let mut state = self.state.lock().expect("session ledger mutex poisoned");
        self.sweep(&mut state, now);
        if state.committed.contains_key(&id) {
            SessionTrust::Trusted
        } else {
            SessionTrust::Untrusted
        }
    }

    /// True when this exact content was already emitted for this session.
    ///
    /// An unknown session is `false`: with no ledger entry there is nothing
    /// committed, which is the honest answer, not a claim of idempotence.
    pub fn is_committed(&self, session_id: Option<&str>, content: &str, now: Instant) -> bool {
        let Some(id) = session_key(session_id) else {
            return false;
        };
        let mut state = self.state.lock().expect("session ledger mutex poisoned");
        self.sweep(&mut state, now);
        state
            .committed
            .get(&id)
            .is_some_and(|set| set.contains(&hex_sha256(content.as_bytes())))
    }

    /// Record the observation bodies this session has just been sent.
    ///
    /// Call this only with content the proxy itself emitted and verified. A session
    /// that has never been seen is created here; that is what makes the *next*
    /// request `Trusted`.
    pub fn commit(&self, session_id: Option<&str>, emitted: &[String], now: Instant) {
        let Some(id) = session_key(session_id) else {
            return;
        };
        let mut state = self.state.lock().expect("session ledger mutex poisoned");
        self.sweep(&mut state, now);
        if state.persistence_failed || emitted.is_empty() || self.max_sessions == 0 {
            return;
        }
        if state.committed.len() >= self.max_sessions && !state.committed.contains_key(&id) {
            // Fail closed rather than evicting an arbitrary live session: forgetting one
            // costs a lost idempotence guarantee, which is recoverable, while trusting a
            // stale commitment is not.
            let oldest = state
                .last_seen
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                state.committed.remove(&oldest);
                state.last_seen.remove(&oldest);
            }
        }
        let entry = state.committed.entry(id.clone()).or_default();
        for content in emitted {
            if entry.len() >= MAX_COMMITMENTS {
                break;
            }
            entry.insert(hex_sha256(content.as_bytes()));
        }
        state.last_seen.insert(id, now);
        if let Some(persistence) = &self.persistence
            && persistence.save(&state, self.ttl, now).is_err()
        {
            state.committed.clear();
            state.last_seen.clear();
            state.persistence_failed = true;
        }
    }

    /// Drops sessions idle for longer than the TTL, and drops a session's contents
    /// once it goes, so an expired id can never come back trusted.
    fn sweep(&self, state: &mut State, now: Instant) {
        let ttl = self.ttl;
        let expired: Vec<String> = state
            .last_seen
            .iter()
            .filter(|(_, at)| now.saturating_duration_since(**at) > ttl)
            .map(|(id, _)| id.clone())
            .collect();
        for id in expired {
            state.committed.remove(&id);
            state.last_seen.remove(&id);
        }
    }

    /// The number of live sessions.
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .expect("session ledger mutex poisoned")
            .committed
            .len()
    }

    /// True when no session is live.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn session_key(id: Option<&str>) -> Option<String> {
    id.map(str::trim)
        .filter(|id| !id.is_empty() && id.len() <= 4096)
        .map(|id| hex_sha256(id.as_bytes()))
}

#[derive(Debug)]
struct Persistence {
    path: PathBuf,
    _lock: File,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema_version: u32,
    written_at_ms: u64,
    sessions: BTreeMap<String, PersistedSession>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedSession {
    committed_at_ms: u64,
    expires_at_ms: u64,
    digests: BTreeSet<String>,
}

fn unix_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .map_err(|e| e.to_string())
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Persistence {
    fn open(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let lock_path = path.with_extension("ledger-lock");
        if lock_path == path {
            return Err("snapshot path conflicts with lock path".into());
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|_| "observation ledger already owned by another process".to_string())?;
        Ok(Self { path, _lock: lock })
    }

    fn load(&self, ttl: Duration, max_sessions: usize) -> Result<State, String> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(e) => return Err(e.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_SNAPSHOT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
            return Err("observation snapshot too large".into());
        }
        let snapshot: Snapshot = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let wall = unix_ms()?;
        if snapshot.schema_version != 1
            || snapshot.written_at_ms > wall
            || snapshot.sessions.len() > max_sessions
        {
            return Err("incompatible, future-dated or over-capacity observation snapshot".into());
        }
        let now = Instant::now();
        let mut state = State::default();
        for (id, session) in snapshot.sessions {
            if !valid_hash(&id)
                || session.digests.is_empty()
                || session.digests.len() > MAX_COMMITMENTS
                || session.digests.iter().any(|hash| !valid_hash(hash))
            {
                return Err("invalid observation snapshot fingerprints".into());
            }
            if session.committed_at_ms > snapshot.written_at_ms
                || session.expires_at_ms < session.committed_at_ms
            {
                return Err("invalid observation snapshot lifetime".into());
            }
            if session.expires_at_ms <= wall {
                continue;
            }
            let remaining = Duration::from_millis(session.expires_at_ms - wall)
                .min(ttl.saturating_sub(Duration::from_millis(wall - session.committed_at_ms)));
            if remaining.is_zero() {
                continue;
            }
            let last = now
                .checked_sub(ttl.saturating_sub(remaining))
                .ok_or("invalid snapshot lifetime")?;
            state.last_seen.insert(id.clone(), last);
            state.committed.insert(id, session.digests);
        }
        Ok(state)
    }

    // ponytail: bounded full snapshots; use a journal only if measured commit I/O warrants it.
    fn save(&self, state: &State, ttl: Duration, now: Instant) -> Result<(), String> {
        let wall = unix_ms()?;
        let sessions = state
            .committed
            .iter()
            .map(|(id, digests)| {
                let remaining =
                    ttl.saturating_sub(now.saturating_duration_since(state.last_seen[id]));
                (
                    id.clone(),
                    PersistedSession {
                        committed_at_ms: wall.saturating_sub(
                            now.saturating_duration_since(state.last_seen[id])
                                .as_millis()
                                .min(u64::MAX as u128) as u64,
                        ),
                        expires_at_ms: wall
                            .saturating_add(remaining.as_millis().min(u64::MAX as u128) as u64),
                        digests: digests.clone(),
                    },
                )
            })
            .collect();
        let bytes = serde_json::to_vec(&Snapshot {
            schema_version: 1,
            written_at_ms: wall,
            sessions,
        })
        .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
            return Err("observation snapshot too large".into());
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temporary = self.path.with_extension(format!(
            "ledger-next-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        let written = file.write_all(&bytes).and_then(|_| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        let result = std::fs::rename(&temporary, &self.path).map_err(|e| e.to_string());
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_commitments_survive_restart_without_storing_raw_values() {
        let root = std::env::temp_dir().join(format!(
            "tf-ledger-{}-{}",
            std::process::id(),
            unix_ms().unwrap()
        ));
        let path = root.join("session.json");
        let ttl = Duration::from_secs(60);
        let ledger = SessionLedger::open_persistent(&path, ttl, 8).unwrap();
        ledger.commit(
            Some("private-host-session"),
            &["private-tool-body".into()],
            Instant::now(),
        );
        ledger.commit(
            Some("private-host-session"),
            &["second-body".into()],
            Instant::now(),
        );
        assert!(SessionLedger::open_persistent(&path, ttl, 8).is_err());
        let bytes = std::fs::read_to_string(&path).unwrap();
        assert!(!bytes.contains("private-host-session"));
        assert!(!bytes.contains("private-tool-body"));
        drop(ledger);
        let loaded = SessionLedger::open_persistent(&path, ttl, 8).unwrap();
        assert_eq!(
            loaded.trust(Some("private-host-session"), Instant::now()),
            SessionTrust::Trusted
        );
        assert!(loaded.is_committed(
            Some("private-host-session"),
            "private-tool-body",
            Instant::now()
        ));
        assert!(loaded.is_committed(Some("private-host-session"), "second-body", Instant::now()));
        assert!(!loaded.is_committed(Some("other-session"), "private-tool-body", Instant::now()));
        drop(loaded);
        let expired = SessionLedger::open_persistent(&path, Duration::ZERO, 8).unwrap();
        assert!(expired.is_empty());
        drop(expired);
        std::fs::write(&path, "corrupt").unwrap();
        assert!(SessionLedger::open_persistent(&path, ttl, 8).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persistence_failure_clears_trust_and_stops_new_commitments() {
        let root = std::env::temp_dir().join(format!(
            "tf-ledger-failure-{}-{}",
            std::process::id(),
            unix_ms().unwrap()
        ));
        let path = root.join("ledger.json");
        let ledger = SessionLedger::open_persistent(&path, Duration::from_secs(60), 8).unwrap();
        std::fs::create_dir(&path).unwrap();
        ledger.commit(Some("s"), &["x".into()], Instant::now());
        assert!(ledger.is_empty());
        assert_eq!(
            ledger.trust(Some("s"), Instant::now()),
            SessionTrust::Untrusted
        );
        std::fs::remove_dir(&path).unwrap();
        ledger.commit(Some("s"), &["y".into()], Instant::now());
        assert!(ledger.is_empty());
        drop(ledger);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zero_capacity_and_per_session_limit_are_enforced() {
        let ledger = SessionLedger::new(Duration::from_secs(60), 0);
        ledger.commit(Some("s"), &["x".into()], Instant::now());
        assert!(ledger.is_empty());
        let ledger = SessionLedger::new(Duration::from_secs(60), 1);
        let contents = (0..MAX_COMMITMENTS + 4)
            .map(|i| i.to_string())
            .collect::<Vec<_>>();
        ledger.commit(Some("s"), &contents, Instant::now());
        let state = ledger.state.lock().unwrap();
        assert_eq!(
            state.committed.values().next().unwrap().len(),
            MAX_COMMITMENTS
        );
    }

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn an_absent_or_blank_session_id_is_untrusted_and_commits_nothing() {
        let ledger = SessionLedger::new(Duration::from_secs(60), 8);
        for id in [None, Some(""), Some("   ")] {
            assert_eq!(ledger.trust(id, now()), SessionTrust::Untrusted);
            ledger.commit(id, &["x".to_string()], now());
            assert!(!ledger.is_committed(id, "x", now()));
        }
        assert!(ledger.is_empty(), "a blank id must not create a session");
    }

    #[test]
    fn committing_makes_the_next_request_trusted_and_idempotent() {
        let ledger = SessionLedger::new(Duration::from_secs(60), 8);
        let emitted = vec!["{\"__tf_cols__\":[\"a\"]}".to_string()];
        assert_eq!(ledger.trust(Some("s1"), now()), SessionTrust::Untrusted);
        ledger.commit(Some("s1"), &emitted, now());
        assert_eq!(ledger.trust(Some("s1"), now()), SessionTrust::Trusted);
        assert!(ledger.is_committed(Some("s1"), &emitted[0], now()));
        // A different result in the same session is still uncommitted.
        assert!(!ledger.is_committed(Some("s1"), "{\"a\":1}", now()));
        // And a different session never sees another session's commitments.
        assert!(!ledger.is_committed(Some("s2"), &emitted[0], now()));
    }

    #[test]
    fn an_expired_session_loses_both_its_trust_and_its_contents() {
        let ledger = SessionLedger::new(Duration::from_millis(1), 8);
        let start = now();
        ledger.commit(Some("s1"), &["committed".to_string()], start);
        let later = start + Duration::from_millis(50);
        assert_eq!(ledger.trust(Some("s1"), later), SessionTrust::Untrusted);
        assert!(!ledger.is_committed(Some("s1"), "committed", later));
        assert!(
            ledger.is_empty(),
            "an expired session must not linger in the map"
        );
    }

    #[test]
    fn sessions_are_bounded_so_a_host_supplied_id_cannot_grow_the_map_forever() {
        let ledger = SessionLedger::new(Duration::from_secs(600), 2);
        for index in 0..10 {
            ledger.commit(Some(&format!("s{index}")), &["x".to_string()], now());
            assert!(
                ledger.len() <= 2,
                "ledger grew past its bound: {}",
                ledger.len()
            );
        }
    }

    #[test]
    fn the_digest_is_stable_and_distinguishes_content() {
        assert_eq!(digest("abc"), digest("abc"));
        assert_ne!(digest("abc"), digest("abd"));
        assert_ne!(digest(""), digest(" "));
    }
}
