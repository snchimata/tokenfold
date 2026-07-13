//! `tokenfold-admin`: release-verification and update/rollback primitives.
//!
//! No live release/update server exists yet (D-007 signing-key management is unresolved per
//! `roadmap.md`) — this crate provides the real, tested verification/rollback primitives
//! operating against a local release-manifest file. A `tokenfold update` CLI command wires
//! these against a local manifest path. Fetching manifests over HTTPS from a real release
//! endpoint is deferred until D-007 resolves.

pub mod manifest;
pub mod update;
