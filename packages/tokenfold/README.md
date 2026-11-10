# tokenfold for Node.js

Fit text, structured data or a complete provider request to an input token ceiling.
The primary SDK runs Rust in-process through Node-API; it never spawns a CLI,
downloads a model, sends a request or requires Python for Core fitting.

```sh
npm install tokenfold
```

```ts
import { fit, Tokenfold, ProviderCounter, type Context } from "tokenfold";

const context: Context = {
  kind: "context",
  query: "Which record answers the question?",
  groups: [
    { id: "instructions", content: "Preserve quoted evidence.", required: true },
    { id: "evidence", content: { kind: "json_data", value: { answer: 42 } } },
  ],
};
const result = await fit(context, { targetTokens: 800 });
const text: string = result.requireMet(); // throws if not exactly within the ceiling
console.log(result.receipt.accounting.counterId, result.tokensSaved);
```

Inputs are plain discriminated objects, including nested groups; no wrapper classes
or manual payload unwrapping are needed. `kind: "json_data"` renders compact JSON
(or explicitly reversible `representation: "folded"`). Provider inputs use
`kind: "openai_chat"` or `"anthropic_messages"` with a full `payload` object.
Text/Data/Context infer string results; provider inputs infer JSON-object results.
Inputs are snapshotted at fit admission. Results have defensive payload snapshots,
readonly camelCase receipt views, and canonical snake_case-only `receipt.toJSON()` /
`Receipt.fromJSON(value)` wire objects.

The default bundled `o200k_base` encoding is exact for rendered text, not provider
framing. `EncodingCounter("cl100k_base")` is also available. Custom counters have
`id` and `count({format,payload,scope})`, returning `{tokens,basis}` or a Promise;
`basis` is `exact`, `estimated` or `unavailable` (which requires `tokens: null`).
Counters must be deterministic and support concurrent calls. Exceptions become
sanitized `AccountingError`, never zero counts.

`ProviderCounter.openai({model: "gpt-4o-2024-08-06"})` is a versioned **local estimate**
for supported text chat, not billing evidence. Use per-fit `budgetMode: "best_effort"`
and `requireFit()` to accept an in-budget estimate. Its status stays `indeterminate`;
`requireMet()` still rejects it. Unsupported provider shapes report unavailable.
Reserve output tokens yourself before setting `targetTokens`; zero is valid.

For optional experimental selection, create a reusable client with explicit local
approval; annotation alone never permits dropping groups:

```ts
const client = new Tokenfold({
  select: { config: "/absolute/select-runtime.json", experimental: true },
  maxPendingScores: 32,
  scoreWaitTimeoutMs: 5000,
});
try {
  const result = await client.fit(context, {
    targetTokens: 800, allowSelection: true, signal: new AbortController().signal,
  });
  console.log(result.payload, result.groupsDropped);
} finally {
  await client.close();
}
```

Select requires separately provisioning the maintained `tokenfold-select-runtime`
Python worker and acquiring its pinned model; base Node installs do neither.
The scorer remains **unqualified**. Native scorer waiting does not occupy Core/libuv
workers; only the bounded scorer queue can raise `BusyError`. `warmup()` is explicit,
`close()` is idempotent and reaps owned workers, and later fits raise `ClosedError`.
An AbortSignal cancels running native work at its checkpoints, not just queued work.
Already-running application counters cannot be forcibly interrupted; cancellation
and close wait for those callbacks. Always close reusable clients explicitly.

Provider `groups: [{id,indices,required?}]` annotate contiguous whole-message ranges.
Record indices while assembling the final messages array; do not infer them from
message contents. Unlisted/protected messages remain required; system/developer,
latest-user and tool/multimodal safety constraints are checked natively. Initial
selection is text-only and cannot establish semantic correctness.

Node >=22 is supported with prebuilt addons for Linux GNU x64/arm64, macOS x64/arm64
and Windows x64. No browser/musl support is claimed. Install optional dependencies;
a missing or mismatched native addon raises `InstallationError` with no global-binary
fallback. The five `@tokenfold/native-*` packages are separate from CLI packages.
For a source checkout: `npm ci`, `npm run build:native`, then `npm test` (set
`TOKENFOLD_TEST_BINARY` to a built CLI for legacy CLI tests). Fake-worker tests use
`TOKENFOLD_TEST_PYTHON`; full installed-Python parity also uses
`TOKENFOLD_TEST_PARITY_PYTHON`. Neither is needed to use native Core.

## CLI utilities

The previous binary/run and compression utilities now live at `tokenfold/cli`. They retain their separate CLI packages and receipt family.

```sh
npm install tokenfold
```

```ts
import { compress, decode, inspect } from "tokenfold/cli";

const receipt = await inspect(input, { format: "json", preset: "balanced" });
const { payload, report, text } = await compress(input, {
  format: "json",
  preset: "balanced",
});
```

Payloads are `Uint8Array`; the `text` convenience getter decodes UTF-8 strictly.
`inspect` returns only the side-effect-free receipt.

`parseReport(jsonOrBytes)` reads archived v1 and current v2 receipts. It preserves the
source schema version, normalizes v1 `mode` to `preset`, and leaves unavailable sections
as `null`. Unknown versions and invalid required top-level fields raise
`TokenFoldProcessError` with `code: "invalid_report"`; this is not recursive JSON Schema
validation. `compress` and `inspect` use the same versioned reader.

Recoverable pruning is explicit and generic-JSON-only:

```ts
import { compress, retrieve } from "tokenfold/cli";

const result = await compress(feed, {
  format: "json",
  targetTokens: 2_000,
  pruning: {
    keepRatio: 0.35,
    preservePaths: ["meta"],
    retrievalStore: ".tokenfold/retrieve",
  },
});
const marker = JSON.parse(result.text).items.find((item) => item.$tf_ref);
const original = await retrieve(marker, { retrievalStore: ".tokenfold/retrieve" });
```

Explicit TOON output is verified before emission and restored with `decode`:

```ts
const encoded = await compress(input, { format: "json", encoding: "toon" });
const jsonBytes = await decode(encoded.payload, { from: "toon" });
```

Requires Node.js 22 or newer. The matching native CLI is installed through an
optional platform package; set `TOKENFOLD_BINARY_PATH` to use a custom binary.
