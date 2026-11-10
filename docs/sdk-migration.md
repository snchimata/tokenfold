# Migrating to the 0.6 SDK

**0.6.0 is unreleased.** Local build/install checks are not registry publication.
Build a Python wheel with `maturin build --release --locked -m crates/tokenfold-py/Cargo.toml`;
install that wheel into a fresh Python >=3.12 environment. Node requires >=22:
developers run `npm ci`, `npm run build:native`, `npm test` in `packages/tokenfold`.
Published users will consume prebuilt addons, with no install-time compiler or Python
requirement for Node Core. Targets are Linux GNU x64/arm64, macOS x64/arm64 and
Windows x64; hosted installed-artifact checks must pass before release. No browser
or Linux musl addon support is claimed (the separate CLI has static musl binaries).

## Fit, then check, then decide whether to send

```python
from tokenfold import Data, fit
result = fit(Data({"answer": 42}), target_tokens=32)
payload = result.require_met()  # str; raises rather than silently exceeding budget
print(result.receipt.to_json())
```

```js
import { fit } from 'tokenfold';
const result = await fit({kind: 'json_data', value: {answer: 42}}, {targetTokens: 32});
const payload = result.requireMet();
console.log(result.receipt.toJSON());
```

Nothing automatically sends requests, downloads models, writes storage or performs
recoverable pruning. The base is model-free and offline. Text is opaque: even text
that looks like JSON is not parsed. Data renders compact JSON preserving insertion
order by default; explicit `representation="folded"` / `representation: "folded"`
uses the reversible codec and may normalize per-object key order. Decode before a
consumer that expects the original JSON shape. Safe integral floats and signed zero
normalize lexically (`1.0` -> `1`, `-0.0` -> `0`); use opaque text for byte preservation.
Unsafe integers, nonfinite numbers, cycles, invalid Unicode and arbitrary objects
reject instead of being coerced. Some long valid inputs can exceed the pinned
tokenizer's limits: typed `AccountingError`, possibly its own panic diagnostic,
never heuristic or zero fallback.

| Before | 0.6 |
| --- | --- |
| `from tokenfold import compress, decode, inspect` | `from tokenfold.lowlevel import compress, decode, inspect` |
| `from tokenfold import Preset, InputFormat` | Same names under `tokenfold.lowlevel` |
| `import { compress, inspect, run } from 'tokenfold'` | Import from `tokenfold/cli` |
| Optional `tokenfold_select` v2 SDK | Explicit maintained `tokenfold-select-runtime`; no compatibility fallback |
| Compression report schema 2.0 | Retained for lowlevel/CLI; fit receipts are a separate versioned family |

Text/Data/Context infer `FitResult[str]` / `FitResult<string>`; full provider
Request inputs infer JSON-object results. Python uses dataclasses, Node structural
discriminated objects, not required wrapper classes. Payload snapshots and receipt
views cannot mutate a fit. Node views are camelCase; serialized receipts use
snake_case only. Signed savings can be negative, unavailable counts are null,
and scorer attempted/succeeded/selection-applied are distinct facts.

## Honest accounting

Reserve output tokens yourself, then pass the remaining nonnegative input ceiling.
Bundled `o200k_base` (or `cl100k_base`) is exact for rendered text, not provider
framing. Default provider counts are unavailable. A custom whole-request counter
must cover messages, tools, schemas and framing; its exactness is the application's
assertion, not independent billing proof. Counters have a stable nonsecret versioned
ID and deterministic concurrent `count` calls. Python callbacks are synchronous;
Node may return a Promise. Exceptions become sanitized accounting errors.

`ProviderCounter.openai(model="gpt-4o-2024-08-06")` (Node: `{model: ...}`) is an
explicit historical local-text estimate, not a claim that this model is still served.
The model must match; unsupported fields, tools, non-string content and other models
return unavailable. Anthropic needs your complete-request counter; no built-in
Anthropic estimate is advertised. See the shared `tests/fixtures/sdk-fit` profiles.

