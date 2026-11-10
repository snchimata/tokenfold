# Tokenfold Python SDK

Offline native request fitting. Python 3.12 or newer is required by the replacement
SDK. Experimental Select uses a separately approved, explicitly configured runtime;
base import and Core fitting never load models or download assets.

```python
from tokenfold import fit

result = fit("Keep this text unchanged.", target_tokens=32)
text = result.require_met()
```

Advanced compression, decoding, retrieval and historical compression report readers
are available under `tokenfold.lowlevel`. Fit receipts are a separate schema.

`fit` never sends a request. Text is opaque and unchanged; `Data(value)` renders
compact JSON. `Data(value, representation="folded")` explicitly opts into the
reversible Tokenfold codec: the consumer must understand that syntax (or decode
with `tokenfold.lowlevel.decode`). Typed JSON normalizes safe integral floats and
signed zero in Rust; default JSON preserves insertion order, while explicit folded
JSON may normalize per-object key order. Opaque text is byte-preserved.

Provider `Request.openai(...)` / `Request.anthropic(...)` payloads are full request
objects, not bare message lists. The default encoding counter cannot establish
provider framing and returns unavailable. Supply a complete-request counter, or
explicitly choose the historical `ProviderCounter.openai(model="gpt-4o-2024-08-06")`
local **estimate**. Estimates never become `met`: strict `require_met()` rejects
them; per-fit `budget_mode="best_effort"` permits `require_fit()` only when the
finite estimate fits. This offline recipe is not proof of billing or model availability.

A client owns independent fitting sessions and supports `with`, `async with` and
`await client.afit(...)`. Custom counters are synchronous and must support concurrent,
deterministic calls; async fitting runs them off-loop. Blocking application callbacks
cannot be forcibly preempted, so no bounded whole-call/close deadline is promised
across them. Cancellation waits for native inference cleanup before raising.

Selection is experimental and unqualified. It requires **both** an approved runtime
(`SelectRuntime(config_path, experimental=True)`) configured on `Tokenfold` and
`allow_selection=True` on that individual fit. Group annotations alone are not
consent. Required groups and the query are retained; callers must bundle semantic
dependencies themselves. `warmup()` only loads that configured runtime; import and
already-fitting Core requests never start it. Provisioning the optional runtime and
acquiring its pinned local model are separate explicit setup actions.

0.6.0 is unreleased; install a locally built wheel for current validation. The exact
`tokenfold[select]` extra adds only `tokenfold-select-runtime==0.6.0`, not ML packages.
Prebuilt targets: Linux GNU x64/arm64, macOS x64/arm64, Windows x64, gated by installed
artifact CI before release. See the [migration guide](https://github.com/snchimata/tokenfold/blob/main/docs/sdk-migration.md)
for provisioning, grouping recipes and queue/cold-start limits. The pinned tokenizer
may refuse long valid inputs with AccountingError (and its own panic diagnostic);
there is no heuristic/zero fallback.
