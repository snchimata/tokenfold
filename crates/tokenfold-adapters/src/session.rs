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
//! only ever used as an opaque key: never logged verbatim, never persisted to disk,
//! and an unknown, expired, or absent id yields `Untrusted`, which means "no
//! stability claim and no idempotence". The ledger is in-memory by design, so a
//! proxy restart drops every commitment and degrades to stateless behaviour rather
//! than to a false claim.
//!
//! Nothing here is a durable store. EP-05 owns durable references; this is the
//! per-process half of the contract that the stateless proxy needs before it may
//! claim anything at all.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// FNV-1a 64. A stable, dependency-free content fingerprint.
///
/// Only ever compared within one session's own committed set, where a collision
/// would at worst skip re-folding a result this proxy also emitted -- the safe
/// direction, since the alternative is folding a frame inside a frame.
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
    /// session id -> the digests this session has already been sent.
    committed: HashMap<String, HashSet<u64>>,
    /// session id -> when it was last written, for the TTL sweep.
    last_seen: HashMap<String, Instant>,
}

/// Remembers which observation bodies each session has already been sent.
#[derive(Debug)]
pub struct SessionLedger {
    ttl: Duration,
    max_sessions: usize,
    state: Mutex<State>,
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
        }
    }

    /// The trust level for `session_id`, sweeping expired sessions first.
    pub fn trust(&self, session_id: Option<&str>, now: Instant) -> SessionTrust {
        let Some(id) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
            return SessionTrust::Untrusted;
        };
        let mut state = self.state.lock().expect("session ledger mutex poisoned");
        self.sweep(&mut state, now);
        if state.committed.contains_key(id) {
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
        let Some(id) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
            return false;
        };
        let mut state = self.state.lock().expect("session ledger mutex poisoned");
        self.sweep(&mut state, now);
        state
            .committed
            .get(id)
            .is_some_and(|set| set.contains(&digest(content)))
    }

    /// Record the observation bodies this session has just been sent.
    ///
    /// Call this only with content the proxy itself emitted and verified. A session
    /// that has never been seen is created here; that is what makes the *next*
    /// request `Trusted`.
    pub fn commit(&self, session_id: Option<&str>, emitted: &[String], now: Instant) {
        let Some(id) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
            return;
        };
        let mut state = self.state.lock().expect("session ledger mutex poisoned");
        self.sweep(&mut state, now);
        if state.committed.len() >= self.max_sessions && !state.committed.contains_key(id) {
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
        let entry = state.committed.entry(id.to_string()).or_default();
        for content in emitted {
            entry.insert(digest(content));
        }
        state.last_seen.insert(id.to_string(), now);
    }

    /// Drops sessions idle for longer than the TTL, and drops a session's contents
    /// once it goes, so an expired id can never come back trusted.
    fn sweep(&self, state: &mut State, now: Instant) {
        let ttl = self.ttl;
        let expired: Vec<String> = state
            .last_seen
            .iter()
            .filter(|(_, at)| now.duration_since(**at) > ttl)
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

#[cfg(test)]
mod tests {
    use super::*;

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
