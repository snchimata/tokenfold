//! tokenfold-rag: deterministic retrieval for tokenfold's optional RAG/vector extension.
//!
//! The default retrieval path is deliberately a deterministic, pure-Rust Okapi BM25 index
//! ([`Bm25Index`]) — not an embedding/vector index — so retrieval stays reproducible and adds
//! no model or vector-database dependency. A vector runtime is opt-in and, in this pass,
//! unimplemented (see [`vector`]).

mod bm25;

pub use bm25::{Bm25Index, Chunk, RetrievedChunk};

// ponytail: vector runtime intentionally deferred; add a real embedding index (e.g. HNSW) only
// when a first consumer needs semantic (non-lexical) retrieval. Optional extensions like this
// one must not drag a vector database or model runtime into the default build.
pub mod vector {
    /// Embed text into a dense vector representation for semantic (non-lexical) retrieval.
    ///
    /// Always returns `Err`: the vector runtime is an optional, opt-in extension that is not
    /// implemented in this pass. [`crate::Bm25Index`] is the deterministic default retrieval
    /// path, and enabling a real embedding index would pull a model/vector-database runtime
    /// into a build that deliberately has none.
    pub fn embed(_text: &str) -> Result<Vec<f32>, String> {
        Err("vector retrieval is an optional runtime not implemented in this pass; BM25 is the deterministic default retrieval path, and enabling a real embedding index would pull a model/vector-database runtime into a build that deliberately has none".to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn vector_embed_returns_err() {
        assert!(crate::vector::embed("hello world").is_err());
    }
}
