//! tokenfold-rag: deterministic retrieval for tokenfold's optional RAG/vector extension.
//!
//! Per plan.md's RAG/vector scope note, the default retrieval path is a deterministic,
//! pure-Rust Okapi BM25 index ([`Bm25Index`]) — not an embedding/vector index. A vector
//! runtime is opt-in and, in this pass, unimplemented (see [`vector`]).

mod bm25;

pub use bm25::{Bm25Index, Chunk, RetrievedChunk};

// ponytail: vector runtime intentionally deferred; add a real embedding index (e.g. HNSW) only
// when a first consumer needs semantic (non-lexical) retrieval, per roadmap.md D-014.
pub mod vector {
    /// Embed text into a dense vector representation for semantic (non-lexical) retrieval.
    ///
    /// Always returns `Err`: the vector runtime is an optional, opt-in extension that is not
    /// implemented in this pass. [`crate::Bm25Index`] is the deterministic default retrieval
    /// path (see roadmap.md D-014 and plan.md's RAG/vector scope note).
    pub fn embed(_text: &str) -> Result<Vec<f32>, String> {
        Err("vector retrieval is an optional runtime not implemented in this pass; BM25 is the deterministic default (see roadmap.md D-014 and plan.md's RAG/vector scope note)".to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn vector_embed_returns_err() {
        assert!(crate::vector::embed("hello world").is_err());
    }
}
