# tokenfold-adapters

Explicit provider-wire adapters for Tokenfold. No provider SDK or model runtime is required.

- `compress_for` and `verify_shape_parity`: OpenAI/Anthropic wire-shape checks.
- `observation`: opt-in OpenAI tool-result JSON compression with envelope validation,
  protected content, thresholds and no-growth fallback.
- `session`: in-process commitments or opt-in locked, bounded fingerprint snapshots
  for restart-safe observation idempotence; not durable history compaction.
- `dedup`: conservative duplicate encoding with public-decoder byte round-trip checks.
- `manifest::build_task_manifest`: caller-declared task/revision and required state;
  missing provenance, conflicting facts or exceeded limits reject the manifest.
- `tools::select_tools_for_query`: deterministic lexical ranking, preserved JSON values
  and key order, forced/companion tools and a caller-owned discovery tool.
- `select`: experimental text/ID mapping, approved direct-child scorer invocation with
  deadline/cancellation, strict score validation, whole-group selection and prompt recount.
  No model runtime is bundled and auxiliary model files remain caller-owned.
- `semantic`: experimental claim-backed literal extraction, **not** a verified generative
  summarizer. Its synchronous strategy interface cannot enforce a hard inference deadline.

Policies are disabled by default. Callers own task association, authorized catalogs,
discovery execution, prompt insertion and recovery. No host hooks are installed.
Framework names denote compatible wire shapes, not independently qualified SDK integrations.
JSON value preservation does not promise preservation of original JSON whitespace.

```rust
use tokenfold_adapters::{compress_for, AdapterFormat};
use tokenfold_core::CompressionPolicy;

let policy = CompressionPolicy::builder().build()?;
let result = compress_for(AdapterFormat::OpenAiChat,
    br#"{"messages":[{"role":"user","content":"Hello"}]}"#, &policy)?;
# Ok::<(), tokenfold_core::TokenFoldError>(())
```

Experimental helpers have not established live task-quality or cache non-inferiority.
Do not silently substitute them for default presets.
