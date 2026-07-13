//! Deterministic Okapi BM25 retrieval over in-memory text chunks.
//!
//! This is the default, dependency-free retrieval path for tokenfold's RAG/vector extension
//! (see plan.md's RAG/vector scope note: "deterministic BM25/TF-IDF default, vector runtime
//! opt-in"). It never paraphrases or truncates chunk text: retrieved text is always
//! byte-identical to the original [`Chunk::text`] it was built from, which is required for
//! citation-grounding fidelity.

use std::collections::HashMap;

/// BM25 term-frequency saturation parameter.
const K1: f64 = 1.2;
/// BM25 document-length normalization parameter.
const B: f64 = 0.75;

/// A unit of retrievable text, identified by a caller-assigned id.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: String,
    pub text: String,
}

/// A chunk returned from [`Bm25Index::retrieve`], carrying its BM25 relevance score.
///
/// `text` is always byte-identical to the [`Chunk::text`] it was built from.
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub id: String,
    pub text: String,
    pub score: f64,
}

/// An in-memory Okapi BM25 index over a fixed corpus of [`Chunk`]s.
pub struct Bm25Index {
    /// Chunk ids in original corpus order; parallel to `term_freqs` and `doc_lengths`.
    doc_ids: Vec<String>,
    /// Per-document term frequency (term -> count within that document), one map per document.
    term_freqs: Vec<HashMap<String, usize>>,
    /// Document frequency: term -> number of documents containing that term at least once.
    doc_freqs: HashMap<String, usize>,
    /// Token-count length of each document, parallel to `doc_ids`.
    doc_lengths: Vec<usize>,
    /// Average document length (in tokens) across the corpus.
    avg_doc_length: f64,
    /// Original chunks keyed by id, for exact-text lookup at retrieval time (citation grounding).
    chunks_by_id: HashMap<String, Chunk>,
}

/// Lowercase and split on runs of non-alphanumeric characters. No regex needed.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

impl Bm25Index {
    /// Build a BM25 index over `chunks`, computing document frequencies, per-document term
    /// frequencies, document lengths, and the corpus average document length.
    pub fn build(chunks: &[Chunk]) -> Self {
        let mut doc_ids = Vec::with_capacity(chunks.len());
        let mut term_freqs = Vec::with_capacity(chunks.len());
        let mut doc_freqs: HashMap<String, usize> = HashMap::new();
        let mut doc_lengths = Vec::with_capacity(chunks.len());
        let mut chunks_by_id = HashMap::with_capacity(chunks.len());

        for chunk in chunks {
            let tokens = tokenize(&chunk.text);
            doc_lengths.push(tokens.len());

            let mut tf: HashMap<String, usize> = HashMap::new();
            for token in tokens {
                *tf.entry(token).or_insert(0) += 1;
            }
            for term in tf.keys() {
                *doc_freqs.entry(term.clone()).or_insert(0) += 1;
            }

            term_freqs.push(tf);
            doc_ids.push(chunk.id.clone());
            chunks_by_id.insert(chunk.id.clone(), chunk.clone());
        }

        let avg_doc_length = if doc_lengths.is_empty() {
            0.0
        } else {
            doc_lengths.iter().sum::<usize>() as f64 / doc_lengths.len() as f64
        };

        Self {
            doc_ids,
            term_freqs,
            doc_freqs,
            doc_lengths,
            avg_doc_length,
            chunks_by_id,
        }
    }

