//! EP-09 / NF-14: a rebuildable evidence-search index over approved retrieval-store entries.
//!
//! # Why the index is disposable
//!
//! This index holds **no state of its own**. It is derived entirely from entries already approved
//! in a [`RetrievalStore`], and can be thrown away and rebuilt at any moment with identical
//! results. That is what makes deletion and expiry synchronization tractable: there is no
//! persistent index to go stale, so "restart" and "resync" are the same operation.
//!
//! # The isolation properties that matter
//!
//! A search index over recoverable originals is a cross-session information-leak waiting to
//! happen, so the rules are enforced structurally rather than left to callers:
//!
//! * **Namespaced.** An index is built for exactly one namespace and can only return hits from
//!   that namespace. There is no API to widen it after construction.
//! * **Authorized.** An empty allowlist means unrestricted; a non-empty one restricts which
//!   namespaces may be searched, reusing the same rule as [`RetrievalStore::retrieve_authorized`].
//! * **Live-only.** Expired entries are never indexed and are filtered again at query time, so a
//!   slow rebuild cannot resurrect content whose TTL has since elapsed.
//! * **Exact restoration.** A hit names a stored hash; the caller retrieves the original through
//!   the store's own fail-closed path rather than trusting whatever text the index holds.

use std::collections::BTreeMap;

use tokenfold_core::retrieval_store::{RetrievalOutcome, RetrievalStore};

use crate::bm25::{Bm25Index, Chunk, RetrievedChunk};

/// One approved, retrievable entry the index knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedEntry {
    /// Content hash under which the original is stored. Retrieval goes through this.
    pub hash: String,
    /// The text actually indexed.
    pub text: String,
}

/// Why an entry was not indexed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexRejection {
    /// The entry's TTL had elapsed when the index was built.
    Expired { hash: String },
    /// The bytes were not valid UTF-8 text, so there is nothing to tokenize. The original is
    /// still retrievable by hash; it is simply not searchable.
    NotText { hash: String },
    /// The entry is too large to index within the configured bound.
    TooLarge { hash: String },
    /// The store refused to return the entry at all.
    Unavailable { hash: String },
}

/// The outcome of building an index, including what was left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexBuildReport {
    pub indexed: usize,
    pub rejected: Vec<IndexRejection>,
}

/// A hit, with the hash needed to restore the exact original.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub hash: String,
    pub text: String,
    pub score: f64,
}

/// A disposable BM25 index over one namespace's approved store entries.
pub struct EvidenceIndex {
    namespace: String,
    entries: BTreeMap<String, IndexedEntry>,
    bm25: Bm25Index,
}

impl EvidenceIndex {
    /// Builds an index over `hashes` as they currently exist in `store`, within `namespace`.
    ///
    /// Publication is explicit: only hashes the caller lists are considered, so an entry is never
    /// searchable merely because it happens to be in the store. That keeps retrieval consent
    /// separate from storage.
    pub fn build(
        store: &RetrievalStore,
        namespace: &str,
        hashes: &[String],
        max_entry_bytes: usize,
    ) -> (Self, IndexBuildReport) {
        let mut report = IndexBuildReport::default();
        let mut entries = BTreeMap::new();
        let mut chunks = Vec::new();

        for hash in hashes {
            // `retrieve` (not `retrieve_authorized`): the caller already decided this namespace is
            // in scope by naming it, and authorization is enforced once, at the query boundary.
            match store.retrieve(hash, namespace) {
                RetrievalOutcome::Found(bytes) => {
                    if bytes.len() > max_entry_bytes {
                        report
                            .rejected
                            .push(IndexRejection::TooLarge { hash: hash.clone() });
                        continue;
                    }
                    let Ok(text) = String::from_utf8(bytes) else {
                        report
                            .rejected
                            .push(IndexRejection::NotText { hash: hash.clone() });
                        continue;
                    };
                    report.indexed += 1;
                    chunks.push(Chunk {
                        id: hash.clone(),
                        text: text.clone(),
                    });
                    entries.insert(
                        hash.clone(),
                        IndexedEntry {
                            hash: hash.clone(),
                            text,
                        },
                    );
                }
                RetrievalOutcome::Expired => report
                    .rejected
                    .push(IndexRejection::Expired { hash: hash.clone() }),
                // `Missing`, `Unauthorized` and `OverBudget` all mean "not available to index".
                // The entry may still exist; it just cannot be published to search right now.
                _ => report
                    .rejected
                    .push(IndexRejection::Unavailable { hash: hash.clone() }),
            }
        }

        (
            Self {
                namespace: namespace.to_string(),
                entries,
                bm25: Bm25Index::build(&chunks),
            },
            report,
        )
    }

