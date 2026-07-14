//! Transform implementations. Each submodule exposes a pure function over bytes/text; the
//! pipeline (`crate::pipeline`) owns all bookkeeping (token counts, `TransformReport`,
//! safety validation/rollback). Canonical IDs and ordering live in `crate::modes`.

pub mod diff;
pub mod json;
pub mod json_fold;
pub mod logs;
pub mod redaction;
pub mod schema;

pub use crate::modes::TransformId;