    /// Score every chunk against `query` using Okapi BM25, and return the top `top_k` by
    /// descending score. Returns an empty `Vec` (never panics) for an empty query or an empty
    /// index. Every returned chunk's `text` is byte-identical to the original `Chunk::text`.
    pub fn retrieve(&self, query: &str, top_k: usize) -> Vec<RetrievedChunk> {
        if top_k == 0 || self.doc_ids.is_empty() {
            return Vec::new();
        }

        let query_terms = tokenize(query);
        if query_terms.is_empty() {
            return Vec::new();
        }

        let doc_count = self.doc_ids.len() as f64;

        let mut scored: Vec<(usize, f64)> = (0..self.doc_ids.len())
            .map(|doc_idx| {
                let doc_len = self.doc_lengths[doc_idx] as f64;
                let tf_map = &self.term_freqs[doc_idx];

                let score = query_terms.iter().fold(0.0, |acc, term| {
                    let Some(&tf) = tf_map.get(term) else {
                        return acc;
                    };
                    let df = match self.doc_freqs.get(term) {
                        Some(&df) if df > 0 => df as f64,
                        _ => return acc,
                    };
                    let idf = ((doc_count - df + 0.5) / (df + 0.5) + 1.0).ln();
                    let tf = tf as f64;
                    let norm_len = if self.avg_doc_length > 0.0 {
                        doc_len / self.avg_doc_length
                    } else {
                        0.0
                    };
                    let denominator = tf + K1 * (1.0 - B + B * norm_len);
                    acc + idf * (tf * (K1 + 1.0)) / denominator
                });

                (doc_idx, score)
            })
            .collect();

        // Descending by score; ties broken by original corpus order for determinism.
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        scored
            .into_iter()
            .take(top_k)
            .map(|(doc_idx, score)| {
                let id = &self.doc_ids[doc_idx];
                let chunk = &self.chunks_by_id[id];
                RetrievedChunk {
                    id: chunk.id.clone(),
                    text: chunk.text.clone(),
                    score,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six chunks, each about a clearly distinct topic, using vocabulary that is
    /// characteristic of that topic and largely disjoint from the others.
    fn sample_chunks() -> Vec<Chunk> {
        vec![
            Chunk {
                id: "rust-ownership".to_string(),
                text: "Rust's borrow checker enforces ownership rules at compile time, \
                       ensuring every value has a single owner and preventing data races \
                       through exclusive mutable borrows and shared immutable borrows \
                       without a garbage collector."
                    .to_string(),
            },
            Chunk {
                id: "python-gil".to_string(),
                text: "The Python Global Interpreter Lock, or GIL, is a mutex that allows \
                       only one thread to execute Python bytecode at a time in CPython, \
                       which limits true parallelism for CPU-bound multithreaded programs."
                    .to_string(),
            },
            Chunk {
                id: "btree-index".to_string(),
                text: "A B-tree database index organizes keys into balanced multi-way nodes \
                       so that range queries and point lookups both complete in logarithmic \
                       time, and the tree stays balanced automatically as rows are inserted \
                       or deleted."
                    .to_string(),
            },
            Chunk {
                id: "tcp-congestion".to_string(),
                text: "TCP congestion control uses algorithms like slow start and congestion \
                       avoidance to adjust the sender's window size, backing off when packet \
                       loss signals network congestion and probing for more bandwidth \
                       otherwise."
                    .to_string(),
            },
            Chunk {
                id: "http-caching".to_string(),
                text: "HTTP caching headers such as Cache-Control, ETag, and Last-Modified \
                       let browsers and proxies reuse a previously fetched response instead \
                       of revalidating with the origin server on every request."
                    .to_string(),
            },
            Chunk {
                id: "json-serialization".to_string(),
                text: "JSON serialization converts in-memory objects into a lightweight text \
                       format using key-value pairs, arrays, strings, numbers, booleans, and \
                       null, making it a common interchange format between services."
                    .to_string(),
            },
        ]
    }

    #[test]
    fn retrieval_qa() {
        let chunks = sample_chunks();
        let index = Bm25Index::build(&chunks);

        let cases = [
            (
                "borrow checker ownership rules garbage collector",
                "rust-ownership",
            ),
            (
                "Global Interpreter Lock GIL CPython bytecode thread",
                "python-gil",
            ),
            (
                "congestion control slow start window size packet loss",
                "tcp-congestion",
            ),
            (
                "B-tree balanced nodes range queries logarithmic",
                "btree-index",
            ),
            (
                "Cache-Control ETag Last-Modified revalidating origin server",
                "http-caching",
            ),
            (
                "JSON key-value pairs arrays booleans interchange format",
                "json-serialization",
            ),
        ];

        for (query, expected_id) in cases {
            let results = index.retrieve(query, 1);
            assert_eq!(
                results.len(),
                1,
                "expected exactly one result for query {query:?}"
            );
            assert_eq!(
                results[0].id, expected_id,
                "query {query:?} should retrieve chunk {expected_id:?}, got {:?}",
                results[0].id
            );
        }
    }

    #[test]
    fn citation_grounding() {
        let chunks = sample_chunks();
        let index = Bm25Index::build(&chunks);
        let valid_ids: std::collections::HashSet<&str> =
            chunks.iter().map(|c| c.id.as_str()).collect();

        for chunk in &chunks {
            let results = index.retrieve(&chunk.text, 3);
            assert!(
                !results.is_empty(),
                "expected at least one result when querying with chunk {}'s own text",
                chunk.id
            );
            assert_eq!(
                results[0].text, chunk.text,
                "top result text must be byte-identical to source chunk {}",
                chunk.id
            );

            for result in &results {
                assert!(
                    valid_ids.contains(result.id.as_str()),
                    "retrieved id {:?} is not present in the original chunk list",
                    result.id
                );
                let original = chunks.iter().find(|c| c.id == result.id).unwrap();
                assert_eq!(
                    result.text, original.text,
                    "retrieved text for id {:?} must be byte-identical to the original chunk",
                    result.id
                );
            }
        }
    }

    #[test]
    fn empty_query_and_empty_index_do_not_panic() {
        let chunks = sample_chunks();
        let index = Bm25Index::build(&chunks);
        assert!(index.retrieve("", 5).is_empty());

        let empty_index = Bm25Index::build(&[]);
        assert!(empty_index.retrieve("rust ownership", 5).is_empty());
        assert!(empty_index.retrieve("", 5).is_empty());
    }
}
