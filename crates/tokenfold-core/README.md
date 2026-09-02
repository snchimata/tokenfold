# tokenfold-core

Provider-neutral compression engine used by every Tokenfold binding and transport.

```rust
use tokenfold_core::{CompressionInput, Preset, CompressionPolicy, compress};

let input = CompressionInput::json(br#"{ "items": [1, 2, 3] }"#.to_vec());
let policy = CompressionPolicy::builder()
    .preset(Preset::Balanced)
    .build()?;
let output = compress(input, &policy)?;
# Ok::<(), tokenfold_core::TokenFoldError>(())
```

Run the complete example with `cargo run -p tokenfold-core --example quickstart`.

## Safety contract

- Secret redaction runs first and cannot be disabled through the normal transform list.
- System messages, the latest user message, and structural diff lines are protected where
  applicable; a transform that violates a protected segment is rolled back.
- Every transform is rejected if it increases estimated tokens or exceeds its preset ratio cap.
- Reports disclose estimator provenance and best-effort/unreachable budget outcomes.
- Lossy JSON pruning is opt-in, requires durable retrieval, and publishes a marker only after the
  removed value is stored. Do not send preview bytes to a model.

`CompressionReport` is the compatibility-sensitive receipt for these decisions. Its canonical
v2 fixture and JSON Schema live under `tests/fixtures/compression_report_v2*`.