| Evidence | status | `require_met()` / `requireMet()` | `require_fit()` / `requireFit()` |
| --- | --- | --- | --- |
| Exact within ceiling | met | payload | payload |
| Exact overflow | unmet | BudgetUnmetError | BudgetUnmetError |
| Estimated within ceiling, strict | indeterminate | BudgetIndeterminateError | BudgetIndeterminateError |
| Estimated within ceiling, explicit best_effort | indeterminate | BudgetIndeterminateError | payload plus receipt warning |
| Unavailable or over-target estimate | indeterminate | BudgetIndeterminateError | BudgetIndeterminateError |

Set `budget_mode="best_effort"` / `budgetMode: "best_effort"` per fit to accept
estimates; it never relabels them exact or met. Returned `.payload` remains inspectable
even when checked access rejects. Counts are full-request recounts, not sums of group
counts. Only identical safe candidates within one fit reuse bounded cache entries.

## Group dependencies at construction time

```python
from tokenfold import MessageGroup, Request
messages = [{"role": "system", "content": "Use supplied evidence."}]
start = len(messages)
messages.extend([{"role": "user", "content": "Earlier question"},
                 {"role": "assistant", "content": "Earlier answer"}])
groups = [MessageGroup("earlier-pair", tuple(range(start, len(messages))))]
messages.append({"role": "user", "content": "Current question"})
request = Request.openai({"model": "example", "messages": messages},
                         query="Current question", groups=groups)
```

Node uses the same sidecar `{id, indices, required?}` records while constructing
`{kind: "openai_chat", payload, query, groups}`. Finish conditional prepends before
recording final indices. Do not insert private tags into payloads: unknown keys are
not silently stripped. Groups are contiguous, increasing whole-message intervals,
atomic and non-overlapping. Explicit groups default optional; unlisted messages
remain required. Bundle semantic dependencies or mark the whole group required.
System/developer/latest-user/tool/media protections cannot be marked optional.
Initial selection supports text only; tool/media shapes retain baseline with
`unsupported_selection`. Whole-group validity is not downstream correctness proof.

## Explicit Select provisioning and consent

The exact `tokenfold[select]` extra adds only the lightweight runtime, not ML packages
or weights. Node users separately install the same Python runtime. Acquire the
immutable v3 bundle separately, then explicitly provision a dedicated environment:

```sh
python -m tokenfold_select_runtime provision --bundle /absolute/bundle --manifest-sha256 EXACT_SHA256 --device cpu --config-out /absolute/select-runtime.json --approved-by OWNER --scratch-root /absolute/owned-scratch
```

Only explicit provisioning may fetch dependencies. CPU uses FP32; explicit `cuda:0`
uses FP32 parameters/BF16 autocast without device/precision fallback. Changed runtime,
executable or model pins require reapproval. No old v2 package is silently substituted.

```python
from tokenfold import Context, Group, SelectRuntime, Tokenfold
with Tokenfold(select=SelectRuntime("/absolute/select-runtime.json", experimental=True)) as client:
    result = client.fit(Context("Question", [Group("evidence", "Source text")]),
                        target_tokens=32, allow_selection=True)
    print(result.receipt.to_json())
```

Configuration AND per-fit `allow_selection=True` / `allowSelection: true` are required.
Already-fitting, unavailable, strict-estimated, disabled or unsupported paths do not
start scoring. Selection remains experimental and **unqualified**; fidelity smoke,
functional CUDA tests and tiny synthetic examples are not promotion evidence.

## Servers and lifecycle

Reuse a client for warm inference; Python supports `with`, `async with`, and `afit`,
Node awaits `fit`, `warmup`, `close`. Core fits own independent sessions, while one
resident scorer serves a bounded FIFO: 32 waiters and 5000ms queue timeout by default.
Overload raises BusyError; expiry returns baseline/scorer_queue_timeout without a
scorer attempt. Waiting never holds a Core/libuv worker. Cancellation of a waiter
does not kill another job; active inference cancellation kills/waits its child.
Warmup is explicit; cold startup/load (default 120s), inference (default 5s) and cleanup
(default 1s) are separate. Close is idempotent, stops admission and reaps the worker;
later fits raise ClosedError. No retry within a fit or implicit global pool exists.
Already-running application counters cannot be forcibly preempted: do not promise a
bounded whole-call/close deadline across them. Applications own admission, OS memory
limits and filesystem permissions; process isolation is not an OS sandbox.
