# SDK 0.6.0 benchmark follow-up - 2026-11-10

**Development-only; Select remains unqualified.** These are local/development
observations, not representative accuracy, noninferiority, production readiness,
or a controlled before/after SDK speed comparison.

## Downstream answers, not answer retention

We froze four exposed repository JSON tasks and eight hand-authored synthetic
Select tasks before hosted answers. Two repetitions reversed baseline/candidate
order, using temperature 0 and seed 271. This is **12 distinct tasks**, not 24
independent tasks. No judge model or generated code was executed: one final JSON
object had to match the predeclared oracle, including recursive value types.
Duplicate keys, extra fields, markdown, truncation and refusals fail grading.

The hosted model ID was `nvidia/nemotron-3.5-super-vl:free`, restricted to the
`nvidia` provider with fallback disabled and zero price ceilings. Responses
reported provider `Nvidia`. The endpoint advertised a 2026-10-15 model name, but
served checkpoint bytes, fingerprint and service tier were unavailable. This is
an exact model-ID/provider pin, **not an immutable downstream checkpoint pin**.

| Diagnostic | Baseline correct | Candidate correct | Baseline-only losses | Provider prompt tokens |
| --- | ---: | ---: | ---: | ---: |
| Core: 4 fixtures x 2 trials, compact vs. explicit folded JSON | 8/8 | 6/8 | 2 | 24,496 -> 16,600 (-32.2%) |
| Select: 8 synthetic tasks x 2 trials, full vs. 50% rendered-text cap | 16/16 | 16/16 | 0 | 5,706 -> 3,416 (-40.1%) |

Both Core losses occurred on the same API-record lookup, once in each repetition:
the baseline correctly returned plan `free`, seats `1`; the folded-input answer
returned plan `team`, seats `25`. The name and active flag were correct. The
folded JSON still decoded exactly to the original value. **Codec reversibility
is not model-answer equivalence**, even with a shared explanation of column and
dictionary frames. Default SDK `Data` remains compact JSON; folding is opt-in.

Select's tasks cover code, logs, retrieval, a flattened conversation and an
unknown-value abstention. Supporting groups occupy positions 0, 1, 2, 3, 4, 0,
1, 2 rather than always appearing first. Dependencies were explicitly bundled by
the fixture author. All eight candidates scored, selected, reconstructed complete
groups in source order and met their exact local targets. This does not test
dependency discovery, required-group protection or provider-role conversation
compaction; all groups were optional. It does not establish general quality.

Before any hosted answers, a separate review pass corrected an omitted schema
oracle field and a first-group placement bias. Earlier local preparations were
retained. These development revisions are another reason not to label this an
untouched held-out qualification set. No prompt, oracle, target or selected
payload was changed after the hosted answers, including the two failures.

## Accounting and failed scopes

There were **50 actual generation attempts**, with no retries or automatic
provider fallback:

1. A `google/gemma-4-31b-it:free` / `google-ai-studio` connectivity probe returned
   HTTP 429 from the shared upstream pool. That scope stopped; all 48 planned
   quality cells remained unexecuted. Its usage and charge are unknown, not zero.
2. A separately frozen and announced NVIDIA scope completed one connectivity
   probe plus all 48 quality calls. All 49 replies ended with `stop`, with no
   BYOK, refusal or nonzero reported charge. The probe is excluded from accuracy
   and paired token tables, but included in all-call accounting.

NVIDIA reported 50,256 prompt + 749 completion = **51,005 tokens including its
probe**. This is not a complete campaign total because the failed probe's usage
is unknown. Request pricing was unreported, and provider-reported zero charges
are not audited invoices or complete billing savings. No dollar-saving ratio is
claimed. The older 12 HTTP-400 attempts described in
[SDK validation](sdk-validation-0.6.0.md) remain separate historical evidence.

Provider prompt counts include the full submitted messages and shared formatting
instructions. They are not local `o200k_base` counts:

| Quality-call totals | Local rendered baseline -> candidate | Provider completion baseline -> candidate |
| --- | ---: | ---: |
| Core | 18,988 -> 12,044 (-36.6%) | 202 -> 204 |
| Select | 3,786 -> 1,694 (-55.3%) | 168 -> 169 |

