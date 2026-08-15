//! `tokenfold-admin`: release-verification and update/rollback primitives.
//!
//! No live release/update server exists yet — the cross-compile/release-runner strategy and the
//! release signing-key management that goes with it are still undecided — so this crate provides
//! the real, tested verification/rollback primitives operating against a local release-manifest
//! file. They are library primitives only; no CLI subcommand wires them up yet. Fetching
//! manifests over HTTPS from a real release endpoint is deferred until that release-signing
//! strategy is settled.

pub mod manifest;
pub mod update;