    /// The single namespace this index can ever answer from.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// How many entries are currently indexed.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Searches the index, dropping any hit the store can no longer serve.
    ///
    /// The live re-check is the reason expiry is honored twice: once when building and once per
    /// query. An index built while an entry was live therefore cannot return it after its TTL
    /// elapses, even if nothing rebuilt the index in between.
    ///
    /// `authorized_namespaces` follows the same convention as
    /// [`RetrievalStore::retrieve_authorized`]: empty means unrestricted.
    pub fn search(
        &self,
        store: &RetrievalStore,
        query: &str,
        top_k: usize,
        authorized_namespaces: &[String],
    ) -> Vec<Hit> {
        if !authorized_namespaces.is_empty()
            && !authorized_namespaces
                .iter()
                .any(|allowed| allowed == &self.namespace)
        {
            // Refused before searching, so a caller without access learns nothing -- not even
            // whether this index holds anything at all.
            return Vec::new();
        }
        self.bm25
            .retrieve(query, top_k)
            .into_iter()
            .filter_map(|RetrievedChunk { id, score, .. }| {
                // Re-verify against the store rather than trusting the indexed copy.
                match store.retrieve(&id, &self.namespace) {
                    RetrievalOutcome::Found(bytes) => Some(Hit {
                        hash: id,
                        // The live bytes, not the index's copy: retrieval must serve what is stored.
                        text: String::from_utf8_lossy(&bytes).into_owned(),
                        score,
                    }),
                    _ => None,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(entries: &[(&str, &str, Option<u64>)]) -> (RetrievalStore, Vec<String>) {
        let store = RetrievalStore::memory();
        let hashes = entries
            .iter()
            .map(|(bytes, ns, ttl)| store.store(bytes.as_bytes(), *ns, *ttl).unwrap().hash)
            .collect();
        (store, hashes)
    }

    /// A filesystem-backed store plus a unique root, so a test can delete an entry's bytes
    /// directly. Memory-backed storage is content-addressed and immutable, which makes an
    /// in-place deletion impossible to express.
    fn filesystem_store(tag: &str) -> (RetrievalStore, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "tokenfold_rag_evidence_{tag}_{}_{n}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        (RetrievalStore::filesystem(&root), root)
    }

    /// Removes one stored entry outright, simulating an expiry sweep or a deletion.
    fn delete_entry(root: &std::path::Path, namespace: &str, hash: &str) {
        let dir = root.join(namespace);
        std::fs::remove_file(dir.join(format!("{hash}.bin"))).ok();
        std::fs::remove_file(dir.join(format!("{hash}.meta.json"))).ok();
    }

    fn no_authorization() -> Vec<String> {
        Vec::new()
    }

    // --- publication ------------------------------------------------------------

    #[test]
    fn only_caller_listed_hashes_are_published() {
        // Storage is not consent: an entry in the store is not automatically searchable.
        let (store, hashes) = store_with(&[
            ("the approved build failure log for the parser", "ns", None),
            (
                "a private note that was never approved for search",
                "ns",
                None,
            ),
        ]);
        let approved = vec![hashes[0].clone()];

        let (index, report) = EvidenceIndex::build(&store, "ns", &approved, 4096);
        assert_eq!(index.len(), 1);
        assert_eq!(report.indexed, 1);
        assert!(report.rejected.is_empty());

        let hits = index.search(&store, "private", 10, &no_authorization());
        assert!(
            hits.iter().all(|hit| !hit.text.contains("private")),
            "an unapproved entry leaked into search: {hits:?}"
        );
    }

    #[test]
    fn a_hash_that_is_not_in_the_store_is_reported_not_silently_skipped() {
        let store = RetrievalStore::memory();
        let (index, report) = EvidenceIndex::build(&store, "ns", &["0".repeat(64)], 4096);
        assert!(index.is_empty());
        assert_eq!(report.indexed, 0);
        assert_eq!(report.rejected.len(), 1);
    }

    // --- expiry -----------------------------------------------------------------

    #[test]
    fn an_expired_entry_is_never_indexed() {
        let (store, hashes) = store_with(&[("already expired evidence", "ns", Some(0))]);
        let (index, report) = EvidenceIndex::build(&store, "ns", &hashes, 4096);

        assert_eq!(index.len(), 0);
        assert_eq!(
            report.rejected,
            vec![IndexRejection::Expired {
                hash: hashes[0].clone()
            }]
        );
        assert!(
            index
                .search(&store, "expired", 10, &no_authorization())
                .is_empty()
        );
    }

    #[test]
    fn an_entry_that_expires_after_the_index_was_built_stops_being_returned() {
        // The live re-check at query time is what makes expiry honored twice; without it, an index
        // built while an entry was live would keep serving it after its TTL elapsed.
        let (store, root) = filesystem_store("expiry_after_build");
        let marker = store
            .store(b"evidence that will outlive its ttl", "ns", None)
            .unwrap();

        let (index, _) = EvidenceIndex::build(&store, "ns", &[marker.hash.clone()], 4096);
        assert_eq!(
            index
                .search(&store, "outlive", 10, &no_authorization())
                .len(),
            1
        );

        // The entry goes away underneath the index, exactly as an expiry sweep would do it.
        delete_entry(&root, "ns", &marker.hash);
        assert!(
            index
                .search(&store, "outlive", 10, &no_authorization())
                .is_empty(),
            "an index kept serving an entry the store can no longer produce"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    // --- authorization ----------------------------------------------------------

    #[test]
    fn an_unauthorized_namespace_gets_no_hits_at_all() {
        let (store, hashes) = store_with(&[("tenant a confidential record", "a", None)]);
        let (index, _) = EvidenceIndex::build(&store, "a", &hashes, 4096);

        let allowed = vec!["b".to_string()];
        assert!(
            index
                .search(&store, "confidential", 10, &allowed)
                .is_empty()
        );
        // The same query IS served when authorized, proving the empty result is the refusal.
        assert_eq!(
            index
                .search(&store, "confidential", 10, &["a".to_string()])
                .len(),
            1
        );
    }

    #[test]
    fn an_empty_authorization_list_leaves_the_namespace_unrestricted() {
        let (store, hashes) = store_with(&[("an openly searchable record", "a", None)]);
        let (index, _) = EvidenceIndex::build(&store, "a", &hashes, 4096);
        assert_eq!(
            index
                .search(&store, "openly", 10, &no_authorization())
                .len(),
            1
        );
    }

    #[test]
    fn there_is_no_cross_namespace_leakage_by_construction() {
        // Two namespaces hold identical text; each index answers only for its own.
        let store = RetrievalStore::memory();
        // Distinct content per namespace so the two hashes differ: storage is content-addressed,
        // so identical bytes would deliberately share ONE hash and could not tell the two
        // namespaces apart.
        let a = store
            .store(b"tenant a keeps its own separate record", "a", None)
            .unwrap();
        let b = store
            .store(b"tenant b keeps its own separate record", "b", None)
            .unwrap();
        assert_ne!(a.hash, b.hash);

        let (index_a, _) = EvidenceIndex::build(&store, "a", &[a.hash.clone()], 4096);
        let (index_b, _) = EvidenceIndex::build(&store, "b", &[b.hash.clone()], 4096);

        assert_eq!(index_a.namespace(), "a");
        assert_eq!(index_b.namespace(), "b");

        let hits = index_a.search(&store, "separate record", 10, &no_authorization());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].hash, a.hash);
        // `b`'s hash is unreachable through `a`'s index, and vice versa.
        assert_ne!(hits[0].hash, b.hash);
        let hits_b = index_b.search(&store, "separate record", 10, &no_authorization());
        assert_eq!(hits_b.len(), 1);
        assert_eq!(hits_b[0].hash, b.hash);
    }

    #[test]
    fn an_index_built_over_one_namespace_cannot_return_another_namespaces_hash() {
        // Even when handed the other namespace's hash, the index only ever resolves within its
        // own -- so a caller cannot widen it by supplying extra hashes.
        let store = RetrievalStore::memory();
        let a = store
            .store(b"tenant a private material", "a", None)
            .unwrap();
        let b = store
            .store(b"tenant b private material", "b", None)
            .unwrap();

        // Deliberately hand `a`'s index a hash that lives in `b`.
        let (index_a, report) = EvidenceIndex::build(&store, "a", &[b.hash.clone()], 4096);
        assert_eq!(index_a.len(), 0);
        assert_eq!(report.rejected.len(), 1);
        assert!(
            index_a
                .search(&store, "private material", 10, &no_authorization())
                .is_empty()
        );
        let _ = a;
    }

    // --- rebuild / restart / deletion synchronization ---------------------------

    #[test]
    fn rebuilding_from_the_same_store_produces_the_same_results() {
        // The index is disposable, so "after a restart" and "rebuilt" are the same thing.
        let (store, hashes) = store_with(&[
            ("the parser failed on a malformed token stream", "ns", None),
            (
                "an unrelated note about documentation formatting",
                "ns",
                None,
            ),
        ]);
        let query = "parser malformed token";

        let (first, _) = EvidenceIndex::build(&store, "ns", &hashes, 4096);
        let (second, _) = EvidenceIndex::build(&store, "ns", &hashes, 4096);
        assert_eq!(
            first.search(&store, query, 5, &no_authorization()),
            second.search(&store, query, 5, &no_authorization())
        );
    }

    #[test]
    fn a_newly_published_entry_appears_after_a_rebuild() {
        let (store, mut hashes) = store_with(&[("the first published record", "ns", None)]);
        let (before, _) = EvidenceIndex::build(&store, "ns", &hashes, 4096);
        assert_eq!(
            before
                .search(&store, "published", 5, &no_authorization())
                .len(),
            1
        );

        hashes.push(
            store
                .store(b"a second published record about caching", "ns", None)
                .unwrap()
                .hash,
        );
        let (after, report) = EvidenceIndex::build(&store, "ns", &hashes, 4096);
        assert_eq!(report.indexed, 2);
        assert_eq!(after.len(), 2);
    }

    #[test]
    fn a_deleted_entry_disappears_from_a_rebuilt_index() {
        let (store, root) = filesystem_store("deletion_sync");
        let doomed = store
            .store(b"a record that will be deleted", "ns", None)
            .unwrap();
        let keeper = store.store(b"a record that stays put", "ns", None).unwrap();
        let hashes = vec![doomed.hash.clone(), keeper.hash.clone()];

        let (before, _) = EvidenceIndex::build(&store, "ns", &hashes, 4096);
        assert_eq!(before.len(), 2);

        delete_entry(&root, "ns", &doomed.hash);

        let (after, report) = EvidenceIndex::build(&store, "ns", &hashes, 4096);
        assert_eq!(after.len(), 1, "a deleted entry survived a rebuild");
        assert_eq!(report.indexed, 1);
        assert_eq!(report.rejected.len(), 1);
        assert!(
            index_excludes(&after, &store, &doomed.hash),
            "the deleted entry is still searchable after a rebuild"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    fn index_excludes(index: &EvidenceIndex, store: &RetrievalStore, hash: &str) -> bool {
        index
            .search(store, "record stays put deleted", 10, &no_authorization())
            .iter()
            .all(|hit| hit.hash != hash)
    }

    // --- bounds -----------------------------------------------------------------

    #[test]
    fn an_oversized_entry_is_rejected_rather_than_silently_truncated() {
        let big = "x".repeat(5000);
        let (store, hashes) = store_with(&[(big.as_str(), "ns", None)]);
        let (index, report) = EvidenceIndex::build(&store, "ns", &hashes, 1024);

        assert!(index.is_empty());
        assert_eq!(report.rejected.len(), 1);
        assert!(matches!(
            report.rejected[0],
            IndexRejection::TooLarge { .. }
        ));
    }

    #[test]
    fn non_utf8_bytes_are_reported_rather_than_lossily_indexed() {
        // The original stays retrievable by hash; it simply is not searchable text.
        let store = RetrievalStore::memory();
        let hash = store
            .store(&[0, 159, 146, 150, 255][..], "ns", None)
            .unwrap()
            .hash;
        let (index, report) = EvidenceIndex::build(&store, "ns", &[hash.clone()], 4096);

        assert!(index.is_empty());
        assert_eq!(report.rejected, vec![IndexRejection::NotText { hash }]);
    }

    #[test]
    fn an_empty_index_searches_to_nothing_without_panicking() {
        let store = RetrievalStore::memory();
        let (index, _) = EvidenceIndex::build(&store, "ns", &[], 4096);
        assert!(index.is_empty());
        assert!(
            index
                .search(&store, "anything", 10, &no_authorization())
                .is_empty()
        );
    }

    #[test]
    fn a_hit_serves_the_live_stored_bytes_under_its_hash() {
        // A hit is a pointer to an exact original, not a copy the index is trusted for.
        let original = "the exact original bytes a caller must be able to restore";
        let (store, hashes) = store_with(&[(original, "ns", None)]);
        let (index, _) = EvidenceIndex::build(&store, "ns", &hashes, 4096);
        let hits = index.search(&store, "exact original bytes", 5, &no_authorization());

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].hash, hashes[0]);
        assert_eq!(hits[0].text, original);
        assert_eq!(
            store.retrieve(&hits[0].hash, "ns"),
            RetrievalOutcome::Found(original.as_bytes().to_vec())
        );
    }

    #[test]
    fn building_is_deterministic() {
        let (store, hashes) = store_with(&[
            ("repeated build input one about caching", "ns", None),
            ("repeated build input two about parsing", "ns", None),
        ]);
        let first = EvidenceIndex::build(&store, "ns", &hashes, 4096).1;
        for _ in 0..4 {
            assert_eq!(EvidenceIndex::build(&store, "ns", &hashes, 4096).1, first);
        }
    }
}