Completion tokens did not decrease. All wrong answers remain in these totals.
Core reported 4,336 cached input tokens per arm; Select reported zero. Cache
and free-provider load were not controlled. Client-call medians, including the
isolated HTTP worker but excluding SDK preparation, were 761.95 -> 849.92 ms
for Core and 608.27 -> 603.04 ms for Select. **No latency improvement is claimed.**
The 900-second limit was a dispatch deadline plus child cleanup, not a strict
whole-process lifetime guarantee.

## Other benchmark and correctness checks

The preceding Core rerun reproduced all seven published structural output counts,
with exact JSON-value recovery; the six-fixture corpus remained 13,071 -> 8,715
tokens. Tokenfold vs. TOON remained 12,776 vs. 14,841 tokens, both round-tripping
all seven fixtures. These local JSON paths are not hosted provider comparisons.
Fresh pinned-source Headroom output totaled 12,819 rather than historical 12,807
tokens (+10 incident-feed, +2 OpenAI payload), still value-equal on 4/6 cases.
The README's existing comparator tables retain their historical source snapshot;
this small environment-dependent drift is not attributed to the SDK.

Explicit SDK folded Data totaled 12,772 instead of 12,776: four fewer tokens from
two `0.0` -> `0` normalizations in one report fixture, not better structural
folding. The opt-in pruning showcase remained 304,032 -> 11,364 tokens (96.3%),
with retrieval/preserve/fallback assertions passing; it remains unvalidated
lossy evidence. Tool-schema savings remained 45.63%. The historical 67.6%
repetitive-JSON claim lacks a versioned input and was not reconstructed.

The release-Rust benchmark gate passed: command p95 449.972 ms, structured JSON
p95 1469.547 ms, total allocation 1,580,507,977 bytes/call, savings ratio 0.4563.
Historical threshold baselines used another compiler/time block; lower observed
latency/allocation is not causal SDK speedup evidence. Total allocation is not
peak resident memory. Folded SDK performance was mixed across fixtures, including
a slower incident-feed path; there is no universal speedup claim.

Fresh local checks on Windows x64, Rust 1.98, Python 3.12 and Node 22 passed:
`cargo fmt --all --check`, workspace Clippy with denied warnings, 773 Rust tests
(two explicit real-worker/model entrypoints ignored), the installed Python suite,
43 Node tests with zero skips, and the deterministic fidelity smoke. These are
not cross-platform or exact-head hosted CI receipts. Initial test runs lacked
Pyright and explicit CLI/Python test paths; those failures were retained and
resolved by supplying the existing test prerequisites, without product edits.

## Evidence and reproduction

- [Frozen corpus](../eval/research/sdk_quality_corpus.json): questions, typed
  oracles, original source paths/hashes, balanced groups, exact submitted message
  arrays, common request template, targets and selection exposure.
- [Curated result ledger](../eval/research/sdk_quality_results.json): every
  quality reply including both failures, probe outcomes, prior unexecuted cells,
  provider usage, cached tokens, timings, charge limitations and source/method
  hashes. Private request/account IDs and credentials are not published.
- [README metric pins](../tests/fixtures/readme_metrics.json): canonical-JSON
  SHA-256 digests and the exact displayed quality counts.

Offline evidence reconciliation (no model, weights or network):

```sh
python -m pytest python-tests/test_sdk_quality_evidence.py -q
```

To replay hosted answers, use a **fresh, explicitly bounded scope**, verify the
current endpoint's zero prices and supported parameters, and compose each body
from the corpus's `protocol.request_template` plus that arm's `messages`.
Follow its two-trial order and 48-call maximum, with at most one separate probe,
1024 output tokens, no retry/fallback, durable dispatch/terminal accounting and
the same exact grading. The existing
[free-provider adapter](../eval/contextdoctor_openrouter.py) supplies catalog
validation, guarded HTTP and receipt normalization. A changed model/provider or
configuration is a new scope, not a resumed or relabeled old run. Future hosted
outputs are not guaranteed to reproduce because checkpoint/cache/load are not
immutable.

Frozen message replay tests interpretation of the measured SDK outputs, not a
fresh execution of Select. To regenerate SDK inputs, install 0.6 artifacts and
use `Data(value)` / `Data(value, representation="folded")` for Core. For Select,
build `Context(query, groups)` in the corpus order, use an explicitly approved
v3 CUDA/BF16 runtime, and fit with `allow_selection=True` at
`floor(full_rendered_baseline / 2)`. Model acquisition and runtime approval remain
explicit; this corpus/test does not download weights, provision environments or
make hosted calls automatically. See [SDK migration](sdk-migration.md).
