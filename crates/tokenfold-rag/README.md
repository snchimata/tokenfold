# tokenfold-rag

Deterministic pure-Rust BM25 retrieval over caller-published evidence. No embedding model,
vector database or network client is required.

`Bm25Index` ranks caller-provided chunks. `EvidenceIndex::build` accepts an explicit list
of hashes and a namespace from `tokenfold-core::RetrievalStore`; it does not enumerate the
store. Search rechecks authorization, expiry and deletion before returning evidence.
Duplicate publication hashes are indexed once. Rebuild after changes; the index is not a
durable publication registry.

The Tokenfold MCP server exposes optional evidence queries through `tokenfold_retrieve`.
The host must publish a version-1 manifest and configure its path, store root and allowed
namespaces. Entry, index, query, result-count and total response-content budgets are bounded.
No partial row is represented as a complete original.

`vector::embed` is explicitly unavailable and returns an error. BM25 lexical matching is
not a claim of semantic recall or live task-quality qualification.
