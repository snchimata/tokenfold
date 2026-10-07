# Research harnesses

Nothing here is on a served compression path.

## Competitive qualification order

1. P0: freeze representative held-out workloads and verify comparator provenance.
2. P0: qualify official ACON base/optimized observation, history and combined baselines.
3. P0: measure live task non-inferiority and CFR with predeclared uncertainty bounds.
4. P0: compare total compression, verification, answering, retrieval and cache economics.
5. P1: complete history/host recovery integration and qualify a reviewed Select runtime.
6. Add a separate experimental generative observation summarizer research arm once
   the P0 protocol/baselines are established; history/combined generation follows
   observation qualification. Preserve protected literals outside generation, enforce
   deadlines/cancellation and output/call bounds, and expose baseline fallbacks.
   Source references or another model's approval do not prove paraphrase fidelity.
   Do not relax the existing literal-extract semantic API or promote defaults.

`freeze_campaign.py` records a predeclared protocol and exact paired-task hashes
without making model calls. It reuses the paired fixture loader, rejects duplicate
IDs and cross-split source/cluster overlap, bounds declared settings, confines files
to the workspace, pins the answering transport (`runtime.transport`) and the
system/user answering roles with assistant prefill disabled, and refuses
overwriting a frozen artifact. The runner declares the transport it actually
uses (Ollama loopback, native OpenAI loopback, or zero-price OpenRouter with no
redirect/fallback); any live backend or role drift fails exact `verify_run`
comparison. Qualification scope requires the same pin alongside train/validation/test
splits, all five workload tags, explicit cluster IDs
and enough declared/test clusters for the zero-event CFR sample floor. These are
necessary checks, not proof that labeled workloads are representative, clusters are
independent, pinned transports attest model weights, or enough raw answers will
succeed. Repeated seeds do not add clusters.

Qualification protocols must declare `quality.cfr_claim_count`, at least the
number of comparator arms (excluding raw), including any additional subgroup CFR
claims. The zero-event floor uses Bonferroni allocation:
`ceil(log((1-confidence)/cfr_claim_count) / log(1-max_cfr))` independent
raw-success clusters **per claim**, not per seed. At 95% family confidence and a
1% ceiling, one claim needs 299 clusters; two need 368. The freeze records the
method and floor in `cfr_sample_rule` and checks the predeclared minimum and total
test clusters. It does not establish per-claim/subgroup raw-success counts, cluster
independence, non-inferiority power, representativeness, or a simultaneous bound
for other metrics. The existing smoke runner still refuses qualification scope;
its nominal CFR reports must not be presented as corrected family-wise results.
Historical smoke protocols need not declare a claim count.
Qualification freezes also refuse origins starting with `synthetic-development`
or `synthetic-protocol-control`, or containing `not-qualification` or
`not-agent-qualification` (case-insensitive). A declared
origin must be a nonempty string. These declarations remain usable in smoke
scope, including existing USGS/Loghub/MTRAG development importers and synthetic
protocol controls. Missing/other origin labels are not provenance proof; removing
or relabelling a development origin does not make previously exposed tasks fresh
or representative. The guard enforces explicit counterevidence, not an automatic
dataset certification. Historical reports are not rewritten.
Freezing now confines discovered task paths before the loader reads them, then
rechecks each recorded path and the complete suite inventory after final byte
checks. A task added during the final pass invalidates publication instead of
silently leaving it out. This is best-effort mutation detection, not an atomic
filesystem snapshot; before/after live verification remains required.
Validation (Windows, 2026-10-05): thirteen freeze and ten native tests pass,
including injected task addition with no output artifact and resolved-outside
path refusal before loader invocation. Workspace fmt/clippy/tests and deterministic
fidelity smoke pass. No inference or held-out outcomes were evaluated.
Windows validation (2026-10-05): ten freeze and four USGS importer tests pass,
including refusal of a declared synthetic control in an otherwise sample-adequate
qualification fixture and unchanged smoke support. Workspace fmt/clippy/tests and
deterministic fidelity smoke pass. No dataset downloads, inference, historical
report replacement or held-out outcome evaluation was performed.

Windows validation (2026-10-05): nine freeze, three exact-confidence and 16
campaign-arm tests pass. The freeze regression rejects a two-claim declaration
with 300 clusters even after increasing the declared minimum, then accepts 368
actual declared clusters and records per-claim alpha 0.025. Invalid/under-counted
claim declarations fail; historical smoke declarations remain supported. Workspace
fmt, clippy, tests and deterministic fidelity smoke pass this cycle. The earlier
proxy empty-response test failure remains part of the validation history; no proxy
fix or claim of eliminated intermittency is made. No inference or held-out task
evaluation was performed.

```sh
python eval/research/test_freeze_campaign.py
# Copy/edit this example with the approved model revision before actual inference.
python eval/research/freeze_campaign.py \
  --protocol eval/research/campaign_protocol.example.json \
  --output tmp/campaign-freeze.json
```

The example intentionally freezes only existing six-task **smoke** evidence. Its
quality thresholds are proposed declarations, not achieved results or approval.
The tool is offline preparation; the live pilot can enforce it with
`--campaign-protocol` and `--campaign-freeze`. It does not implement runtime arms
or certify model descriptors. Frozen arms, model revision, seed, ratio, context,
output, call budget and deadline must match before inference. Changes during a
run invalidate its provenance. Observation/replay pilots refuse qualification scope.
Previously exercised smoke sets must not be relabeled fresh qualification data.

The shared comparator checkout check refuses `assume-unchanged` and
`skip-worktree` index flags: Git's clean status can otherwise hide tracked
mutations. Use an ordinary clean checkout for provenance; the harness never
clears those flags or changes comparator files itself. The offline regression
is `python eval/research/test_benchmark_provenance.py`.

Validation on Windows (2026-10-05): the provenance regression reproduces both
hidden mutations with an empty `git status`, rejects each, and accepts the
restored checkout. Provenance (1), campaign-arm (15), and freeze (8) tests pass.
Workspace fmt and clippy pass; the fidelity smoke gate passes (deterministic
fixtures, not competitive evidence). Workspace tests fail at proxy test
`duplicate_conflicting_content_length_headers_are_rejected` with an empty
response. The ACON test script stops because `jinja2` is unavailable. Those
failures were not retried or excluded; no model calls or quality claims result.

An explicit task `workload` must also match its suite's declared workload; a
`mixed` suite uses each task's supported workload label. Legacy fixtures without
a workload still use the non-mixed suite label. This prevents relabelling retrieval
tasks as code/logs to satisfy coverage, but does not prove representativeness.
The updated offline audit checked 29 discovered protocols: 24 passed, with only
the five previously recorded split/literal-answer failures. No new workload
mismatch was found or inference made. The report is in
`tmp/campaign-workload-audit-20261005`; historical reports remain unchanged.

Training/validation suite execution is now explicit: add `--campaign-split train`
or `--campaign-split validation` and declare the same `campaign_split` in frozen
`runtime`. The verifier requires a suite with that exact split and directory;
the default remains `test` and cannot silently authorize a validation suite.
Opted-in report metadata uses `suite_directory`/`suite_split`, not `test_directory`.
This does not permit qualification scope or turn development selection into
held-out evidence. Keep candidate selection rules frozen before validation calls.

## Existing runners

### Attempted methods versus blocked arms

Research `HeadroomArm` callers may explicitly supply `capture_path` together with
`guard_binary`. Source and serialized capture must pass native safety guards;
exclusive files retain the actual worker payload/receipt, source/query hashes and
provenance before unhealthy-result refusal. Existing captures are never overwritten
or retried. This supplies future wrapper evidence, not retroactive recovery of
discarded payloads, valid-arm admission or a default capture/export behavior.

`tmp/usgs-ccr-copied-consumer-control` used the pinned upstream SQLite backend in
a separate process against a database backup, not the original retained store.
Its unchanged-TTL lookup hit; backend setup/retrieval took 14.467 ms and bounded
restoration 5.796 ms, returning the exact 23254 source bytes. Original input/store
hashes stayed unchanged. The wrapper was explicitly constructed because the
original emitted wrapper was unavailable; this is not that consumer's recovered
result. Full recovery returns the raw 8336-token payload (4168 target), so it is
not compression or a summarizer win. No model calls or TTL extension/retries;
cross-request authorization, original-wrapper recovery and downstream quality
remain unqualified. Import/startup time is outside the two reported stage timings.

`restore_authorized_rows` is a bounded research primitive for a trusted caller's
source, explicit row path and expected marker. Recovered arrays must match source
content/order (including duplicates and JSON scalar types); substituting them may
not change any outer-wrapper fields. Only then are exact original source bytes
returned. It performs no store lookup or cross-request authorization and adds no
model/default recovery path. Full recovery may exceed budget and must not be
reported as a compression/summarizer win; caller safety checks remain required.

The retained validation SmartCrusher store was inspected read-only in
`tmp/usgs-ccr-retained-scope-audit`: it stores all 32 rows of the source's nested
`features` array, in matching order, not the entire FeatureCollection tool string.
Hashes remained unchanged; no compression/inference rerun or text export occurred.
`audit_store_restart` now supports bounded explicit `source_row_path` (for example
`["features"]`) to bind such diagnostics to current source data. This does not
restore the outer wrapper or prove cross-request authorization/model-directed
recovery; the prior unresolved-recovery arm remains invalid and unchanged.

Prospectively frozen `tmp/usgs-preselection-fullpath-validation` exercised the
declared USGS validation suite (one source cluster, development/possible prior
exposure, not qualification). Same resident 4B answering model, seed/roles/context,
4096 output allowance; separate resident default 0.8B summarizer. Declared order
was raw, lossless, generated, Headroom universal, SmartCrusher, base ACON.

| Arm | Exact outcome | Payload tokens | Full arm wall ms |
|---|---|---:|---:|
| Raw | failure | 8336 | 1932.329 |
| Lossless | failure | 8131 | 2438.326 |
| Preselected generated/unverified | success | 500 | 1492.986 |
| Headroom universal default (unchanged payload) | failure | 8336 | 12635.630 |
| Headroom SmartCrusher default | invalid: unresolved CCR recovery | unavailable | 4228.443 |
| Official base ACON | success | 700 | 28851.403 |

All pins/freeze/profile/source checks stayed stable and both servers were reaped.
Runner exit 1 retained the invalid SmartCrusher attempt (no downstream call), not
a compressor quality loss. Totals: seven primary calls and two 0.8B calls including
warmups, no retries. Generated work used 2322/103 summarizer input/output tokens
and 729/9 answering tokens, separate ledgers; base ACON used 10050/2328 compression
and 1116/9 answer tokens. Startup/warmup are separately recorded. Costs unknown,
no raw-success denominator, no all-arm common-valid intersection, fixed order and
cold comparator processes: this point result is not CFR/NI, controlled warm
superiority, optimized/history/stateful comparator qualification or economics proof.

Research `--arm-order <names...>` must name each live arm exactly once and is
frozen as runtime `arm_order`. Defaults retain raw/lossless/ACON/extras order.
Execution rows follow the explicit order while pairs still bind the same task,
snapshot and seed, even with raw later. No ledgers reset or retries occur after
halt. Prospective order can expose different attempted/blocked arms, not erase
earlier counterevidence; balance/order/cache effects still require real study controls.

Prospective `tmp/usgs-earliest-frozen-frame-answers` tested the exact retained
preselected frame against raw input with separately pinned resident 4B answering,
same query/roles/seed/allowances, fixed raw-first order and no recompression. Both
answered `us7000gbcd` correctly. Raw used 9595 input/8 output model tokens and
2250.852 ms caller wall; frame used 772/8 and 1091.661 ms. Only warmup plus the two
answer calls ran; inputs stayed stable and the server was reaped. Prior 0.8B
generation ledger remains separate; do not sum incompatible model token units or
call this an end-to-end latency/economics win. Artifact reuse, fixed order/KV-cache,
separate generation sessions, N=1 exposed TRAIN and unknown costs limit inference.
The summary's listed event values match its literal record; date bounds/order
match source metadata/ordering. That scoped inspection is not a general
faithfulness verifier, semantic admission, representative non-inferiority or a
five-workload qualification result.

Comparison profiles may explicitly set `preselect:true` with a numeric selection
contract. Native source-only evaluation restricts generation/citation IDs to the
selected literal group(s), retains all comparison rows and existing protected
context, and computes no inferred prose-query contract or evaluator label. The
source/record binding and coverage checks still run; no eligible rows retain all
record IDs. This is opt-in research, not production semantic admission.

Prospective exposed TRAIN `tmp/usgs-earliest-preselected-training` retained the
expected `us7000gbcd` evidence ID at 531 payload tokens, generated/unverified not
raw fallback, with default resident 0.8B. Usage was 2252 input/152 output model
tokens and adapter wall 994.845 ms; only warmup plus one summary, stable inputs,
reaped server, unknown cost. Prior table-only failure (11727 input, wrong selected
ID) remains preserved. Current validation-only guards also changed, so do not
attribute the wall-time difference solely to preselection. Literal retention is
not summary entailment, downstream correctness, representative coverage or a win.

Research guards now use experimental Select `--validate-only`, preserving bounded
raw/schema/decoded-secret checks while avoiding exact-tokenizer initialization and
selection/output work. Normal Select remains unchanged. A unit regression asserts
no estimator initialization and refusal of escaped/split secrets and bad schema.
On the same retained TRAIN source, validation-only guard pairs measured p50
31.038 ms; first/p95 pair was 423.462 ms (10 timings). Current-binary normal
selection measured p50 1007.483 ms/p95 1047.628 ms on the same source. Both
profiles remain preserved; this is stage timing, not an
end-to-end, model-quality or representative win. No batching/daemon was added.

`tmp/native-guard-stage-profile` freezes a native-guard microprofile on the retained
exposed USGS TRAIN source: two separate source/metadata guard processes per pair,
10 timing samples, zero inference. First pair 1053.709 ms, p50 1018.609 ms and p95
1056.097 ms; inputs/binary stayed hash-stable. Client wall includes process startup,
native validation, accounting and output, not just secret scanning. This identifies
a measurable stage cost but does not justify removing either safety check or claim
an implemented speedup. One source and repeated timings are not workload coverage.

`code_repair.isolated_test_command` only prepares a Docker argument vector for an
immutable local `sha256:` image and capsule `evaluator.py`: no pulls/network,
read-only capsule, non-root, dropped capabilities, no-new-privileges and fixed
CPU/memory/process/temp limits. No engine/image is started or downloaded. Caller
approval, actual image/runtime checks, deadline enforcement and explicit container
termination after timeout are still required; killing Docker CLI is not server
cancellation. Command construction is not evidence of isolation or repair success.

`code_repair.freeze_capsule` hashes bounded, disjoint actor/evaluator/dependency
files plus Python identity, rejecting actor-envelope evaluator fields and input
mutation during freezing. The maintenance capsule's new role-separated freeze is
additive: prior probe results are unchanged and are not retroactively qualified.
It is not a complete hermetic runtime lock or proof that source text contains no
hidden labels. Docker execution and data-export authorization remain separate.

The existing offline research CI job now runs `python eval/research/test_code_repair.py`,
including declared input/output/edit-count limits. This uses only the standard
library and does not start Docker, execute candidate code or constitute repair
qualification. It preserves the separate approval boundary for isolated execution.

`code_repair.edit_receipt` reports bounded source/response hashes, applied status,
static application failure reason, repaired hash on success and wall time, without
candidate text. It always records `candidate_code_executed=false`; application is
not a repair verdict. This improves future probe diagnostics, not the previous raw
probe: its missing response/application reason cannot be reconstructed or retried.

The frozen `tmp/code-raw-answer-training` probe exercised the separately pinned
local resident 4B answering model on the exposed maintenance actor capsule. It
made warmup plus one raw structured edit generation, no summarizer/comparator
calls, retries or code execution. Generation completed (4209 input/113 output
model tokens, 2048.612 ms client generation wall), but the response did not pass
bounded literal application. Adapter/check wall was 3154.875 ms, startup 4026.510
ms and warmup caller wall 390.221 ms. Inputs stayed stable and server was reaped;
costs remain unknown. No repair verdict exists without isolated tests. The failed
edit response was not persisted, so the precise application error cannot be
audited; do not rerun or reinterpret it as a repair success. The unused copied
CLI approval is not an active summarizer identity: the sole runtime ledger is 4B.

`code_repair.apply_edits` reads the capsule's bounded JSON edit list without
executing candidate code: one caller-owned source string, no paths/commands,
unique literal spans, strict duplicate-key/schema checks and bounded output.
Successful parsing is not a passing repair. The installed Docker CLI currently
cannot connect to its Linux engine; no daemon was started or image downloaded.
Arbitrary model patch execution remains disabled pending an isolated runtime and
separately frozen evaluator/dependency configuration.

`tmp/code-duplicate-parser-maintenance` begins a real repository-maintenance TRAIN
capsule from the preserved defective research source (SHA
`5c8d83e6ec0b3449f91da8c93ce170f162867e7ca80c04f0af042735c5832e46`),
not generated handler-marker questions. Actor input contains source/problem/output
contract, not regression inputs/verdicts or the repaired file. Separate executable
checks demonstrate the original accepts top-level/nested duplicate candidate keys
while current code rejects both; an ordinary-candidate control passes in both.
These are trusted source snapshots, scripted responses and mocked native guards,
with zero inference. One exposed/already-repaired task is not representative
code qualification; arbitrary model patches have not been executed and no
downstream repair/compression advantage has been measured.

Qualification freezes require an `advantage` contract: `latency` declares a warm
end-to-end p50/p95/p99 millisecond `metric` and positive `min_relative_reduction`;
`economics` declares `total_campaign_cost_usd`, a positive reduction and explicit
`cost_basis`; `features` lists unique required behavioral outcomes. Reductions
must be less than 1. These are frozen hypotheses, not measured wins: unknown costs,
undeclared amortization, fallback results or feature checklists cannot establish
them. Smoke remains compatible and the pilot still refuses qualification execution.

When frozen quality declares `cfr_claim_count`, research cluster CFR bounds now use
Bonferroni per-claim confidence `1 - (1 - family_confidence) / claim_count`, matching
the freezer's sample rule, with explicit multiplicity metadata. Profiles without
that declaration retain prior descriptive single-claim bounds. This adjusts both
observed and missing-outcome sensitivity bounds; legacy bootstrap CI95 fields are
unchanged descriptive summaries. Declared subgroup counts do not automatically
produce subgroup results or prove independent sampling/non-inferiority.

`test_campaign_arms.py --acon-root <pinned-checkout>` includes a frozen-profile
integration canary using the real CLI/official ACON and scripted responses. It
checks source selection receipts and the frozen profile hash, then independently
injects a profile-byte change after deployment: final status fails, with all six
scripted calls and all comparison files retained. This is a synthetic protocol
check, not inference retry, representative selection or model-quality evidence.

Comparison-profile whole-suite preflight and generated-arm validation use the
same source-only record-binding helper. Invalid optional group IDs/record
projections stop before model setup, not merely before the generated arm's call.
This avoids spending on earlier arms under an unusable declared profile.

Research runner `--generative-comparison-profile profile.json` requires
`--generative-arm`. The strict, bounded profile has `record_path`, `columns` and
optional `selection` using the API contract above; unknown/duplicate keys are
refused. All task sources are preflighted before model setup. Frozen runtime must
include `generative_comparison_profile_sha256`; the report retains configuration
and its final stability check. Changed/unavailable profiles fail the final status
without erasing consumed attempts. This does not enable qualification scope or
alter production/default behavior; caller contracts must express the current query.

Explicit selection coverage additionally requires one literal JSON record per
optional source group (an array-leading comma is framing). Each group's projected
columns must match the corresponding source-derived table row before inference;
swapped group labels are refused even if concatenation reconstructs the source.
Other grouping layouts remain supported without this opt-in coverage contract;
they are not silently interpreted as verified record bindings.

Post-hoc `tmp/selection-coverage-retained-audit` applies explicit current-query
coverage to the retained latest/earliest TRAIN failures without inference. Both
wrong evidence sets were rejected; two constructed correct-ID controls passed.
On the shared source, parsing plus checking had p50 0.2374/0.2369 ms and p95
0.2594/0.2557 ms (100 timing repetitions per case). Hashes stayed unchanged.
These are two real candidates from one source cluster, not 200 independent tasks.
Constructed IDs are not faithful generated summaries; representative FP/FN rates,
semantic completeness and end-to-end quality/latency remain unproven.

Research API `comparison_selection` can explicitly declare `id_column`, numeric
`filters` (`column`, `op` gte/lte, `value`), `rank_column`, `direction` min/max and
`tie` id-ascending. It validates paths/IDs before inference and rejects cited
evidence omitting the source-computed selected record. Nulls exclude eligibility;
no eligible records require all source IDs as absence evidence. Contracts must
represent the current user query, not gold/support labels; there is no query
parser or qualification. Summary entailment/comparison
completeness remain unverified even when this narrow coverage check passes.

The separately frozen `tmp/usgs-earliest-table-training` control changed the TRAIN
query to earliest (smallest timestamp), avoiding a restart of the interrupted
latest-table probe. The bounded API retained all original groups plus every
record's ID/magnitude/time/depth, without gold labels. Default resident 0.8B made
only warmup and one summary call; source hashes stayed stable and server was
reaped. Its 494-token unverified frame selected `us7000g8ad`, whereas the later
evaluator-only earliest calculation expected `us7000gbcd`; that ID was absent from
selected evidence. Usage was 11727 input/99 output, adapter wall 3123.605 ms and
cost unknown. Frame and audit are preserved. This is N=1 constructed TRAIN
counterevidence, not a matched comparison, downstream result or representative
claim; a compact numeric hint did not establish correct constraint selection.

Research comparison-table source JSON and generated candidate JSON reject duplicate
object keys at every nesting level using the existing strict response parser.
The table never silently projects a last-key-wins interpretation of ambiguous
source; malformed candidates retain a failed-attempt receipt and untouched source.

The research `GenerativeArm` API can explicitly supply `comparison_record_path`
and `comparison_columns` together to attach a bounded source-derived scalar table
alongside unchanged complete groups. All records remain in source order, including
missing/null values; it computes no ranking or answer and accepts no evaluator
labels. Paths/method are recorded in receipts. This has offline contract checks,
not model-quality qualification. The explicit research profile above exposes these settings. The interrupted earlier
`tmp/usgs-summary-table-training` probe has no terminal result and unknown consumed
work; its interruption record is retained and it was not restarted.

The prospective `tmp/usgs-summary-comparison-training` v2 instruction experiment
explicitly required condition/comparison/tie-break evidence. It returned the exact
same 538-token frame (SHA `aaddf4bb757c174ce1a7f27f580f81d20952f5a02194b77ea1dfec90b7309dc6`)
and wrong evidence selection as v1, adding 80 input model tokens (10295 versus
10215), with 99 output tokens and 2549.625 ms adapter wall. Gold literal remained
absent from evidence. Both attempts are retained; no downstream calls or retries.
The ineffective guidance was reverted rather than shipped as an improvement;
its exact module snapshot is retained with the frozen study. This TRAIN result
argues against further prompt-only claims, not against every selection approach.

A distinct prospective TRAIN arm at `tmp/usgs-summary-schema-training` used the
research structured source-ID guideline with resident 0.8B and explicit native
template preflight (not a rerun/replacement of the CLI probe). It persisted the
public returned frame and made only warmup plus one summary generation. The frame
fit at 538 tokens, but selected only `us7000g8fk`; evaluator-only post-generation
literal audit found the gold identifier absent from selected evidence. Summary
usage was 10215 input/99 output model tokens, adapter wall 2540.272 ms, startup
1383.482 ms and warmup caller wall 386.812 ms. Input hashes stayed stable, server
was reaped and monetary costs remain unknown. This is retained N=1 TRAIN selection
counterevidence: structural acceptance/budget fit did not ensure answer evidence
retention. No downstream quality or superiority was measured; do not promote it.

The separately frozen `tmp/usgs-summary-only-training` probe exercised the resident
default 0.8B CLI on the same exposed first USGS TRAIN task that the earlier primary
halt prevented from reaching compression. It made exactly two generations (warmup
and summary), no downstream calls/retries: startup 1374.636 ms, warmup caller wall
258.977 ms, summary CLI-adapter wall 1487.474 ms (receipt wall 859 ms). The summary
reported 10592 input/68 output model tokens; the complete o200k text frame fit at
506 of 4046 target tokens, versus source 8092. Disposition was `generated_unverified`,
not raw fallback. Inputs stayed hash-stable and the owned server was reaped; costs
are unknown. The study did not persist the returned summary frame, so faithfulness
and downstream correctness cannot be audited from this artifact. This is N=1
training budget/lifecycle evidence only, not a campaign retry, quality win or
qualification. Retain the separate earlier raw failure/blocked-arm counterevidence.

Research-generated receipts retain `candidate_budget` after output guards: complete
text-frame, original-source and target counts, including over-budget/not-smaller
fallbacks. Counts include attached evidence and markers, not the model chat template.
No rejected summary text is added; the counted frame is reused for both size checks.
These diagnostics do not establish accepted summaries or a quality/latency win.

Research cluster CFR reports include `missing_outcome_sensitivity`: excluded
invalid/unpaired clusters are hypothetically raw successes with regressions,
deduplicated across tasks and attempts. This worst-case sensitivity does not
relabel invalids as observed failures. The original common-valid CFR bound is
unchanged; neither bound proves independence, representativeness or non-inferiority.

Qualification freezes also reject identical source bytes assigned to different
cluster IDs, even when queries/task IDs differ. Such projections must share a
cluster and cannot inflate the sample floor. This check is qualification-only;
different source bytes do not prove independence (overlapping windows, shared
documents and trajectories still require justified provenance-bound clustering).
Qualification requires a nonempty task `origin`; omitting provenance is not an
escape from explicitly unqualified origins. Smoke may still omit it. An accepted
origin string is only a declaration, not verified provenance or representativeness.

Post-hoc application to the retained exposed TRAIN full-path USGS run is preserved
at `tmp/usgs-native-fullpath-training/missing-outcome-sensitivity.json`, with its
exclusive-write reanalysis script and input/implementation hashes. No inference
was rerun and historical files stayed byte-identical. The single lossless pair had
no raw success (observed CFR unavailable); ACON, CLI generation and both Headroom
arms each had zero valid pairs and one invalid record. Their deliberately
conservative hypothetical missing-outcome bound is 1, not a quality result or a
reason to relabel known raw failure. This remains N=1 training counterevidence.

The CLI summarizer adapter refuses contradictory receipts: `runtime_invoked=false`
cannot accompany reported inference usage or a generated output disposition. Such
attempts retain parsed usage, mark invocation unknown, halt the ledger and export
no candidate. A consistent pre-inference raw fallback still removes the reserved
model-call entry rather than inventing a zero-token inference. These are ledger
integrity checks, not evidence that an untrusted receipt measures actual server work.

The pinned official ACON HistoryOptimizerV2 preserves replay roles and appends its
compression instructions to the final message. History/combined pilot arms now
require that final message to be a user message: assistant-final replay is refused
before inference, not rewritten into a different comparator or silently treated as
approved assistant prefill. Provenance records `history_transport`. The real pinned
history and combined classes have scripted-response integration checks for role
order, input immutability, no evaluator-label export and pre-inference refusal.
These checks do not qualify optimized guidelines, full agent state or model quality.

Pilot economics rows now include `execution`: `compression_method_attempted`,
`answer_method_attempted`, and `blocked_by` (`primary_model`, `evaluator`, or null).
A halted primary/evaluator before those methods marks the arm blocked, keeps its
invalid outcome, and does not fabricate compression/generation usage. Method
attempt flags are not evidence that inference started; provider ledgers and
accounting receipts remain the consumed-work record. Raw has no compression method.
Aggregate `blocked_before_methods` and `execution_state_unknown` separate known
blocked rows from historical reports without metadata. Historical rows are not
silently classified or rewritten, and halted models are not reset/retried just
to obtain usable comparator results. Existing source/payload count fields on a
blocked row describe its retained input, not an actual compressor output or miss.

`execution.output_available_for_answering` records a payload that passed validation
before the answering method, even if answering subsequently fails. Aggregate
`observed_output_budget` counts budget fit only for those non-raw outputs, separately
reporting unavailable outputs, unknown historical state, excluded raw controls and
known fallback outputs. Known raw fallbacks remain included but are not summarizer
wins. Legacy `target_misses` is unchanged; availability or budget fit proves neither
semantic admission nor downstream quality. Historical results are not rewritten.

The regression uses a native primary deadline failure, verifies only one generation
request is attempted and no compressor runs, then checks all later rows are invalid
with the correct blocked state and retained work. The initial test used legacy
`LocalModel`, which has different halt semantics; it was corrected to exercise the
actual native policy rather than changing that policy. Twelve native, 22 ACON,
nine judge and nineteen arm tests pass. No new inference or held-out evaluation
was performed, and no historical full-path failure has been relabelled.
Workspace fmt/clippy/tests and deterministic fidelity smoke also pass this cycle.

### Frozen full-path USGS TRAIN counterevidence

`tmp/usgs-native-fullpath-training` preserves a prospective smoke/TRAIN campaign
with raw, native lossless, official pinned base ACON, actual resident 0.8B `summarize`
CLI, and both pinned Headroom default observation arms. It selects the first
filename-order previously exposed USGS selection TRAIN task, with no held-out or
outcome-based selection. One owned resident 4B answering/ACON server and one owned
0.8B summarizer server remain loaded; warmups are charged to those same model
ledgers. The primary uses frozen experimental native preflight. Output allowances
were predeclared as 1,536 primary/1,024 summarizer, not automatically escalated or
retried. No prompt receives gold/evaluator fields.

| Arm | Outcome | Payload o200k tokens | Deployment wall ms |
| --- | --- | ---: | ---: |
| Raw | Wrong exact answer | 8,092 | 1,743.270 |
| Native lossless | Wrong exact answer | 7,887 | 2,333.524 |
| Official ACON base | Invalid generation | 8,092 raw source retained in record | 19,619.226 |
| 0.8B CLI / Headroom universal / Headroom SmartCrusher | Unexecuted after primary halt; invalid | Not compressed | Not comparative latency |

ACON consumed 9,746/1,536 input/output tokens and failed with the historical
generic `generation_failed` reason. Cap exhaustion alone does not prove the exact
failure category; no inference retry was made. The primary halt prevented further
answering/comparator execution, so these rows are not quality losses, recovery
failures or fallback wins attributable to those unexecuted compressors. The run
returns failure and keeps all attempts. It establishes no advantage: both valid
answers were wrong, lossless had higher measured wall time, and raw-success N=0.

Startup was 3,673.4451 ms for 4B and 1,398.2590 ms for 0.8B. First-generation
warmup wall (including verification/preflight where applicable) was 345.8009 and
232.0446 ms respectively. All ledgers retain four 4B calls (one warmup + three
deployment) and one 0.8B warmup call; summarization itself was not invoked. Report
and caller code/data/runtime hashes stayed stable; both servers were reaped. GPU
snapshots on RTX 5080 were 3,633 / 14,637 / 3,629 MiB used before load / after load /
after reap (16,303 MiB total). These snapshots are not peak-memory measurement or
controlled latency distributions. Local monetary cost is unknown, not zero.

The evidence motivated a small failure-receipt fix: `NativeModel` now retains
only allowlisted fixed reason codes such as `incomplete_response`,
`model_alias_mismatch`, and client deadline/transport failures. Unknown exception
strings remain `generation_failed`, with no body/source/credential diagnostics.
The original result is not relabelled under the new code; a native-client source
snapshot preserves its original version. Regression tests verify safe reasons,
consumed usage, no retry and suppression of a custom private diagnostic. Further
qualification must address failure policy without cherry-picking usable arms or
calling this frozen smoke study a representative-workload win.
Twelve native contract tests, nineteen arm tests, workspace fmt/clippy/tests and
deterministic fidelity smoke pass for the safe-reason change.

### Opt-in native-template preflight API (experimental)

`NativeModel(..., native_template_preflight=True)` enables an explicit research
alternative to the unchanged default byte guard. It seals the complete body
(including roles, query, wrappers and any response schema), obtains resident
template/tokenizer counts without cropping content, and reserves requested output
plus the existing 1,024-token margin. Accounting and generation share the remaining
post-alias-verification request deadline. Missing/malformed accounting, changed
sealed body or unavailable APIs fail closed without fallback/retry. Known
over-context inputs are refused before any generation call is recorded.

Generation must report the exact preflight prompt count and stay within output/
context allowances; otherwise its usage is retained, response text discarded, and
later calls halted. This still trusts server-supplied accounting, not weight identity,
actual source semantics or server cancellation. No production admission or default
changed. The option is exposed through an explicit campaign CLI/profile opt-in, and is
not implicitly applied to the approved `summarize` bridge's different body builder.
Future frozen studies must declare this mode and bind actual owned runtime/template
configuration, not merely reuse a model alias.

Use `--provider native --native-template-preflight` to enable this mode for the
primary answering/ACON client, and freeze `runtime.native_template_preflight=true`.
Non-native providers refuse the flag. An independent research summarizer profile
may include the optional Boolean `native_template_preflight` separately; its file
digest freezes that choice. Primary opt-in does not silently change an independent
summarizer's mode. A profile requesting template preflight with the approved
`summarize` bridge is refused before initialization, because that bridge builds a
different generation body and does not implement this preflight. No route/default,
role rewrite, download or implicit server start is introduced.

Scripted frozen-protocol validation checks five accounted primary calls with a
separate unchanged CLI summarizer ledger, final profile mutation refusal, and typed
runtime-flag digests. Twelve native, fifteen freeze and nineteen arm tests pass.
These are protocol controls, not additional live-model quality evidence.
Workspace fmt/clippy/tests and deterministic fidelity smoke pass for the CLI
exposure as well; no production/binding defaults changed.

Every attempted accounting stage (including failures before generation) is retained
in `accounting_calls`, with endpoint attempts, timing, body digest and unknown cost.
Research deployment rows and judge receipts retain these separately; economics
summaries expose accounting work without mixing it into inference token usage or
zero-filling cost. Metadata verification/startup and GPU resource use remain separate
qualification requirements; this is not complete economic accounting.

The prospective follow-up in `tmp/native-template-generation-study` froze two
generations on the same exposed USGS TRAIN selection task: one minimal warmup and
one full-source answer. Actual template/gen prompt counts matched **15/15** and
**9,594/9,594**. Startup was 3,789.5764 ms; warmup generation 125.1398 ms, full-source
generation 1,477.4665 ms, and full-source accounting 152.4806 ms. Both requests
completed, consuming 15/2 and 9,594/10 input/output tokens respectively, with unknown
cost. The full-source answer was **wrong** under the predeclared exact task evaluator.
It is retained as counterevidence, not a compression or quality win. Inputs were
stable and the owned server reaped. Native client/runner source snapshots preserve
the original diagnostic versions alongside its protocol/report; no original report
was overwritten. Independent N=1; no held-out selection or inference retry occurred.

The regression suite verifies default refusal remains, caller-list mutation cannot
change the sealed body, malformed counts and prompt-count mismatch halt without
retry, known usage survives rejection, and failed preflight does not fabricate a
generation call. Twelve native, 21 ACON, nine judge and 19 arm tests pass. The first
editing attempt displaced the context-usage check; its existing regression failed
and the check was restored before validation. These are contract controls, not
full five-workload qualification or a proof of summarizer faithfulness.
Workspace fmt/clippy/tests and deterministic fidelity smoke also pass this cycle.

### Resident template/tokenizer accounting diagnostic

`native_model.template_accounting(port, body)` uses bounded, non-retrying client
workers for the native `/apply-template` and `/tokenize` loopback routes. It passes
the caller's complete chat body unchanged, validates a bounded nonempty prompt and
integer token IDs, and reports count plus separate client wall times. No weights,
server, dependency, cache or fallback engine is implicitly loaded. The normal
`NativeModel.chat` bytes-as-token guard is **unchanged**. Server-supplied tokenizer
results are not runtime/weight attestation or an admission gate.

A frozen source-only diagnostic is preserved in
`tmp/native-template-accounting-study/{run.py,protocol.json,report.json}`. It uses
the first filename-order previously exposed USGS TRAIN task, with complete source
and current query only (no gold/evaluator labels). The script explicitly owns one
already-installed b11399 CUDA server and checks the pinned 4B BF16 GGUF digest
`01af8c5cd543e6db94c072eeec58cf9e26e1418c6024d9bf7630e074b5baa04e`,
runtime files, client code and data before/after. 4B here is the declared answering
runtime for this **accounting** diagnostic, not a summarizer escalation. No models
or data were downloaded, and no generation was attempted. The startup readiness
poll policy was predeclared; tokenizer requests were not retried.

Observed owned-server startup: 11,687.2343 ms. First minimal template/tokenizer
warmup: 145.6727 ms/15 tokens. Full training-source template: **9,594 tokens**,
129.6164 ms wall (62.9191 template + 66.6616 tokenizer ms, including lightweight
client processes). The existing conservative JSON-byte bound is **28,227**, so it
would refuse this body at context allowance 16,384, although the returned native
prompt count plus output allowance 512 and margin 1,024 is 11,130. This identifies
a conservative-accounting refusal worth investigating; it does not prove that
generation would match the returned count, succeed, preserve quality, or beat a
comparator. Independent N=1; there are no latency distributions or monetary-cost
claims. Cost stays unknown. Inputs were stable and the server was reaped.

The inspected cached llama.cpp source archive is not asserted to be the compiled
b11399 binary revision; actual runtime files are fingerprinted in the diagnostic
protocol and the API result was validated empirically. Any future opt-in preflight
change must bind the same actual generation body/template/runtime and preserve
fail-closed limits, roles, protected content and all consumed-work accounting.
Eleven native contract tests, workspace fmt/clippy/tests and deterministic fidelity
smoke pass for this change. No production inference preflight or admission gate changed.

### Evaluator-only literal-link coverage

`summary_evidence.literal_link_coverage(task, annotations, payload)` audits an
explicit analyst-defined list of source-stated links after a native frame is
produced. Annotations contain exactly `source_sha256`, `query_sha256` and `links`;
each link has a unique `id`, source `group_id` and nonempty exact `literal` span.
They are bounded to 64 KiB/512 links, bound to the frozen source/query, and
validated against the original group. Duplicate link labels/spans are refused.
Never add these labels to compression groups, protected context, selector scores,
generation/answering prompts or native admission requirements.

The audit reuses the evidence judge's exact compiled-frame validator, including
protected text, source order and source-ID/text authorization. A selected unrelated
paragraph containing the same year does not retain an identity/date link from an
omitted paragraph. Raw source retains all labelled links but is not a summarizer
win. The output reports missing link IDs and literal recall, without returning
the label spans or producing a model verdict. Unsupported narrative can return
null; malformed/unauthorized native evidence fails rather than scoring it.

The original `EvidenceSummaryJudge` rubric is unchanged; its known date-link
blind spot is not declared fixed. Labels may themselves be wrong or incomplete,
and all links can be retained while the generated summary makes unsupported
claims. This is neither semantic completeness/entailment, an official Hotpot
metric, an answering-quality score, nor a production gate. Freeze/evaluate link
annotations only on the evaluator side; never use held-out labels to tune selection.

Windows validation (2026-10-05): three evidence and nine grounded-judge tests pass,
including a date present in unrelated selected evidence but missing its required
group, stale source/query digests, duplicate labels, changed evidence and an
unsupported summary with complete literal retention. The examples are protocol
controls, not observed model-failure rates, measured judge FP/FN, or a workload
win. No model calls or held-out outcomes were evaluated.

### Independent resident research summarizer

For `--provider native --generative-arm`, `--generative-model-profile PATH`
chooses a separate caller-owned resident summarizer without changing the primary
answering model or official ACON backend. The file requires these fields, with only
the optional Boolean `native_template_preflight` permitted in addition:

```json
{"name":"Qwen/Qwen3.5-0.8B","digest":"REPLACE_WITH_DECLARED_64_HEX_PROFILE_DIGEST",
 "port":8001,"max_calls":10,"timeout":25,"output_tokens":1024,"context_tokens":16384}
```

No server is started, weights downloaded/reloaded, or model automatically escalated.
Qwen 2B/4B alternatives require explicit profiles; 0.8B need not answer the raw or
other comparator arms. Same identity (name + digest) must use the existing shared
ledger instead of a separate profile. Per-model native bounds and source-only
generation guards remain active. Independent summarizer `max_calls` must cover
every task; primary `max_calls` reserves its answer calls but not the separate
summarizer's generation. Missing/extra/duplicate profile keys are refused.

Freeze `models.summarizer` as `name@digest`, alongside the existing
`models.answering_and_compressor` (primary answering/ACON identity), and
`runtime.generative_model_profile_sha256` as the profile-file digest. These fields
must match live settings. Final summarizer alias/profile changes invalidate the
run while retaining attempts. Reports include the profile and separate token
ledgers; mixed-model token counts and unknown monetary cost remain null rather
than a fabricated combined total. Profile/alias checks are declarations, not
weights, server startup flags, assistant-prefill or tokenizer attestation.

This option covers the research generated-summary arm and the shipped native
`summarize` CLI research adapter, but not history generation or qualification scope.
Generated prose remains experimental/semantically unverified; raw fallback is not
a summarizer win. No semantic admission or production/binding defaults changed.

Scripted validation (Windows, 2026-10-05) freezes a 4B primary/0.8B summarizer smoke
protocol, retains five primary calls plus one separately charged summary call,
and fails final profile mutation without discarding attempted work. Eleven freeze,
ten native and eighteen arm tests pass; workspace fmt/clippy/tests and deterministic
fidelity smoke pass this cycle. These are protocol/ledger controls, not live-model
quality, latency, economics or fresh held-out evidence.

For `--generative-cli-approval PATH`, the independent profile must match the
approved bridge's `model_revision`, its single explicit `--backend openai`,
`--port` and `--max-output-tokens` arguments. Binding failures occur before model
initialization/inference. The existing helper writes these argument forms; no
executable, approval or server is implicitly created or changed. This independent
CLI path currently supports that native OpenAI-loopback bridge, not the helper's
different resident-Transformers wire protocol. Shared-model approval paths remain
unchanged. CLI inference retains the frozen `generative_cli_timeout_ms` deadline;
profile `timeout` bounds alias verification, not an attestation of server cancellation.

If both generated-summary arms are enabled, the independent summarizer's call
cap must cover both arms per task. Known CLI receipt usage above the independent
profile's output/context allowances is retained and rejected; no text is forwarded
and no retry occurs. Missing usage remains unknown. Usage-bound checks do not
verify server honesty or weights. A scripted frozen 4B-answering/0.8B-CLI test
retains five primary calls plus one independent CLI call, rejects wrong bridge
port before initialization, and invalidates final profile mutation while keeping
attempts. Nineteen arm and ten native tests pass. An initial test asserted the
per-call receipt after a second halted call had cleared it; the corrected test
checks the first call's invalid receipt and retained ledger separately. No live
model quality or held-out study was performed.
Eleven freeze tests, workspace fmt/clippy/tests and deterministic fidelity smoke
also pass this cycle; no production/binding defaults changed.

### Native reported-usage allowance enforcement

The native loopback research client rejects valid integer usage reporting more
completion tokens than the configured output allowance, or prompt + completion
tokens beyond the same model's context allowance. Failed-call usage is retained
with fixed `output_allowance_exceeded`/`context_allowance_exceeded` reasons and
unknown cost. The client halts further calls, does not retry or route elsewhere,
and does not export the rejected completion text. Exact boundary values remain
allowed. Missing/malformed usage retains the existing unknown-usage behavior;
this check does not prove server honesty, exact model tokenization, weight identity,
startup configuration or cancellation. Roles, prompts and model-selection defaults
are unchanged, and mixed-model token units are never added for this check.

Validation (Windows, 2026-10-05): eight native client and eighteen campaign-arm
tests pass, including usage retention, exact-boundary acceptance, no retries and
no rejected text in receipts. The first regression placement accidentally moved
an alias-refusal assertion into the new test; restoring its original test resolved
that assertion failure without changing runtime behavior. No model calls or
held-out outcomes were evaluated.

### Removed duplicate research accounting

The pilot counts each candidate payload once for receipt/budget reporting. A
byte-identical source payload reuses only the scalar count bound to the original
source string; a caller-mutated source cannot reuse it. Answer prompt text,
including wrappers and query, is still counted independently. No tokenizer cache,
daemon, safety-boundary or model-token-unit change was introduced. A regression
reduces count calls from 13 to eight for raw/no-op/changed-payload arms and verifies
unchanged payload counts, budget decisions, wrapped prompt counts and invalid
payload handling. This is reduced bookkeeping, not a compression-quality win.

```sh
uv run --offline --python 3.12 --with jinja2 --with requests --with tiktoken \
  python eval/research/test_acon_benchmark.py --accounting-profile TRAIN_TASK.json
```

Windows observation (2026-10-05), Python 3.12.14/tiktoken 0.14.0/o200k_base:
the existing USGS TRAIN source hash
`be0f32e4f1a10adee29de84c04fa0475c727485afd0b3ed47bf727164fec46ed`
is 22,894 bytes/8,092 tokens. An initial inline diagnostic measured 27.2219 ms
for first encoding and 1.5713/1.6783 ms p50/p95 for the pair of redundant encodes.
The reproducible helper measured 8.9261 ms first encoding and 1.7555/2.5902 ms
p50/p95 for those removed encodes. Both series are retained; there was no model
inference, retry, held-out consumption or favorable-run selection. Each uses
100 repetitions of **one source**, not 100 independent tasks. Replacement reuses
the previously counted scalar; these are removed-work timings, not a measured
end-to-end latency distribution or model-economics advantage. Monetary cost is
unknown, not zero. The profile prints exact task/source digests and runtime versions.
Twenty offline ACON checks, eighteen arm tests, workspace fmt/clippy/tests and
deterministic fidelity smoke pass this cycle; prior validation failures remain
recorded. No production compression or binding behavior changed.

### Multi-step USGS development outcome

The offline USGS importer accepts `--task-mode latest-shallow-moderate` beside
the unchanged `lookup` default. The task filters events by magnitude >= 4.5 and
depth <= 70 km, excludes missing/null measurements, selects the largest event
timestamp, and breaks ties by event ID. It returns the selected ID or `NONE`.
All source metadata/features and conservative snapshot clusters remain intact;
no target/evaluator fields are added to compression groups. Malformed/nonfinite
selection measurements are refused, not interpreted as sensor values.

```sh
python eval/research/import_usgs.py \
  --snapshot tmp/usgs-json-development-20261004/snapshot \
  --manifest tmp/usgs-json-development-20261004/snapshot-manifest.json \
  --output-dir tmp/campaign-usgs-selection-development-20261005 \
  --task-mode latest-shallow-moderate
python eval/research/test_import_usgs.py
```

This is an executable outcome over real API data, but its selection question is
constructed for development and is not a demonstrated representative consumer
workflow or live decision-making task. The existing historical snapshots are
bounded, previously exposed development data; never relabel them fresh held-out
qualification. No-match `NONE` is an inferred outcome rather than a source
substring, so use the existing explicit `--allow-inferred-answers` and matching
frozen runtime declaration when executing such suites. The exact-answer evaluator
must retain no-match failures; do not exclude them or force a match.

Windows validation (2026-10-05): four importer and nine freeze tests pass. The
existing training snapshot imports as one 32-group task; complete reconstruction
and native source/query guards pass, with zero inference calls. Only training
was audited, with no model outcome evaluation or tuning on validation/test data.
The CLI initially lacked `eval` on the diagnostic Python import path; this was
corrected before that source-only audit. Fmt/clippy and fidelity smoke pass;
workspace tests stop at the recorded `LNK1201` writing `tokenfold_py.pdb`.

Pilot economics rows include `stage_wall_ms` for preflight, compression,
payload validation and answering. Failures retain time in the attempted stage;
later/unstarted stages are null, as is raw-arm compression. Compression includes
selection/repack and subprocess overhead; answering includes prompt preparation,
transport and generation. These are client wall times, not server inference times,
GPU resource use, cold/warm labels, or cancellation proof. Existing aggregate
fields and separate evaluator timings retain their historical definitions. Do not
reinterpret historical reports without these fields as zero stage cost.
Aggregate economics now adds `p99_wall_ms` and `stage_latency` with nearest-rank
p50/p95/p99 plus timed/untimed attempt counts. Unstarted stages, historical missing
fields and invalid/nonfinite durations are not zero-filled. All-attempt summaries
retain failed attempts; complete-pair summaries remain separately identified.
A percentile from one or a few attempts is not a reliable tail-latency claim.
Windows validation (2026-10-05): 19 ACON checks and 18 arm tests pass, including
nearest-rank 50/95/99 on 100 declared durations and null/count handling for missing
and failed stages. Workspace fmt/clippy/tests and deterministic fidelity smoke pass
this cycle. The prior linker failure is preserved below; no build-environment fix
or proof of eliminated intermittency is claimed. No new inference was performed.
Validation (Windows, 2026-10-05): 18 offline ACON checks and 18 arm tests pass,
including compression failure with null later stages and answering failure with
retained duration/usage. Fmt, clippy and fidelity smoke pass. The workspace test
attempt stops at linker error `LNK1201` writing `target/debug/deps/tokenfold_py.pdb`;
the E: drive had 348,291,231,744 free bytes, so disk exhaustion was not established.
No inference, retries or downstream quality/economics qualification occurred.

### Headroom recovery wiring audit (not qualification)

The optional native canary exercises the pinned SmartCrusher's full-array CCR
store, Python mirror, and upstream non-destructive store reopen. It uses synthetic
JSON and explicitly sets `lossless_min_savings_ratio=0.99` to force the lossy path;
the receipt records that nondefault config. It does not modify campaign defaults
or turn recovered raw context into an accepted compression result. The audit
compares duplicate-preserving row multisets to source, refuses source-authored
CCR markers and unknown payload wrappers, and returns no recovered text to a model.
The only supported optional trailing wrapper is the upstream tool-digest marker.

```sh
python eval/research/test_campaign_arms.py
python eval/research/test_campaign_arms.py --native-recovery \
  tmp/headroom-campaign tmp/benchmark-0.5.1/latest/Scripts/python.exe \
  tmp/headroom-native-core-20261004/_core.pyd
```

On Windows (2026-10-05), the pinned native artifact
`2cfd5c48feda7f26528aaeda861bb40b1c66e04974e24962035b52c54d0ee324`
at Headroom commit `1cf496612e781ef8d67ff87ee4f78037492f0bc7`
recovered the 50-row full original (1,191 bytes). Native retrieval was 0.0041 ms,
Python mirror retrieval 0.1763 ms and store reopen/retrieval 9.0883 ms in this
single canary; these are not latency distributions or comparative performance.
The first default-config diagnostic on a heterogeneous 200-row fixture failed
the audit with `JSONDecodeError`: the output had a tool-digest suffix and no CCR
marker. It was not a recovery success. A second diagnostic exposed the error type;
the wrapper parser was then bounded explicitly and a separately labelled forced-
lossy fixture used for wiring validation. No answering model or held-out evaluation
was involved. All attempts are reported here, not qualification exclusions.

This proves neither durable **process restart**, cross-request authorization,
model-directed retrieval, default cache behavior, byte-exact reconstruction nor
downstream quality. The normal comparator still reports `unimplemented-ccr-recovery`
and invalidates marker-bearing attempts. Seventeen offline arm tests cover duplicate
rows, missing/foreign rows, mirror misses, wrapper bounds and unchanged safety guards.
The provenance regression, workspace fmt/clippy/tests and deterministic fidelity
smoke also pass this cycle. No production/binding behavior changed.

The canary now also launches a distinct reader **after the writer process exits**
against the same temporary workspace, using upstream default persistence. The
reader emits match/miss metadata only, never stored content. A separately labelled
source-mismatch control returns `hit` but `source_matches=false`; a missing-key
control returns `miss`. This is not an authorization API: source equality does
not confer cross-request ownership or permission to retrieve model-visible text.

Windows observation (2026-10-05), same pinned commit/artifact/config as above:

| Separate reader case | Process wall ms | Store initialization + retrieval ms | Result |
| --- | ---: | ---: | --- |
| Matching source | 531.6405 | 19.4968 | Hit, row multiset matches |
| Different source | 509.1149 | 9.5783 | Hit, row multiset does not match |
| Missing key | 499.1682 | 5.2262 | Miss |

These are single cold-process measurements, not p50/p95/p99, warm-cache or
matched-hardware superiority claims. They identify process/import overhead as
material in this canary; they do not justify removing the bounded worker safety
boundary or adding a new daemon. The separate-process hit proves this synthetic
entry survives writer exit, not restart qualification for every workload, storage
policy, expiry condition or authorized host. Eighteen offline arm checks cover
metadata-only hit/miss/source mismatch and marker refusal. Use the same
`--native-recovery` command to reproduce all cases; no models, retries or downloads.

The research `load_acon` API accepts `history_guideline_path=` in `history` and
`combined` modes. A combined arm can also pass the existing observation
`guideline=`; base modes remain unchanged. History templates are bounded to
64 KiB and allow only literal text and exactly the official `task`, `history`,
`prev_summary` placeholders, retaining `### REASONING` and `### COMPLETED`.
No generated Jinja control flow, calls or attributes are executable. The exact
bytes are fingerprinted in provenance and rechecked before every compression.
This reuses pinned `HistoryOptimizerV2` semantics: raw conversation messages reach
the model directly, and the compression instruction is appended to the last
message; the V2 template rendering supplies only `task`, as upstream does.
No labels are added to those prompts. Scripted integration verifies both
guidelines, role/order preservation, separate calls and refusal of changed bytes
before inference. This API enables frozen training/selection studies; it is not
an automatically trained/selected guideline, a qualified
optimized comparator or agent trajectory evaluation. Custom drivers must freeze
the guideline hashes and account for both combined-mode calls.

For the campaign CLI, add `--acon-history-arms --acon-history-guideline PATH`.
This adds `acon-history-guideline` and `acon-combined-history-guideline` without
changing the official `acon-history`/`acon-combined` bases. The combined external
arm uses the official base observation template, not an external observation
guideline. Freeze `runtime.acon_history_guideline_sha256` and both arm names;
reserve an additional five calls per history task (three per observation-only
task). Template/source/output native guards apply. Changed template bytes fail
before history inference; missing/changed final bytes invalidate provenance while
preserving attempts. No automatic training, selection or quality promotion occurs.

Windows validation (2026-10-05): 16 campaign-arm, 17 ACON, eight freeze and one
provenance offline checks pass. The pinned checkout integration passes with
64 scripted calls per eight-arm run, including an independent final-guideline
removal case that returns failure and retains all seven paired JSONL files
(10 records each). An initial test assertion incorrectly expected a standalone
raw JSONL file; the runner stores raw records inside each comparator's paired
file. The corrected assertion checks the declared arm names and record counts.
These are transport/protocol tests, not live model or held-out quality evidence.
Fmt, clippy and deterministic fidelity smoke pass. Workspace Rust tests were
not repeated for this Python-only cycle; the prior proxy failure remains open.

```sh
# Offline uses already cached dependencies; no model downloads or provider calls.
uv run --offline --python 3.12 --with jinja2 --with requests --with tiktoken \
  python eval/research/test_acon_benchmark.py
# Set TOKENFOLD_BIN to the built CLI; point at the pinned ACON checkout.
uv run --offline --python 3.12 --with jinja2 --with requests --with tiktoken \
  python eval/research/test_campaign_arms.py --acon-root tmp/benchmark-0.5.1/acon
```

A native 4B training comparison selected the first two distinct-component MTRAG
train tasks with history and at most 10,000 source bytes, without gold/outcome
selection. All 22 deployment and 14 separately accounted evaluator calls completed
on one owned resident CUDA server (3.636-second startup); all frozen inputs and
comparators stayed unchanged and the server was reaped. The same-checkpoint
blinded research judge reported 1/2 successes for raw, lossless, ACON observation/
combined and both Headroom defaults, and 0/2 for ACON history. Lossless and
SmartCrusher were byte-identical raw controls. ACON observation payloads were
91/95 tokens versus raw 1,838/1,083: there is no size-superiority evidence.

**History transport compatibility remains unqualified:** both history payloads
contained the original final assistant message and appended compression
instruction. The owner used llama.cpp's
[default assistant-prefill behavior](https://github.com/ggml-org/llama.cpp/blob/b11399/tools/server/README.md),
which continues a final assistant message; the pinned ACON V2 adapter retains
that role. A separate frozen `--no-prefill-assistant` diagnostic is needed before
these failures can be treated as prompt-optimization feedback or qualified ACON
regressions. Do not rewrite upstream roles/prompts, silently strip echoed text,
alter historical outcomes, or infer a causal fix without the diagnostic.
One raw-success training component is insufficient for the CFR floor; the judge
is not independent or official MTRAG scoring. All usage is retained, local costs
remain unknown, and no hosted/validation/test calls or defaults changed.
Evidence: `tmp/native-mtrag-history-training-20261005`, including `analysis.json`.

The predeclared six-call follow-up used the same two TRAIN tasks, official
roles/prompts, weights and decoding limits, changing the owned server to
`--no-prefill-assistant`. All four deployment and two evaluator calls completed;
startup was 4.746 seconds, provenance stayed frozen, and the server was reaped.
Neither history payload retained the final-assistant-plus-instruction prefix.
Payloads changed from 2,110/1,151 to 1,868/984 tokens; judged history outcomes
changed from 0/2 to 1/2. These are diagnostic comparisons with prior runs, not
fresh paired raw controls or held-out non-inferiority evidence.

The corrected second answer also exposes a judge limitation: its conclusion
groups minority interest and preferred stock under subtraction despite the plus
terms in its quoted formula, yet the same-checkpoint judge returned all true.
Keep the scored outcome and source-consistency concern separately; do not turn
the judge's approval into semantic proof. Prompt training must not optimize
against the prior continuation/echo artifact. Native history studies should
explicitly pin `--no-prefill-assistant` in the caller-owned server configuration;
the loopback client neither sets nor attests server startup flags. Independent
judge calibration, source-faithfulness review and optimized/agent qualification
remain pending. Diagnostic evidence is in
`tmp/native-mtrag-history-noprefill-diagnostic-20261005`.

`--bm25-select-arm` adds experimental `tokenfold-bm25-select`. It reuses the existing
research `sel_bm25` (k1=1.5, b=0.75) as caller-declared fallback scores in native
Select, not the separate Rust RAG index or a model scorer. No compression inference
is performed; add one answering call per task to the ceiling. Freeze runtime
`bm25_select_guideline: tokenfold-caller-bm25-native-select-v1`. Caller grouping,
required content, native guards and whole-prompt recount remain enforced. A literal
whole-group witness is checked; ambiguous prefix-overlapping layouts can be
conservatively rejected. The native `scorer unavailable` receipt is intentional
here, not a model runtime failure or proof of quality. Raw fallbacks stay visible.
This lossy research arm is not a default or reviewed Select model runtime.
Check: `python eval/research/test_bm25_select.py` (requires a built native CLI).

The experimental `--readable-logfold-arm` adds `tokenfold-logfold-json` without
replacing the original `tokenfold-lossless` comparison. `readable_logfold.py`
presents native logfold templates as JSON `template_parts` and positional rows,
keeping captured numbers/hex identifiers as strings (including leading zeroes).
Version 2 keeps singleton-template rows as literal source lines and indexes only
repeated templates. This source-only choice never consults questions or answers.
Source ordering, delimiters, CRLF and missing final newlines must independently
reconstruct the exact original bytes and agree with the native CLI decoder.
Native source/output secret guards remain mandatory; malformed, mismatched or
unavailable native recovery is invalid evidence. No extra model call is used to
render or verify the presentation. All schema explanation text counts as payload.

Non-logfold, non-smaller and over-budget presentations visibly fall back to raw
source with a receipt; do not describe passthrough successes as compressed wins.
The arm adds one answering call per task to the declared ceiling. Freeze runtime
`readable_logfold_guideline: tokenfold-logfold-json-presentation-v2`. Native frame
token count, presentation token count, disposition and exact recovery are retained
in the research compression receipt. This is not a new served codec or preset,
and byte reversibility does not prove model readability. Different payload-budget
experiments cannot be silently merged or used as a matched-budget comparison.
Offline checks: `python eval/research/test_readable_logfold.py`.

`import_hotpot.py --input <local-record-array.json> --output-dir <new-scratch-directory>`
adapts a captured [HotpotQA](https://hotpotqa.github.io/) public snapshot (original
record-array or Hugging Face column-style context). Keep dataset attribution and
CC BY-SA 4.0 licensing with adaptations; never commit the downloaded corpus/cache.
All context titles form connected components before deterministic development
splitting. No gold supporting facts or model outcomes drive grouping, protection
or selection. The default cap is 20 records per split, not 20 independent clusters.

For a source-component-distinct pool, add `--one-per-component`: retain the first
SHA256-record-ID-ordered task per component before applying the per-split cap.
Optionally add `--exclude-components <local-json-list>` to exclude previously
exposed component IDs before selection. This requires component-distinct mode;
the list is bounded to 64 KiB and must contain unique IDs from this exact snapshot.
The manifest binds the exclusion bytes and records all excluded IDs. Defaults,
source framing, grouping and split assignments are unchanged; neither gold labels
nor model outcomes choose tasks. Component disjointness is not proof of statistical
independence or absence from model pretraining.

An offline preparation materialized 500 distinct components per split after
conservatively excluding all 213 components previously materialized in v2/v3,
including unconsumed tasks. All 1,500 file digests, literal reconstruction and
selected-title disjointness were audited; three representative native guards
passed. No inference was made and these tasks do not yet supply a raw-success
qualification denominator. The pool is `tmp/campaign-hotpot-component-distinct-v4`,
with the preparation/exclusion audit in
`tmp/hotpot-component-pool-preparation-20261005`. This remains public development
data, not the official hidden HotpotQA test or a representative agent benchmark.

The campaign freezer now refuses a suite split that contradicts a task's explicit
`development_split`. Legacy fixtures without that field remain supported. An
offline audit of 27 existing top-level campaign protocols found four older Hotpot
"training" protocols with split mismatches; preserve their attempts and consumption
as development exposure, not correctly partitioned training/held-out evidence.
One older Loghub protocol also failed its current literal-answer loader contract.
Original reports were not edited or retrospectively relabelled. The v4 pool's
1,500 tasks pass the split-aware freeze (500 test components); a conservative
audit against all Hotpot task files referenced by those discovered protocols found
no selected task-ID or context-title overlap. This scan is not an exhaustive
historical-exposure or pretraining audit. Evidence is retained in
`tmp/campaign-split-audit-20261005`, including an offline smoke pool freeze; no live
run is scheduled and the five-workload qualification requirements remain unchanged.

The first two v4 **training** components have since been used in a separately
frozen Liquid-route baseline study: raw, native lossless, official ACON
AppWorld/QA base prompts, and both pinned Headroom defaults. Nine model calls
completed; the tenth returned HTTP 429, so all subsequent calls stopped without
retry or route fallback. Provenance checks passed, but only one case was valid
across all arms. All answered `Alfred von Schlieffen` against fixed reference
`Alfred Graf von Schlieffen`; keep strict scored failures without claiming a
factual regression. ACON's 30/33-token frames were substantially smaller than the
1,548-token raw/lossless/Smart frames (Universal: 1,326). This is counterevidence
to a size-superiority claim, not an optimized comparator or quality qualification.
The second raw answer was correct, but candidate attempts were rate-limited or
skipped, not valid compression failures. Nine calls reported zero charges; the
failed call's cost/usage remain unknown. Artifacts are in
`tmp/liquid-component-training-baselines-20261005`. V4 validation/test remain
unconsumed by this study; do not retry its failed cases or infer route readiness
from the earlier successful replies.

### Resident native Qwen comparator client (no Ollama)

`native_model.NativeModel` supplies the existing research model interface to an
explicitly started OpenAI-compatible server on `127.0.0.1`. It supports the three
official Qwen3.5 aliases, non-thinking decoding and optional JSON-schema requests.
For a custom frozen study driver, use the same instance for ACON and answering:

```python
from native_model import NativeModel
from acon_benchmark import load_acon
model = NativeModel("Qwen/Qwen3.5-0.8B", declared_profile_sha256,
                    8000, 16, 25, 512, 16384)
compressor, provenance = load_acon(acon_root, model, prompt_family="smolagents")
```

The same client is available as explicit `acon_benchmark.py --provider native`:

```sh
python eval/research/acon_benchmark.py --run-live --acon-root <pinned-acon-checkout> --provider native --native-port 8000 --model Qwen/Qwen3.5-0.8B --model-digest <declared-profile-sha256> --context-tokens 16384 --output-tokens 512 --timeout 25 --max-calls 16 --tasks-dir <guarded-training-tasks> --campaign-split train --output-dir <new-private-study-directory>
```

Start and pin the resident server separately; the command never launches it.
Explicitly use this provider for local Qwen, not the unchanged legacy Ollama
default. Native mode requires the official alias, declared SHA256, a valid port,
at most 30 seconds per client call and at most 16,384 context tokens. Every native
task receives the existing source/query guard before model initialization. Frozen
runtime settings bind the selected port; reports distinguish the declared profile
from weight attestation. A final alias mismatch preserves attempts but returns a
failed run. Other provider defaults and report fields remain unchanged.

Keep the caller's existing native source/query/output guards, and independently
freeze the owned server binary, DLLs, weights, configuration and startup. The
declared digest and `/v1/models` alias check do **not** attest server weights.
The client loads no model, launches no server, discovers no proxies, sends no
credentials, follows no redirects and performs no retries. Every generation,
including identical repeated input, consumes the call budget; usage is retained
when incomplete/mismatched replies are rejected. Local cost remains null. A
killable client bounds trickling HTTP responses, not ongoing server kernels;
any failed generation halts subsequent calls. Conservative byte-based context
preflight never truncates input and is not native-tokenizer qualification.

An owned pinned 0.8B CUDA integration reused one server: two fresh synthetic
READY generations and one raw TRAIN0002 answer completed; official ACON QA
compression then exhausted its 512-output-token cap. The fourth attempt's
reported usage was retained and the compressed student call was skipped. Server
startup was 1.524 seconds, frozen inputs were stable, and the server was reaped.
No rejected reply was retained. This proves the client path works, not summary
quality, comparator qualification or a higher-cap retry recommendation. V4
TRAIN0002 is now development-exposed; validation/test were not called. Evidence
is in `tmp/native-model-integration-20261005`; five offline contracts, including
a real loopback subprocess test, run in CI via `test_native_model.py`.

The actual benchmark command subsequently completed four fresh generations in
one owned native 0.8B server on a synthetic repeated-log training case. Raw and
Tokenfold lossless answered correctly; native lossless reduced the payload from
800 to 189 tokens, while the four-token ACON base frame led to a wrong answer.
Startup was 1.519 seconds, all four model calls reported usage, costs stayed null,
and the server was reaped. This is a one-case CLI integration/regression signal,
not a representative quality or CFR bound, optimized ACON/Headroom comparison,
or justification to select a workload after observing outcomes. Artifacts are in
`tmp/native-benchmark-cli-integration-20261005`; the actual call ledger is in
`live/economics.json` and the disjoint-call audit in `call-accounting.json`.

### Complete optional 4B training comparison

A separately frozen native CUDA 4B study used the next source-ordered v4 TRAIN
components 0003/0004 with a predeclared 1,024-output-token allowance, one resident
server and at most 20 generations. All 20 completed with reported usage; native,
ACON and Headroom provenance checks passed. Startup was 4.142 seconds and the
server was reaped. No hosted, validation/test or retry calls were made. This was
an explicit optional capacity experiment, not a change to the 0.8B default or a
retry of a failed 0.8B request.

Strict answer EM was 1/2 for raw, lossless, both official ACON base prompts and
both Headroom defaults. The structured source-ID generative **research** guideline
(not the native `summarize` command's instruction) scored 2/2, but one correct
answer used exact raw fallback (`not_smaller`). The other accepted summary
retained both annotated supporting documents, yet reversed the parent/subsidiary
relationship: it called Vivendi a subsidiary of Universal Music Group, while the
source says UMG is a subsidiary of Vivendi. A matching downstream answer and full
document coverage therefore do not establish faithfulness or justify promotion.
ACON's 46–86-token frames were smaller than the accepted 238-token generative
frame; Headroom defaults missed the predeclared target on both cases.

Compression-plus-answering model input/output totals were raw 2,694/6, ACON base
3,252/692, ACON QA 3,384/707 and generative 4,891/441. Local costs remain unknown.
Do not infer one-shot total-economics superiority from fewer downstream tokens,
omit fallback generation work, or amortize startup over invented reuse. Only one
common-valid raw-success component is available, far short of the unchanged CFR
qualification floor. Artifacts and posthoc literal/semantic counterevidence are
in `tmp/native-4b-component-training-comparison-20261005`. V4 TRAIN0003/0004 are
now development-exposed; validation/test remain unconsumed by this study.

### Blinded summary-faithfulness diagnostics

`grounded_answer.SummaryJudge` evaluates the **summary text**, separately from
downstream answers and literal-document coverage. Its versioned three-boolean
rubric checks grounding of every claim, query-relevant completeness and preserved
relationships (including direction, subjects, negation and numbers). The model
receives only original context, question and summary: no reference answer,
supporting-fact labels, chosen evidence IDs, receipts or arm identity. Existing
`GroundedJudge` answer inputs, rubric and default metrics remain unchanged.
Both use the same strict schema, native input guards, bounded attempts and
separately retained evaluator usage/cost. No malformed judgment or provider body
is persisted. This Python research API is **not** a production approval gate,
automatic correction, CLI judge option or proof of semantic equivalence.

A pinned resident 4B diagnostic ran eight predeclared evaluator calls: good/bad
synthetic pairs for subsidiary direction, negation and numeric value, plus the
already observed TRAIN0003 parent-inversion summary and a source-grounded
correction. All eight completed and matched the expected grounding/relationship
labels, including rejection of the real summary defect previously masked by a
gold-matching student answer. Labels were kept outside judge prompts. Startup
was 4.095 seconds; inputs stayed frozen and the owned server was reaped.

This small selected calibration is not held-out judge accuracy or a fidelity
qualification. The judge used the same 4B checkpoint as the earlier compressor:
fresh requests and blinded inputs do not establish model/statistical independence.
All eight evaluator usages are accounted separately, local cost remains unknown,
and no generated candidate is promoted. Evidence is in
`tmp/summary-judge-native-calibration-20261005`. Offline answer/summary contracts
run in CI through `test_grounded_answer.py`.

A paired, training-only facts-only instruction hypothesis reused those same
TRAIN0003/0004 cases on one pinned resident 4B CUDA server. All 16 actual calls
completed, with startup measured separately at 4.047 seconds. On TRAIN0003,
the existing summary reproduced the parent-direction inversion despite its
gold-matching answer; the facts-only summary preserved the stated relationship
and passed the source-only judge, but its student answer failed fixed-label EM.
TRAIN0004 fell back to the original context for `not_smaller` in both arms.
The built-in lossless/ACON slots were deliberately no-op raw controls, **not**
compressor comparators. This two-case hypothesis does not establish independent
judge accuracy, downstream non-inferiority or superiority; no production prompt
or default is changed. All generation, answer and evaluator usage is retained,
local cost remains unknown, and no validation/test or hosted calls were made.
Frozen inputs, receipts and the reaped-server record are in
`tmp/source-facts-only-training-hypothesis-20261005`.

The manifest fingerprints input/implementation/output and records overlap limits.
Public validation is not official held-out test data, and model contamination is
unknown. This adds retrieval QA only, not all-workload or agent qualification.

HotpotQA includes inferred answers such as yes/no that need not appear literally
in source. Use the explicit research `--allow-inferred-answers` flag and declare
`"allow_inferred_answers": true` in a frozen protocol's `runtime`. The shared
loader's existing literal-answer default remains unchanged. Exact-string pilot
scoring is the unchanged default. Explicit `--answer-evaluator hotpot-answer-v1`
adds the [HotpotQA v1 answer normalization and EM/F1 rules](https://github.com/hotpotqa/hotpot/blob/master/hotpot_evaluate_v1.py)
with per-attempt answer metrics and EM-based outcomes. Declare
`"answer_evaluator": "hotpot-answer-v1"` in the frozen `runtime` for that run.
This does not score supporting facts/joint metrics or qualify an official benchmark
submission. Never rescore old evidence silently or mix exact-string and normalized runs.

Local development replay can now run against an already-installed Ollama generation
model without hosted export. The first five-workload local replay did not qualify:
provider/tokenizer failures left invalid pairs, lossless recovery did not imply
model-readable answers, and generated-summary validation fell back. Raw and healthy
Headroom answers succeeded on these development probes. Preserve failed attempts;
do not discard them or present the offline token savings as live superiority.
An explicit logfold decoder-instruction probe restored some answers but still
produced a wrong answer and a provider failure. Decoder hints alone are not a
qualified fix. Count their prompt overhead; native byte-exact recovery and
model interpretation are separate acceptance gates.

- `acon_benchmark.py` starts a **synthetic observation-only pilot**, not an official
  ACON QA/AppWorld or agent benchmark. It compares raw, Tokenfold lossless and the
  official ACON `ObservationOptimizer` with base AppWorld prompts, including its
  system message. ACON is imported only by this research tool; nothing is bundled
  into the CLI, proxy or bindings. Tokenfold generated-history compaction and
  full official ACON agent training/validation qualification remains incomplete. Official base history/combined text
  replay and an unverified generated-observation arm are now opt-in research paths;
  they are not independent agent trajectories or production semantic strategies.
  Model-ranked extraction is also available as an experimental arm, described below.

  Add `--acon-observation-guideline <public-prompt.jinja>` to compare an externally
  trained observation template as a separate `acon-observation-guideline` arm.
  For a guideline trained from the official QA template, explicitly add
  `--acon-guideline-prompt-family smolagents`; its upstream QA files and family
  are pinned in provenance and the nondefault family is frozen in runtime field
  `acon_observation_guideline_prompt_family`. The default remains `appworld`;
  selecting a family without a guideline is refused before model initialization.
  The official base arm and system prompt remain unchanged. The file is bounded
  to 64 KiB and permits only literal text plus exactly one each of the original
  `task`, `history`, and `observation` placeholders; template calls, expressions,
  filters, imports and control flow are rejected before inference. Native secret
  guards also preflight it. This adds two calls per task to the declared ceiling.
  Freeze its exact byte hash in runtime field `acon_observation_guideline_sha256`.
  A changed/unavailable guideline invalidates provenance but preserves attempts.
  Supplying a file does **not** establish fair optimization: retain training-only
  regression analysis, official optimizer inputs/outputs, validation selection and
  untouched test evidence separately. The report deliberately labels such an arm
  `external-guideline-not-qualified-optimized` until that evidence exists. No
  history/combined guideline override or production default is enabled.

  A bounded hosted training adapter has now exercised the pinned official
  regression and prompt-update templates on two original synthetic development
  regressions, producing two distinct byte-pinned observation variants. These
  are QA-snapshot adaptations, not the official agent-training benchmark, and
  still need validation selection and untouched held-out evaluation. Teacher
  critiques may contain nonliteral spans under the upstream analysis contract;
  record them as unverified critiques, never production semantic evidence.
  Preserve prior invalid adapter attempts and include their inference usage in
  training economics. Template guards and reported zero billed cost do not
  establish quality or total-economics superiority.

  An October 5 QA-specific follow-up used all raw-success/QA-ACON-failure pairs
  from the existing four-task public Hotpot **training** pilot: exactly one pair.
  Its source, query and observed answers (not gold) were rendered through the
  pinned official regression template, then its accepted critique through the
  official prompt-update template with the original `smolagents/prompt_obs`.
  The approved, descriptor-pinned zero-price Nemotron route completed all three
  declared calls: one critique and two distinct guarded guideline variants.
  Their hashes are `3fa8905f7eb5cbd4967f0605f2a32f0f908a548345a8b178ffed0b987bf412d1`
  and `f2ed56ecf07822614a47d654cfd09ff0aaafe710f928fc84c73f3d1f5cb6acb0`.
  All frozen inputs remained stable. The calls reported 4,750 input / 3,030
  output tokens and zero billed cost; earlier development attempts remain separate
  economics, not free amortized training. This is snapshot-adapted training on one
  regression, not a qualified optimized baseline or full agent training.
  Artifacts are in `tmp/acon-qa-official-template-training-20261005`, with its
  original inputs in `tmp/acon-qa-training-regression-inputs-20261005`.
  Validation selection and untouched held-out quality remain separate requirements.

  Its bounded QA validation selector subsequently terminated with **no selected
  guideline**: 18 of at most 20 declared calls were attempted, nine failed, and
  there were no common-valid tasks across all arms. Provenance remained stable.
  Known usage was 12,390 input / 4,168 output tokens; nine calls had unknown usage
  and cost. Known billed charges were zero, not a known-zero total. The one valid
  raw-success/official-QA-base pair regressed. These two previously consumed
  source-order validation tasks cannot establish held-out quality, and incomplete
  evidence must not select a variant or justify superiority. Attempt journals,
  paired records and economics remain in
  `tmp/acon-qa-guideline-validation-selection-20261005`; no failed call was retried.

  **Development campaign and optional arms.**

  ```sh
  python eval/research/build_campaign_corpus.py --output-dir tmp/campaign-development
  python eval/research/test_campaign_arms.py
  uv run --python 3.12 --with jinja2 --with requests --with tiktoken \
    python eval/research/test_campaign_arms.py --acon-root tmp/acon
  ```

  The generator creates train/validation/test snapshots across JSON, logs, code,
  retrieval and text-history replay. It deliberately assigns one template cluster
  per workload/split, not one independent subject per generated instance. These
  are synthetic development probes, not a representative held-out qualification
  corpus. A freeze suite can use `"workload": "mixed"` when each task declares its
  own supported `workload`. No generated fixture or benchmark cache is published.

  Add `--acon-history-arms` to the pilot for official `HistoryOptimizerV2` using
  `prompt_history_v2` and a combined history-then-observation arm. A task's
  `history_messages`/`observation` must reconstruct its source exactly; only text
  user/assistant messages are supported. Tasks without history are visible no-ops
  for the history arm. This is transcript replay, not a real agent/tool protocol,
  threshold policy, multi-update summary or optimized-guideline comparison.

  Add `--generative-arm` for the experimental `tokenfold-generative` arm. Add
  optional `--generative-structured` to request the reply schema, including authorized
  citation IDs. Both non-structured research-v3 and structured-v4 request self-contained
  facts and relationships rather than bare answers. The payload now contains the
  `summary_unverified` and complete literal `source_evidence` groups for cited IDs,
  deduplicated in source order. This retains source relationships that short quotes
  may omit; it does not verify the summary or prove all needed groups were cited.
  Evidence and wrapper text count against the payload budget, with visible raw
  fallback if oversized or not smaller. Protected caller text remains outside this
  envelope unchanged. Older studies retain their original versions and failures. Local
  [Ollama](https://github.com/ollama/ollama/blob/main/docs/api.md) uses `format`.
  Approved hosted endpoints must explicitly advertise `structured_outputs`; the
  [OpenRouter request](https://openrouter.ai/docs/guides/features/structured-outputs)
  uses strict JSON Schema with `require_parameters`, still pinning the provider and
  zero prices with no fallback. Ordinary answering/ACON requests are unchanged.
  Schema requests do not establish source entailment. Native guards and literal quote
  checks still run, failures remain invalid and metered, and receipts expose only
  fixed failure-stage labels, never rejected replies or provider exception text.
  The arm uses the same approved bounded model transport, invokes native secret guards on source,
  query, grouping metadata and emitted text, keeps caller-protected text outside
  generation, and requires exact-source citation quotes. No savings/over-budget
  candidates keep the authorized baseline; failed inference/schema/citation checks
  keep that baseline but mark the attempt invalid, never a compression win. Receipts
  label admitted paraphrases **semantically unverified**. Attribution is not entailment;
  this arm cannot pass the production literal-extract semantic API. No extra verifier
  model is invoked and no production runtime/default is changed.

  A hosted stop condition (including HTTP 429) is retained as `model_stop_reason`
  in the report and subsequent refused calls retain that reason, not a misleading
  budget-exhaustion label. A child Select stop also stops parent inference. Nothing
  retries, changes keys, enables paid requests or switches providers after a stop.
  Skipped compressors have null selection/compression receipts; a previous task's
  receipt must never be counted as a fallback or citation for a refused attempt.

  Add all three of `--headroom-root`, `--headroom-revision` (full commit) and
  `--headroom-python` (existing isolated environment) for the pinned
  `headroom-universal-default` arm. It runs in a bounded fresh worker with cleared
  credentials, isolated local state and cache-only Hugging Face loading. A missing
  cached model, unfinished/degraded canary, Kompress warning or truncation fallback
  is invalid evidence rather than an easy comparator loss. It uses the universal API,
  not Headroom's query-aware proxy/SmartCrusher or retrieval. Cold process/model-load
  times must not be advertised as intrinsic library latency. No automatic dependency
  installation or model download is performed.

  Additionally pass `--headroom-smart-core PATH` to a prebuilt native extension
  from the same pinned checkout to add `headroom-smartcrusher-default` alongside
  the universal arm. It applies upstream default SmartCrusher to a query plus
  tool-response snapshot, preserving tool protocol and digest markers. The native
  artifact SHA is frozen in runtime settings and checked before loading and after
  the run. This adds one answer call per task. Source/query native secret guards
  run before hosted calls. Warnings or CCR retrieval markers are invalid evidence,
  not comparator losses: this runner does not implement CCR recovery. Defaults
  (including upstream default relevance and gpt-4o counting) are unchanged;
  payload targets are not matched budgets for this arm. Cold worker latency,
  default embedding availability and tool-snapshot results do not qualify the
  full Headroom proxy/cache/retrieval system. No weights are downloaded.
  Optional offline native canary (requires an already built artifact/environment):

  ```sh
  python eval/research/test_campaign_arms.py --native-smart HEADROOM_ROOT HEADROOM_PYTHON NATIVE_CORE
  ```

  A frozen repeated-TRAINING probe on three Loghub first-window JSON projections
  (Apache/Linux/Spark) retained all three answers with native lossless, BM25 Select,
  generated-summary v3 and both Headroom arms. Total provider input+output tokens
  were respectively 9,656 / 6,630 / 19,315 / 12,980 (universal) / 12,045
  (SmartCrusher). BM25 used about 45% fewer inference tokens than SmartCrusher
  on these probes; generation's compression overhead still lost economically.
  Raw Spark and ACON Apache had provider errors (unknown usage/cost), so the
  26-call study is incomplete, not qualification. Native payloads all exceeded
  the requested 50% target; Headroom defaults were not matched budgets. All
  provenance checks passed. These are three repeated training source families,
  not independent held-out agent workloads or full Headroom recovery evidence.

  An unchanged-arm TEST probe then used the first HDFS/Windows windows in both
  projections (four tasks, two held-out families; train/validation/test source
  clusters frozen disjointly). Raw, universal Headroom and BM25 Select answered
  4/4; BM25 used 11,829 total inference tokens versus universal's 22,338 (~47%
  fewer). The other arms had incomplete responses or wrong answers, so the
  35-call overall study did not qualify. SmartCrusher's two incomplete answers
  remain invalid evidence, not comparator losses. Native and SmartCrusher also
  answered one Windows raw-log query incorrectly despite byte-identical raw
  payloads: hosted response nondeterminism, not demonstrated compression-caused
  loss. Preserve these attempts and conservative observed CFR; do not retry or
  exclude them post hoc. Zero observed BM25 regressions over two source clusters
  still gives a 95% CFR upper bound of ~77.6%, far above the 1% promotion target.

  Repeated HotpotQA TRAINING probes exposed a separate BM25 limitation: it could
  drop an explicitly named subject paragraph in favor of high-frequency lexical
  distractors. Raw/both Headroom arms achieved 2/4 normalized EM; BM25 1/4 with
  one raw-success regression. Generation had two invalid attempts; no promotion.
  `--bm25-heading-arm` adds a separate development variant that boosts exact,
  token-normalized title phrases named in the query, only for `[document N] Title`
  paragraph framing. It reuses native whole-group selection and existing guards;
  it does not inspect gold, infer entailment, reserve new required groups or prove
  that all multi-hop evidence survives. Original BM25 results remain unchanged.
  Frozen runtime guideline: `tokenfold-caller-bm25-heading-native-select-v1`.
  One additional answer call per task; no compression-model inference. Native
  regression tests verify two named subjects can fit, not downstream qualification.

  Live heading-aware v1 testing on the same four TRAINING tasks completed all 32
  calls with stable provenance and no invalid attempts. It fixed the named-director
  comparison (UNKNOWN to yes), but introduced a Yellowcraig/Yellowcraigs exact-match
  regression. Heading/original BM25 each achieved 1/4 normalized EM versus raw and
  both Headroom arms at 2/4. Heading used 3,616 total inference tokens, original
  3,916, universal 5,864 and SmartCrusher 6,402; these savings do not compensate
  for unqualified quality. Retaining named subjects is not sufficient evidence of
  superiority. Keep the variant experimental; do not relax the evaluator or discard
  the new regression. The generative summarizer remains separately unqualified.

  Broader unchanged-arm validation on eight previously uncalled Hotpot development
  tasks (eight source-title components) rejected both 50%-budget BM25 variants:
  each achieved 2/8 normalized EM versus raw/native/both Headroom arms at 3/8,
  with one raw-success regression each. Heading boosts did not improve mean F1
  (both ~0.417). One ACON incomplete response made the 64-call study incomplete;
  all provenance checks passed and all reported usage/cost was retained. These
  results do not justify a new test/promotion run at this operating point. Return
  to training for budget/quality calibration; do not relax the evaluator, tune on
  this validation set or conceal regressions behind inference-token savings.

  A separate frozen 75%-retention calibration on the four repeated TRAINING tasks
  completed all 32 calls with stable provenance. Original BM25 matched raw/both
  Headroom arms at 2/4 normalized EM and mean F1 ~0.792, with zero observed
  raw-success regressions, fallbacks or budget misses. Its 5,129 total inference
  tokens were below universal's 5,879 and SmartCrusher's 6,275. The predeclared
  training rule selected original BM25 only as a hypothesis for new validation;
  heading-aware F1 (~0.750) failed the same rule despite lower token usage.
  This does not qualify 75% pruning, erase the failed 50% studies or establish
  statistical non-inferiority. Headroom still used defaults, not matched budgets.

  Fresh 75%-retention validation on eight additional source-title components failed
  the predeclared development rule: BM25 6/8 normalized EM versus raw/SmartCrusher
  7/8, mean F1 ~0.833 versus ~0.958, with one raw-success regression. All 56 calls
  completed validly with stable provenance. Its 11,648 inference tokens versus
  universal's 14,589 and SmartCrusher's 15,447 do not compensate for failed quality.
  Neither 50% nor 75% retrieval pruning is qualified by these studies.

  `--acon-qa-observation-arm` adds `acon-qa-observation`, using the pinned official
  `experiments/smolagents/prompts/context_opt/prompt_obs.jinja` with the unchanged
  official optimizer and system prompt. The explicit template-name mapping is
  necessary because that file is not named `prompt_user.jinja`; no missing-template
  fallback is used. Keep the AppWorld base arm alongside it. Freeze runtime
  `acon_qa_observation_prompt: smolagents/prompt_obs`; two additional model calls
  per task cover compression and answering. Source/query/output native guards and
  post-run QA-source/prompt hashes apply. This is a QA-specific **base** prompt,
  not trained/validation-selected optimized ACON or its live retrieval agent.

  A frozen live comparison on four repeated Hotpot TRAINING tasks confirmed the
  QA-specific base arm runs validly with stable prompt provenance (1/4 EM, same
  as AppWorld base; both Headroom arms/raw 2/4). Generated-summary v3 had three
  invalid attempts, including two nonliteral quotation claims. Preserve these
  41 calls and their costs; no competitor or candidate promotion follows.

  `--generative-source-ids` (requires `--generative-arm`) selects a separate
  experimental schema: `summary` plus a nonempty list of exact authorized
  `source_ids`. Native code compiles complete literal source citations, then
  reuses the existing protected-content, source-order evidence, secret, no-growth
  and token-budget gates. Unknown/protected IDs, extra fields and malformed
  replies fail closed; duplicate IDs are deduplicated. Models do not author the
  evidence quotations in this mode. Free-form summary semantics and citation
  completeness remain unverified; this is not a weaker production admission rule.
  Original quote-checking v3/structured-v4 contracts and studies stay unchanged.
  Frozen guideline: `tokenfold-generated-observation-source-ids-v1`; also freeze
  `generative_source_ids_structured` as true/false. Optional structured mode
  uses authorized-ID enums only on already supported endpoints; no route fallback.
  There is still one compression call per task, and complete evidence is budgeted.

  A frozen source-ID study on the same four repeated Hotpot TRAINING tasks
  completed 43 calls with stable provenance but remained incomplete: one
  generation reached the 1,536-token output limit and was retained as invalid.
  Three completed generations passed native evidence attachment and budget gates;
  normalized EM outcomes were one success and two failures, not a quality win.
  Total generative inference usage was 11,816 tokens, including the failed call,
  versus Headroom universal's 5,698 and SmartCrusher's 6,452 (each 2/4 EM).
  All reported usage/cost was available with zero provider charges. Source-ID
  compilation addresses model-authored quotation errors, not output-limit
  reliability, paraphrase fidelity or compression-call overhead. No promotion or
  independent validation follows this failed training result; old failures remain.

  A separate supported structured-output route study used Liquid
  `lfm-2.5-2.6b:free` for every arm on the same four TRAINING tasks. All four
  source-ID generations passed schema/evidence/budget checks (2/4 EM), but their
  12,698 total inference tokens exceeded universal's 7,233 and SmartCrusher's
  7,561. Raw and universal each had one incomplete answering call; all 44 calls
  were retained with stable provenance and known zero provider charges. The
  predeclared advancement rule failed. Route and schema changed jointly, so
  reliability differences are not isolated causal evidence or qualification.

  Declare all enabled arm names and reserve their complete call budget before a
  frozen run: raw/lossless/observation use four model calls per task; Select and
  generation each add at most two; Headroom adds one answer call; history/combined
  add five calls on a replay task and three on a task without history. Hosted
  export still requires the existing explicit public-data flag; paid routes stay
  refused. Quality reports now supplement descriptive bootstrap intervals with a
  one-sided exact binomial CFR upper bound over declared task clusters. Zero events
  on six raw-success clusters give a nonzero bound, not certainty. Binomial bounds
  assume independent sampled clusters; synthetic declarations do not establish that.
  Economics totals retain invalid attempts, native compression-plus-answering usage,
  unknown billed costs and generated-summary fallbacks.

  Offline contract checks need only Python; optional official-class wiring checks
  require the pinned checkout and isolated dependencies, but make no model calls:

  ```sh
  python eval/research/test_acon_benchmark.py
  git clone https://github.com/microsoft/acon.git tmp/acon
  git -C tmp/acon checkout d63f9ae18959dc7215ff62899c94c5e8c56847ae
  uv run --python 3.12 --with jinja2 --with requests --with tiktoken \
    python eval/research/test_acon_benchmark.py --acon-root tmp/acon
  ```

  Ollama execution is explicit and **local-only**. Supply an already-installed
  completion model's exact name and 64-character digest from Ollama `/api/tags`.
  No downloads, credentials, environment proxies, redirects or paid APIs are used.
  Use a trusted loopback Ollama daemon with cloud features disabled; remote models
  and embedding-only models are refused. Only use public/synthetic task fixtures;
  this harness is not a redaction gateway. The same pinned model answers every arm
  and compresses the ACON arm. Gold answers and critical atoms are not sent to it.

  ```sh
  cargo build --release --locked -p tokenfold-cli
  uv run --python 3.12 --with jinja2 --with requests --with tiktoken \
    python eval/research/acon_benchmark.py --run-live --acon-root tmp/acon \
    --model YOUR_INSTALLED_MODEL --model-digest YOUR_64_HEX_DIGEST \
    --context-tokens 32768 --output-tokens 512 --max-calls 12 --timeout 120 \
    --output-dir tmp/acon-pilot
  ```

  The output directory must be empty. Each candidate has its own compatible
  raw/candidate JSONL file; never concatenate them or count the shared raw answer
  twice. `report.json` records source/prompt/binary/dataset hashes and settings;
  `economics.json` records all compression and answering calls, failed attempts,
  wall times, no-ops and target misses. Unavailable usage/billed cost is `null`, not
  zero. Exact `o200k_base` text counts are not native model context or billing
  counts. Conservative byte-based context preflight is not tokenizer/truncation
  qualification. ACON is not given an invented budget prompt: its output is measured
  against the requested ratio, not assumed to meet it. Failures are invalid evidence,
  not compressor wins, and cause a nonzero exit. Calls are not retried. A timeout
  kills/disconnects the HTTP client, not necessarily daemon-side inference.
  Exact-answer scoring and three tasks are smoke evidence only; even perfect pilot
  results or zero-width bootstrap bounds cannot qualify production defaults.
  Implementation hashes are captured before inference; changes during a run cause
  a nonzero exit and disqualify its provenance rather than silently hashing new code.

  **Hosted model-ranked pilot (explicit public-data export).** Only
  `stealth/space-bunny-alpha`, `nvidia/nemotron-3-ultra-550b-a55b:free`,
  `nvidia/nemotron-3.5-lightning:free` and `qwen/qwen3.8-27b:free` are
  approved. Supply `OPENROUTER_API_KEY` (or lowercase `openrouter_api_key`) in the
  explicitly selected `.env` file; the parser does not expand shell expressions or
  load other variables. The key is sent only to OpenRouter's HTTPS chat endpoint,
  never in command arguments, output files or scorer replies. Provider metadata
  must show zero prices before every request; routing additionally sets maximum
  prompt/completion/request/image prices to zero and disables provider fallback.
  Unexpected reported charges stop future calls; missing usage/cost stays unknown.
  No paid-model support or substitution is implemented. Only native OS `SystemRoot`
  is reconstructed before Windows networking initialization in the environment-cleared
  child; credentials are not inherited.

  ```sh
  python eval/research/test_openrouter_model.py
  uv run --python 3.12 --with jinja2 --with requests --with tiktoken \
    python eval/research/acon_benchmark.py --run-live --provider openrouter \
    --allow-hosted-public-data --env-file .env --select-arm --acon-root tmp/acon \
    --model stealth/space-bunny-alpha --tasks-dir eval/research/acon_grouped_corpus \
    --ratio 0.25 --context-tokens 32768 --output-tokens 1536 \
    --max-calls 36 --timeout 90 --output-dir tmp/acon-hosted-pilot
  ```

  Repeat with another approved model in a **new** output directory. The six grouped tasks are a
  predeclared synthetic development pilot, not a held-out test set. Their declared
  groups reconstruct the original source exactly; the required rule is caller-protected,
  not selected using gold answers. Output may be source fragments, not valid JSON;
  do not use this text selector where a structured-wire protocol is required.
  The runtime's scoring guideline is `tokenfold-source-rank-v1`. It sends only the
  query and authorized groups, requires exactly one bounded numeric score per source
  ID, and returns only scores. Existing Rust Select validates IDs/revision, preserves
  required groups and source order, and recounts the complete emitted context.
  Scorer failures retain the existing declared fallback policy; the experiment
  marks these attempts invalid rather than calling them model-quality successes.
  The shipped runtime deadline remains at most 30 seconds, even when answering
  requests have a longer client deadline. Slow hosted models can therefore be
  unsuitable scorers, and that must not be reported as a compression win.

  `select-runtime/approval.json` is also an example of the existing explicitly
  approved scorer configuration: Python executable/hash, script arguments, model
  descriptor revision, authorized credential file and scratch directory. To use
  it outside this pilot, review approval/data scope and protect the interpreter,
  auxiliary scripts, credential file and scratch/metrics paths against modification;
  it is not an OS sandbox. Never commit that local configuration. There is one
  reserved model call per invocation and no child descendants. This is an experimental
  research runtime, not a default hosted integration or an ACON-style generated summary.

  Hosted hashes pin endpoint **metadata**, not immutable model weights. Record actual
  provider usage including hidden reasoning; reasoning is requested at low effort
  and excluded from visible text, not disabled (Space Bunny requires reasoning).
  Seeds are sent only when supported, so even temperature zero is not guaranteed
  deterministic. `economics.json` now retains emitted payloads for public-data
  regression attribution as well as answers, no-ops, target misses and call records.
  Measure native compression-plus-answering tokens, not just context savings. Keep
  invalid attempts and unmatched pairs visible; do not cherry-pick valid rows or
  combine repeated development runs into independent statistical evidence.

  These free endpoints have retention/logging restrictions: send **no private or
  personal data**. See [Space Bunny's model page](https://openrouter.ai/stealth/space-bunny-alpha)
  and [Nemotron's model page](https://openrouter.ai/nvidia/nemotron-3-ultra-550b-a55b:free).
  The additional approved routes are
  [Nemotron 3.5 Lightning](https://openrouter.ai/nvidia/nemotron-3.5-lightning:free)
  (`nvidia/nvfp4`) and [Qwen3.8 27B](https://openrouter.ai/qwen/qwen3.8-27b:free)
  (`modelrun/fp4`); exact provider tags are pinned, not automatically selected.
  A further seven zero-price routes were added after a live smoke pass on the frozen
  holdout corpus: [Nemotron 3 Super](https://openrouter.ai/nvidia/nemotron-3-super-120b-a12b:free)
  (`nvidia`), [Nemotron 3 Nano Omni](https://openrouter.ai/nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free)
  (`nvidia`), [Dots3-Note Preview](https://openrouter.ai/dots-studio/dots-3-note-preview:free)
  (`atlas-cloud/fp8`), [Laguna S 2.1](https://openrouter.ai/poolside/laguna-s-2.1:free)
  (`poolside/fp4`), [Laguna XS 2.1](https://openrouter.ai/poolside/laguna-xs-2.1:free)
  (`poolside/fp8`), [North Mini Code](https://openrouter.ai/cohere/north-mini-code:free)
  (`cohere`) and [LFM2.5-2.6B](https://openrouter.ai/liquid/lfm-2.5-2.6b:free)
  (`liquid/fp8`). Being listed does not imply the endpoint answered: Inkling and Ling
  return HTTP 403, Gemma returns HTTP 429, Apodex returns `finish_reason: length`, and
  both Laguna routes return a single HTTP 429 that exhausts the call budget for every
  arm in the run. LFM answers the scorer question but wraps its priority list in a
  Markdown fence, so the strict scorer schema rejects it. These are transport and
  format facts, not compression-quality evidence.
  Add `--select-lossless` alongside `--select-arm` to replace that arm with
  `tokenfold-model-select-lossless`: model selection followed by native lossless
  repacking. It retains the original selected text unless a strictly smaller
  exact-token-count payload decodes byte-for-byte to that text. Repacking uses
  no additional inference calls; selection receipts still describe the original
  selected groups, while `selected_payload_tokens` and its hash record that
  intermediate text separately from the final compressed payload. This remains
  experimental: exact recovery does not prove a downstream model understands
  the frame. Score live answers before treating it as a quality-preserving win.
  HTTP 429 rate limits stop remaining inference in the run (including failures
  reported by the scorer child); there is no automatic retry or provider switch.
  Space Bunny is scheduled to disappear October 5, 2026; it is not a durable default.

  **Lower-overhead scorer:** add `--compact-scorer` with `--select-arm`. This keeps
  the legacy numeric scorer available but switches the experiment to a compact
  source-priority guideline. JSON-object values are packed into common fields and
  column/value rows; non-object fragments and ambiguous duplicate-key JSON remain
  literal. Decimal/exponent numbers and signed zero also remain literal rather than
  being rounded through binary float parsing. An explicit source-order list retains
  ordering across different tables and literals. This guideline is
  `tokenfold-source-priority-v3`. The model replies only with needed source IDs in priority order. Reject
  unknown/duplicate IDs, empty lists and extra rewritten-text fields; expand validated
  priorities to the unchanged complete numeric-score runtime response internally.
  Only the inference representation changes: Rust still assembles original text,
  retains caller-required groups and recounts the complete output. Packing is not
  semantic-quality proof and does not authorize redacting or rewriting source facts.

  `acon_holdout_corpus` contains six fresh, fixed synthetic instances (seed 70413)
  in the known task families, with shuffled source positions and opaque group IDs.
  It was frozen before first inference; no gold answers are used by the scorer.
  Use a new output directory and the same 36-call cap for each model. This is a
  held-out **smoke** set, not official ACON data or broad promotion evidence. Never
  tune the guideline using its observed errors and still call it held-out.

- `prompt_cache.py` measures whether a proposed output preserves a declared byte prefix and models
  repeated-input cost using caller-supplied prices. Use this before proposing a real cache API.
- `near_dedup.py` reports likely duplicate JSONL records using deterministic token-set Jaccard
  similarity. It never deletes or rewrites data.
- `toon_benchmark.py` compares compact JSON, Tokenfold, and the pinned official TOON CLI on
  caller-supplied JSON files. It reports `o200k_base` tokens (when `tiktoken` is installed),
  encoded bytes, median encode/decode latency, determinism, and value/round-trip checks across a
  versioned seven-case project corpus, including a flat projection where TOON should be strongest.
  Run it from the repository root:

  ```sh
  npm ci --prefix eval/research
  uv run --with tiktoken python eval/research/toon_benchmark.py --require-exact \
    --manifest eval/research/toon_corpus/manifest.json \
    --tokenfold-revision c1c2c8fc4cb7284a96a1fb52086bc9bc01541989 \
    --output eval/research/toon_results.json
  ```

  The harness uses the local `@toon-format/cli@4.1.1` research dependency, falling back to `npx`
  when it is absent. It does not add TOON to a served path or require a model.
- `provider_benchmark.py` compares Tokenfold with Headroom's default local generic-JSON API on
  the same six versioned inputs, counts both outputs with `o200k_base`, and verifies recovered
  values. Headroom remains an isolated research dependency:

  ```sh
  cargo build --release --locked -p tokenfold-cli
  git clone https://github.com/headroomlabs-ai/headroom.git tmp/headroom
  git -C tmp/headroom checkout 4c9c29c421224920dee682a0cb0c688c1c71e64e
  uv run --python 3.12 \
    --with-editable "tmp/headroom[proxy]" --with tiktoken \
    python eval/research/provider_benchmark.py \
    --manifest eval/research/provider_corpus/manifest.json \
    --headroom-revision 4c9c29c421224920dee682a0cb0c688c1c71e64e \
    --headroom-root tmp/headroom \
    --tokenfold-revision "$(git rev-parse HEAD)" \
    --output tmp/provider-results.json
  ```

  The checked-in report is the evidence used by the README comparison. This is a local API
  comparison, not a claim about either hosted proxy.
  New runs require a clean pinned comparator checkout and verify the imported module
  belongs to it. Tracked comparator files, fixture bytes and the Tokenfold binary are
  hashed; comparator/binary changes during execution refuse evidence publication.
  Output files cannot be overwritten. The binary hash is not build/source attestation;
  dirty Tokenfold worktrees are explicitly reported. Historical reports are unchanged.
  Offline provenance regression: `python eval/research/test_benchmark_provenance.py`.
- Learned pruning experiments remain in `eval/learned/`; production integration stays blocked on
  a completed `eval/tasks/v04/HUMAN_AUDIT.md` and evaluation provenance review.

These small tools establish evidence cheaply; add embeddings, provider APIs, or a served selector
only when the deterministic baselines show a measurable gap they can close.

## Public real-log development corpus

`import_loghub.py` adapts an offline [Loghub](https://github.com/logpai/loghub)
snapshot into numbered raw-log windows and JSON projections of its structured CSV.
The snapshot manifest must pin the official repository revision and each file's
SHA-256 and commit-specific download URL. Raw line counts, row IDs and literal
Content messages must align. The importer excludes EventId/EventTemplate parsing
labels, preserves complete rows, and selects lookup questions in source order,
without model-outcome filtering. It never downloads data or calls a model.

```sh
python eval/research/import_loghub.py \
  --snapshot tmp/loghub-public-snapshot \
  --manifest tmp/loghub-public-snapshot/manifest-complete.json \
  --output-dir tmp/campaign-loghub-public-development --window 64
python eval/research/test_import_loghub.py
```

Include the upstream LICENSE in the snapshot; the output retains it as
`LICENSE.loghub` and records attribution to Zhu et al., ISSRE 2023. This is Loghub's
[research/academic license notice](https://github.com/logpai/loghub/blob/master/LICENSE),
not an Apache software license. Keep downloaded and adapted corpora untracked.
All windows and both projections from a source family share one cluster and split;
hundreds of windows do not create hundreds of independent quality trials. Public
logs may overlap model training and are not necessarily sanitized: native secret
guards remain mandatory before any approved hosted export. JSON projections are
not genuine tool/API traces; indexed message lookup is not incident diagnosis.
These tasks add external-source development coverage, not agent or superiority
qualification. Exact JSON message values may be escaped in serialized source;
use the research loader's explicit inferred-answer option, not a default change.

## Multi-turn candidate screening

The public [LongMemEval cleaned dataset](https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned)
is a candidate for descriptive long-history testing, not an independent held-out
campaign merely because it has 500 questions. An offline audit of the full S
snapshot at revision `98d7416c24c778c2fee6e6f3006e7a073259d48f` (SHA-256
`d6f21ea9d60a0d56f34a05b609c79c88a451d2ae03597821ea3d5a9678c3a442`)
connected all 500 questions into **one** source-overlap component by unioning all
haystack session IDs and identical role/content session hashes. This uses no
answers, answer-session IDs, `has_answer` annotations or model outcomes. Do not
randomly partition its questions and call the splits source-independent, or use
500 as the independent-unit count for CFR qualification. The pinned dataset card
declares MIT and explains its upstream cleaning; preserve that provenance rather
than presenting it as untouched original data or using the oracle subset.

The S snapshot has 396–616 turns per question, above the research replay's current
64-message bound. No replay bound was relaxed, conversation silently truncated,
oracle evidence selected, or hosted data exported during this screening. A future
long-history adapter needs an explicit bounded contract, annotation stripping,
appropriate evaluator and model context qualification. The existing short replay
adapter does not establish full long-history support.

## Human multi-turn RAG candidate screening

An offline snapshot of [IBM MTRAG](https://github.com/IBM/mt-rag-benchmark/tree/2c618bb98db3c8526433e22d8a2f7320f10a7470)
at revision `2c618bb98db3c8526433e22d8a2f7320f10a7470` contains 842 tasks
from 110 conversations. The captured **full RAG** file has SHA-256
`5d5201da9fabd072fd8f6b8d051bfaafa7ef031e76722a4920c66e94cede1873`.
Use its five retrieved passages, not the reference or reference-plus-RAG files
that explicitly supply gold passages. Current-turn targets, relevance/reference
flags, answerability labels and author metadata must not enter model source or
drive passage filtering. Prior recorded agent responses are legitimate history,
not current-turn targets. Preserve the pinned repository's Apache-2.0 notice
and verify underlying corpus attribution/licensing before adaptation/export.

A reproducible source-only audit provisionally joined conversation IDs,
collection-namespaced parent-document IDs, exact passage hashes and exact prior
speaker/text hashes into **34** overlap components, including one with 375 tasks.
Document-root parsing is inferred from the snapshot's ID formats and needs
verification before final split assignment. This is conservative screening,
not proof of independent clusters or 842 independent trials. The largest input
has 22 prior messages and 18,990 raw content bytes before framing, so this
candidate does not inherently require relaxing the existing 64-message bound.

`import_mtrag.py` now adapts that pinned full-RAG snapshot offline. It preserves
all five passages and prior message text in exact source-order frames, maps the
recorded `agent` speaker to the replay's `assistant` role, and verifies both
caller-group reconstruction and the existing bounded replay contract. It refuses
unknown passage-ID layouts, duplicate task IDs, unsupported speakers, more than
64 prior messages, oversized questions/sources and snapshots with mismatched
hashes or URLs. Only the byte-pinned full-RAG file is accepted, never an oracle
file renamed by a caller. All 842 records are clustered before the per-split cap.
The current conversion reproduces 34 components: 17 training, seven validation,
ten test, containing 616/58/168 records. Public development labels are not an
independent qualification claim. The default materializes at most 20 per split.

```sh
python eval/research/import_mtrag.py \
  --snapshot tmp/mtrag-public-snapshot-20261004 \
  --output-dir tmp/campaign-mtrag-public-development-20261004
python eval/research/test_import_mtrag.py
```

No MTRAG model call, golden-reference injection or truncation was performed.
The importer does not run native export guards or approve underlying corpus
rights. A predeclared evaluator for long-form grounded answers and abstention
is still required. Imported tasks declare that unmet evaluation requirement;
the short-answer observation pilot refuses them before inference rather than
silently treating exact-value or Hotpot EM as adequate. Corpus/cache and
screening artifacts remain untracked; this is preparation, not a win.

### Blinded grounded-answer evaluator development

`grounded_answer.py` adds a research-only `GroundedJudge` callable, separate
from compression and answering. It evaluates against the original uncompressed
context, question, reference answer and response, never an arm name or compressed
payload. References are evaluation-only exemplars, not additional source evidence.
The native source/query and serialized evaluation-input guards run before export,
including reference/response text. There is one bounded model attempt, no retry
or route fallback. Optional structured mode requires the transport's already
advertised support. Replies must contain exactly five genuine JSON booleans:
correctness, grounding, completeness, relevance and appropriate abstention. Extra,
missing, duplicate or incorrectly typed fields make the judgment invalid.

The receipt retains the rubric/input hashes, all attempted-call usage/cost,
fixed failure stage and valid metrics, never rejected replies or exception text.
Unknown usage/cost remains unknown. Deployment compression/answering costs and
evaluation costs must remain separately identifiable in future paired reports;
neither may be discarded. This rubric is **not** official MTRAG/RAGAS/RADBench
scoring, semantic proof, an independent human judgment or production admission.
It still needs representative human calibration and frozen live-runner integration;
the default CLI short-answer pilot continues to refuse MTRAG tasks.

The explicit Python `run_pilot` mode `answer_evaluator="grounded-answer-research-v1"`
now accepts `answer_judge=GroundedJudge(...)` with a separate bounded model transport.
It uses a complete-answer prompt rather than the unchanged default short-value
prompt, guards source/grouping and every candidate output before export, and never
sends references to answering/compression calls. Unknown evaluation requirements
or missing/shared/implicit judge transports are refused before inference.
Malformed/failed judgments make the paired outcome invalid, never a candidate win.
Student and judge calls/usage remain separate: opt-in rows contain `judge_receipt`,
evaluation time and combined time, while `economics_summary` adds an `evaluation`
section retaining all judgments, including invalids. Campaign-wide costs must count
both sections; deployment latency excludes the separately reported evaluation time.
Cost totals now also expose `known_billed_cost` (only a subtotal),
`unknown_cost_records`, and `cost_accounting_complete` for each arm and the
evaluation section. `billed_cost` stays null if any attempt lacks a valid,
finite, nonnegative reported cost; booleans are not costs. A zero known subtotal
with unknown records is not a zero-price total. Arm counts refer to task/arm
records (including skipped rows), while evaluation counts refer to model-call
records; neither should be presented as a new inference-call count. The existing `complete` field
describes valid paired attempts, not complete cost accounting. Do not sum shared
raw controls across comparator reports or combine different model token units.
The CLI does not yet expose this mode or reserve/freeze its separate judge budget.
Do not use the default CLI to silently bypass that missing integration.

Offline check: `python eval/research/test_grounded_answer.py`.
A frozen live calibration of eight paired synthetic positive/negative responses
covered four constructed scenarios (negation, leading-zero identifiers, omitted
subanswers and justified abstention). Six judgments matched their predeclared
outcomes, but two negative cases reached the 1,536-token output allowance and
remained invalid. The study failed its all-eight-valid/matching rule. All eight
attempts and their 10,033 inference tokens were retained with known zero provider
charges; no failed case was retried or dropped. This is counterevidence against
using the current judge configuration, not MTRAG quality qualification or eight
independent trials. No MTRAG answer was generated or judged.

A distinct 3,072-token repeated calibration completed all eight judgments but
matched only five: three correct, non-abstaining answers received an inappropriate
false `appropriate_abstention` verdict. Output validity alone did not fix accuracy.
The optional `GroundedJudge(..., explicit_abstention=True)` uses versioned guideline
`tokenfold-grounded-answer-explicit-abstention-v2`, explicitly distinguishing
appropriate handling of answerability from the mere presence of an abstention.
The original v1 default/rubric and old failures remain preserved; all five criteria
are still required, with no threshold relaxation. A new frozen 3,072-token v2
study matched all eight repeated synthetic outcomes with valid replies and stable
provenance. This is development calibration, not independent judge accuracy or
MTRAG/compressor qualification; representative human calibration remains required.

## External-code development screening

A development corpus uses real [CPython v3.13.7 source](https://github.com/python/cpython/tree/bcee1c322115c581da27600f2ae55e5439c027eb)
at commit `bcee1c322115c581da27600f2ae55e5439c027eb` with **synthetic signature
lookup questions**, not real coding-agent tasks. The offline AST builder parses
but never executes source, extracts literal constant-default expressions, and
retains complete callable snippets up to 16 KiB. It selects at most twelve
source-order questions per module before any model inference. Caller groups are
source-only, with no gold-derived required content or critical atoms.

The current screening produced 70 questions from seven modules. After a
source-only split assignment (before inference), all questions from a module
stay in one component/split: four training modules, one validation module and
two test modules. These are development labels, not independently qualified
held-out trials; functions/parameters within a module do not increase the
independent-unit count. Different modules are not proven independent either.
Preserve upstream LICENSE and module headers, snapshot URLs/revision/byte hashes,
the AST builder fingerprint and shared source-component converter fingerprint.
Downloaded source, builder scripts and generated fixtures remain untracked.

All generated source/query pairs passed native secret guards, literal-answer
loading and output-hash checks without model calls. That establishes preparation
only, not consumer readability, signature-answer quality, bug-fixing ability or
superiority. Public code may overlap model training; a full coding workload and
live paired quality/economics qualification are still needed.

### Native CLI generative arm (local Qwen3.5)

The standard comparison runner can now add the **actual** experimental
`tokenfold summarize` command with `--generative-cli-approval <local-approval.json>`.
This explicit flag requires `--provider native`; no Ollama or hosted execution is
introduced. The bounded approval must name the same native model alias used by
the shared answering/ACON ledger. Freeze runtime
`generative_cli_guideline: tokenfold-native-cli-summarize-v1`,
`generative_cli_approval_sha256` and `generative_cli_timeout_ms` (deadline floored
to milliseconds), and
`generative_cli_seed_control: worker-owned-not-set-by-benchmark`. The pilot seed
controls answering/ACON, not the approved CLI worker's generation RNG; do not
claim a shared compression seed from the report's answering settings. Freeze and
verify worker decoding separately. The core inference deadline cannot exceed the declared runner
deadline or 30 seconds; the subprocess has ten additional seconds for CLI/receipt
overhead, not a longer inference allowance.

Reserve two additional calls per task (generation and answer). The arm is named
`tokenfold-generative-cli`, separately from the unchanged Python
`--generative-arm`. Both may be explicitly compared. Native guards run before
model initialization, source grouping/query alone reach the summarizer, and the
approval hash is checked before and after the run. Changes preserve attempts but
invalidate provenance and exit nonzero. No approval contents, credentials or
worker configuration are embedded in reports; freeze worker code/runtime/weights
independently, because the approval hash and server alias do not attest them.
Scripted runner regression checks disjoint usage, deadline propagation, refusal
before initialization and final approval changes; it is not live quality evidence.

The first default-0.8B live CLI-arm comparison reused the already exposed
Hotpot TRAIN0003/0004 cases. It stopped after three actual calls: raw/lossless
answers completed, then official ACON base compression reached its predeclared
1,536-token output cap. Consumed usage was retained; every subsequent arm,
including the CLI summarizer, was skipped without retry. Stable provenance and
the reaped server do not make this a complete comparison or a summarizer failure.
Artifacts: `tmp/native-cli-generative-hotpot-training-20261005`.

A separate synthetic integration used a previously successful literal log source
with deterministic source-line grouping, not gold-derived protection. The actual
CLI arm invoked the bounded helper against one resident default-0.8B CUDA server;
the complete eight-call run preserved all comparator/approval/runtime hashes and
reaped the server (1.537-second startup). Generation consumed 1,695 input/299
output tokens, but its candidate exceeded the 600-token target: native admission
returned exact 800-token raw source, and the student answered correctly. This is
**a visible budget fallback, not a compressed-summary quality win**. Total CLI-arm
usage was 2,582/301 versus raw 887/2 and native lossless 281/2 (189-token frame),
so there is no one-shot economics superiority. ACON's `Billing service` versus
fixed `billing` is an EM-format mismatch, not proven factual regression. Headroom
Universal/Smart payloads were 679/800 tokens. Local costs remain unknown, startup
is separate, and no hosted or validation/test calls were made. This verifies the
real CLI path and accounting only; representative/optimized/CCR qualification
remains pending. Evidence: `tmp/native-cli-generative-synthetic-integration-20261005`.
That historical ledger labelled its valid budget-rejected generation `failed`;
the receipt still records `valid_attempt: true`. The adapter now labels valid
generation consumption `ok`, separately from candidate disposition and target
failure, so budget fallback is not misreported as interrupted inference. Original
artifacts are preserved; no model calls were repeated for this accounting fix.

A subsequent dual-model TRAIN study used the next source-order Hotpot v4
components 0005/0006: the actual CLI summarizer stayed on resident **0.8B**, while
all answering and official ACON base/QA calls shared a separately pinned resident
**4B**. Both servers loaded once; all 20 calls completed, frozen provenance stayed
unchanged, and both servers were reaped. Startups were 2.157/10.839 seconds,
reported separately. Both CLI generations were valid but exceeded their targets
(782/746 tokens), returning exact 1,042/995-token raw source. Thus its 1/2 answer
successes equal raw controls, **not accepted-summary wins**. ACON base/QA scored
0/2 with 66/49 and 129/50-token payloads; Headroom Universal scored 1/2 with
929/865 tokens, while SmartCrusher and native lossless were raw no-ops. The second
task's fixed answer is `25 April`, explicitly available via its Anzac Day source;
all arms answered the year or season instead. Preserve those failures rather than
reclassifying them as harmless answer aliases.

CLI-arm consumption is disjoint: **0.8B 3,304 input/501 output**, plus **4B
2,330/17**, exactly the latter's raw answering usage. Mixed-model `total_usage`
remains null, not a sum of different tokenizer units. ACON base/QA totals are
4B 2,923/729 and 3,065/775; Headroom Universal is 4B 2,111/15. Costs remain unknown.
The SHA-bound posthoc audit reconciles all row calls against both ledgers and
confirms both fallback payloads byte-for-byte. This is counterevidence to current
one-shot summarizer economics, not superiority or held-out qualification: only
one raw-success training component, no accepted CLI summary, optimized ACON,
full Headroom CCR, or independent semantic verification. Evidence:
`tmp/native-cli-dual-model-training-20261005/analysis.json`. No calls were repeated
or defaults/safety gates changed during the audit.

A private source-cost hypothesis then reused only those two exposed TRAIN cases,
adding explicit complete-wrapper budget guidance and source-only single-group
empty-summary frame token estimates. The actual CLI made two fresh 0.8B
generations on one owned resident CUDA server; both completed within unchanged
native gates, with stable frozen files and a reaped server. Payloads fell to
498/192 tokens, versus the prior 1,042/995-token raw fallbacks. This **does not
justify promotion**: the first summary used a Wasim Akram/1995/161-run account
absent from its selected evidence (it occurs in an unselected original paragraph),
whose relevant selected Pakistan paragraph instead
states Javed Miandad/1992–93/33 runs. It also omitted the Shaheens identity link.
The second summary was locally grounded but omitted the Anzac Day `25 April`
link needed for the fixed answer. Correct native citations and smaller frames do
not prove entailment or answer sufficiency. Generation usage was 3,923 input/256
output, startup 1.541 seconds; no student, judge, hosted or held-out calls were
made. The private helper was **not installed as a default**. SHA-bound source and
receipt audit: `tmp/native-cli-budget-guidance-training-20261005/analysis.json`.

`grounded_answer.EvidenceSummaryJudge` adds an explicit research-only rubric for
native compiled frames. Before any evaluator call it checks exact protected text,
strict wrapper fields, nonempty summary/evidence, authorized optional IDs, literal
evidence text, uniqueness and source order. The judge receives original context,
question, summary and supporting text, **not gold, group IDs or arm/receipt labels**.
Grounding is checked against selected support rather than licensing every claim
found elsewhere in the original source; completeness compares retained support
against the original. Existing `SummaryJudge` behavior is unchanged. This API
does not add automatic verification, alter native admission, or certify semantics.

Four live resident-4B calibration calls used those two actual TRAIN frames and
two manually source-derived positive controls. All calls completed with stable
provenance and a reaped server (4.103-second startup; 6,643 input/108 output tokens).
The judge rejected the unsupported selected-evidence claims in the first actual
frame and accepted both controls. It nevertheless accepted the second actual
frame's completeness, missing our intended `25 April` linking-evidence criterion:
**3/4 rubric matches, one completeness false positive**. The broad `when` query
also permits a season/round reading; this small calibration cannot establish
held-out judge accuracy or silently substitute judged sufficiency for fixed-answer
scores. A source-only judge is useful diagnostic feedback, not a safe automatic
promotion gate. Original study artifacts remain unchanged; the follow-up corrects
the earlier shorthand `invented`: the first account exists in original context
but not its selected evidence. Calibration and SHA-bound audit:
`tmp/evidence-summary-judge-calibration-20261005`.

`generate_observation.CliGenerativeArm` exercises the actual `summarize` command,
not the older Python-only `--generative-arm` implementation. Its approval file must
name its own pinned summarizer ledger's model, which may differ from the pilot
student/ACON teacher. Freeze the bridge,
CLI binary, approval arguments, server identity and artifact digest independently.
The adapter exports only reconstructed source grouping and query, never task gold.
It reserves one bounded model attempt, retains runtime-reported usage even on failed
candidates, and marks malformed/incomplete/unauthorized generation invalid rather
than scoring raw fallback as a successful generated-summary attempt. Source-only
preflight skips do not invent a zero-token inference call.
Interrupted/malformed/incomplete generation now halts this adapter and any shared
native/hosted ledger exposing `stop_reason`, preventing subsequent generation or
answering against uncertain inference. Known usage survives invalid receipts;
missing `runtime_invoked` is unknown consumption, not a fabricated no-call skip.
Valid source-only skips and size/budget fallbacks do not poison a healthy ledger.
Pre-generation model verification failures halt without inventing an inference.

For local Qwen, use the resident offline Transformers setup in
`docs/configuration.md`, not an Ollama dependency. A caller-owned local snapshot
ledger supplies `name`, `digest`, `verify()`, `max_calls` and mutable `calls` to
`CliGenerativeArm`; verify its approved weight/tokenizer manifest independently.
The adapter does not need the ledger to answer/chat. Use a separately pinned,
bounded student for answering and the official ACON teacher. Existing local
research transport defaults are unchanged, not a recommendation for local Qwen.

`run_pilot` retains separate compression calls in `compression_model_calls` and
reports counters by pinned identity in `usage_by_model` (including invalid work).
An independent compression ledger makes mixed-model `total_usage` null, never a
sum of different tokenizer units. Unknown counts remain null within their model;
unknown local compute cost makes combined billed cost null even if hosted answering
reports zero. A source-only skip records no invented compression call. Reusing the
same pinned identity across separate ledger objects is refused before inference;
same-model studies must share a ledger to avoid ambiguous/double accounting.
An explicit `valid_attempt: false` compression receipt is invalid regardless of
the candidate's display name. No answering call follows; renamed 0.8B/2B arms
cannot count a malformed/incomplete generation's raw fallback as a quality win.
All compression and answer inference must share the pinned tokenizer before their
counters can be summed. Native payload
estimator counts are separate. Unknown local compute cost/billing remains null,
not a dollar win. This observation pilot is still not official optimized ACON,
Headroom full recovery/proxy qualification, or a complete representative campaign.

#### Resident-runtime development evidence (2026-10-04)

The native Qwen3.5-0.8B arm now supports a caller-started resident runtime: weights
and tokenizer load once, while each request performs fresh inference. Record
startup separately from request latency and allocate it explicitly in deployment
economics; do not count warm requests as cold loading or artifact reuse.

A public TRAIN development comparison used the pinned Liquid student/ACON teacher
with an 8,192-token output allowance and a 60-second call deadline, avoiding the
earlier 1,024-token truncation failures. The route remained `liquid/fp8`, zero-price
only, with its existing descriptor pin and no retries or alternative routes. One
task, `hotpot-5a8a6b625542996c9b8d5ed9`, completed across all six arms. Raw, native
lossless, official base ACON, and both pinned Headroom default observation arms
answered correctly. The native 0.8B generated arm answered incorrectly: its
summary invented locations and cited only one of the documents needed for the
multi-hop answer. Native attribution/budget checks accepted the structurally valid
candidate; they do **not** establish semantic entailment or evidence completeness.

The generated payload shrank from 1,742 to 166 estimator tokens, but this is not a
quality-preserving win. Its Qwen usage was 2,241 input / 163 output tokens, separate
from the hosted answerer's 255 input / 1,227 output tokens. Combined local/hosted
cost remains unknown, not zero. Model startup took approximately 5.1 seconds;
generation receipt latency was 6.2 seconds and the complete generated arm took
14.0 seconds excluding startup. These are development observations, not controlled
speed comparisons. Retained artifacts are in ignored
`tmp/hf-qwen-resident-liquid-train-8192-20261004`; no promotion follows this result.

Next, improve grounding on training data before opening fresh validation. A
source-only expansion materializes 100 records per existing development split;
the original first 20 remain byte-identical. Fresh validation selection must
exclude previously materialized IDs and known exposed source components, select
without gold/outcome filtering, and retain its pre-inference audit. Public
validation contamination and semantic independence remain unresolved; preparation
alone is not held-out quality or representative campaign qualification.

A subsequent paired prompt experiment used the original and strengthened native
grounding instructions against the same resident 0.8B weights on a constructed
incident and two already-observed TRAIN inputs. Six fresh generations completed,
with stable model/source/approval/worker/binary pins. The stronger instruction
restored the festival document and Camber Sands fact on TRAIN0010, but still
included an unsupported location phrase. On TRAIN0009 it instead omitted the
needed Renzo Gracie/Carlos Newton evidence and asserted a different champion.
These mixed source-level findings do not establish an answer-quality improvement;
no hosted answering or held-out evaluation occurred in this prompt experiment.
Instructions are guidance, not a semantic guard. Preserve both old and new outputs
in `tmp/hf-qwen-grounding-prompt-training-20261004` and keep the runtime experimental.

The explicitly selected optional 2B snapshot is now available at Hugging Face
revision `15852e8c16360a2fea060d615a32b45270f8a8fc`, with downloaded LFS weight
hashes checked before inference. Its first three native development calls all
failed closed as malformed candidates. A separate shape-only diagnostic found
that the response was a JSON object containing `summary` and `source_ids`, but
omitted required `schema_version` and `model_revision`; rejected text was not logged
or persisted. Native acceptance was **not** relaxed to fill missing metadata.

The shared native instruction now includes a concrete required four-field JSON
shape with the exact approved model revision. A new implementation study retained
the earlier failures and completed three native 2B generations on the same
observed inputs: incident 1,160→114, TRAIN0009 1,455→382, TRAIN0010 1,742→550
estimator tokens. Startup was 6.6 seconds; generation receipts were approximately
4.0, 4.2 and 5.9 seconds. These are structurally accepted, semantically unverified
summaries, not answer-quality or superiority proof. The 0.8B default is unchanged;
2B is explicit, with no runtime escalation or fallback. The two retrieval summaries
include the expected answer values, but still require independent answering and
evidence-completeness checks. Artifacts remain in
`tmp/hf-qwen-2b-required-shape-training-20261004`.

A subsequent seven-arm comparison on fresh TRAIN0011
(`hotpot-5ae0fe8855429945ae959495`) used both explicit resident profiles with the
same current native prompt. All implementation, model, hosted-route and Headroom
pins were stable. Raw, native lossless, base ACON and both Headroom default arms
answered correctly. Both Qwen profiles consumed their complete 256-token output
allowances without EOS, producing `incomplete_response` and exact raw fallback.
Both attempts were invalid, not answer failures or successful compression; no
hosted answer calls followed them. There were zero common-valid seven-arm tasks.
Retain these failures in `tmp/hf-qwen-dual-resident-liquid-train-20261004`.

This identifies generation allowance as a remaining tuning parameter, not proof
that either larger models or prompt changes preserve quality. Any larger-cap study
must be explicitly frozen as a new training configuration, retain these attempts,
keep the same bounded native deadline and separately account for startup and all
consumed inference. Never silently retry a truncated candidate or claim its raw
fallback as a generated-summary win. Fresh validation remains unopened for tuning.

The explicit 512-token study on the **same observed TRAIN0011** completed both
generations, but both complete evidence payloads exceeded the 866-token target.
Native code returned exact 1,154-token raw fallback (`over_budget`), so the correct
downstream answers were fallback results, not useful generated compression. Retain
all inference: 0.8B consumed 1,755 input / 318 output tokens and 2B consumed 1,751 /
351; both incurred unknown local cost. This repeat does not increase independent N.
Artifacts are in `tmp/hf-qwen-dual-resident-cap512-train-20261004`.

That study also exposed student variability: raw and native lossless sent identical
source bytes, yet the latter returned an explanation rather than only `NO` and
failed standard answer EM. Reports now include additive
`byte_identical_raw_controls` attempt/answer/outcome disagreement counts. These
controls warn against attributing unchanged-payload differences to compression;
they do not remove failures, replace the evaluator, cache replies, retry calls or
discount consumed inference. A post-hoc supplemental controls file preserves the
original frozen report unchanged.

The native prompt now requests the shortest sufficient summary and smallest
supporting group set, without dropping needed facts. A source-only follow-up on
the same observed TRAIN task (no hosted answering) admitted the 0.8B payload at
788/866 tokens, but its summary incorrectly described both players as Austrian;
the literal evidence retained seven groups, including Tom Gullikson's contradicting
United States fact, but omitted the Jürgen Melzer group. The
optional 2B generation instead exhausted its 512-token allowance. This again
demonstrates why structural acceptance and literal attribution do not establish
factuality or quality admission. Both attempts and stable pins are retained in
`tmp/hf-qwen-minimal-evidence-training-20261004`; no validation promotion follows.

#### Evaluator-only source-document coverage

`summary_evidence.document_coverage(task, upstream_record, payload)` measures native
literal document retention against Hotpot's human supporting-fact annotations.
It checks exact task/source/group reconstruction, sentence-index validity and every
attached ID/text pair. Raw source is distinguished from native generated evidence;
unsupported narrative formats return null rather than being scored as losses.
Duplicates, changed text, unknown IDs and mismatched upstream records are refused.
Both original and columnar public snapshot annotation layouts are supported.

This is document-level annotation overlap, **not** official sentence-level/joint
Hotpot scoring, semantic entailment, answer correctness or a production guard.
Complete document coverage can coexist with a false summary. Gold labels are used
only after generation: never add them to grouping, required flags, critical atoms
or model prompts. Run `python eval/research/test_summary_evidence.py`; CI runs it.

A frozen post-hoc audit of seven already-observed training outputs confirmed the
missing-evidence problem: TRAIN0009 recall changed from 1.0 to 0.0 under the first
grounding prompt, while the optional 2B shape-fixed output retained only one of two
required documents. The latest 0.8B TRAIN0011 output retained one of two required
documents despite selecting seven groups. TRAIN0010 grounding/2B outputs retained
both required documents, but coverage alone does not establish factual summaries.
The audit made no inference calls or model-input changes; artifacts are in
`tmp/hf-qwen-4b-resident-training-20261004/prior-training-document-audit.json`.

#### Optional 4B resident runtime boundary

The explicitly downloaded official 4B snapshot is pinned at revision
`851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a`, with LFS weight hashes checked.
A training integration planned four requests using one CUDA resident model, a 512-token
output allowance and unchanged native deadlines. The constructed incident completed
at 1,160→114 estimator tokens (1,582 model input / 71 output tokens, approximately
17 seconds native receipt latency). TRAIN0009 returned `runtime_timeout` with
unknown inference usage. The next CLI call reached its post-call health probe,
which timed out before that attempt's receipt was persisted; retain it as an
additional invalid attempt with unknown usage, not a fabricated zero. TRAIN0011
was not attempted. The owned model server was terminated/reaped in `finally`.

An independent post-terminal audit verified all frozen files remained unchanged.
A system GPU sample during the run showed 15,417/16,303 MiB used; it is neither an
isolated model footprint nor a peak measurement. Memory pressure may contribute,
but the sample does not prove the timeout's root cause. Full artifacts and partial
telemetry limitations are retained in `tmp/hf-qwen-4b-resident-training-20261004`.
This profile is not live-qualified for retrieval quality or sustained throughput.
Future runtime trials must persist native attempt receipts before optional health
probes. Do not extend deadlines, relax guards or silently escalate the 0.8B default
to mask these failures.

#### Native llama.cpp resident development evidence

An explicit follow-up used official llama.cpp `b11399` (commit `2ca15f540`),
the verified official Windows Vulkan x64 archive, and locally converted BF16 GGUF
from the same pinned official HF snapshots. No Ollama, community checkpoint,
integer quantization, hosted answering calls or production-default changes were
involved. The existing OpenAI loopback bridge reused one caller-owned server.
Protocols freeze the converter inputs/source and runtime/DLL/weight/worker files;
all frozen integration files remained unchanged after both terminal runs.

For 0.8B, startup was 6.635 seconds; native receipt latencies for the constructed
incident and observed TRAIN0009/10/11 were 14.157, 2.897, 6.929 and 3.689 seconds.
Three candidates passed structural guards; TRAIN0010 fell back for budget overflow.
The post-hoc document audit found required-document recall of 0.0 on TRAIN0009
and 0.5 on TRAIN0011. The latter also falsely called both tennis players Austrian.
TRAIN0010's complete raw fallback is not a generated-summary quality success.

For optional 4B, startup was 14.772 seconds. All four requests returned
`runtime_timeout` at the existing 25-second client limit, with unknown model usage
and exact raw fallback. Every primary receipt was persisted before teardown;
there were no post-attempt health probes. A system sample showed 13,779/16,303 MiB
GPU memory used and 99% utilization, not an isolated footprint or proven root cause.
Timeouts do not guarantee server-kernel cancellation, so later serial requests can
include outstanding work from earlier calls. Do not interpret them as independent
decode-speed measurements. The owned server was terminated/reaped after the run.

A separate one-request 4B diagnostic in a fresh server with a 4,096-token context
and 32-token output cap completed a tiny synthetic prompt: startup 8.148 seconds,
request 0.918 seconds, 18 input / 2 output tokens, EOS completion. Engine-reported
prompt time was 530.647 ms and decode time 357.093 ms. This rules out a completely
nonfunctional 4B runner, but the tiny two-token result is not sustained throughput
or an explanation of the larger summarizer timeouts. Its context and prompt differ
from the integration run. No summary quality was evaluated and no deadlines changed;
the diagnostic server was stopped after its single request.

Artifacts are in `tmp/llama-native-qwen-20261004`, including conversion protocols,
`integration-08b/analysis.json`, its post-hoc document audit, and
`integration-4b/analysis.json`. These are observed training/runtime diagnostics,
not held-out quality admission or a controlled cross-engine performance ranking.
The isolated diagnostic is retained in `diagnostic-4b-small/result.json`.

The next explicit backend trial used the official matching `b11399` CUDA 13.4 x64
binary and runtime DLL archives, verified against their release SHA256 digests.
`--list-devices` identified CUDA0 as the RTX 5080 on driver 595.97. The same 4B
BF16 GGUF, native CLI/worker/prompt, 16,384-token context, 512-token output cap,
and deadlines were preserved. Its protocol stops after a timeout rather than
queuing more requests behind potentially outstanding work. No timeout occurred:
startup 5.645 seconds, constructed/TRAIN0009/10/11 receipt latencies
2.620/3.840/3.180/6.165 seconds. All frozen files remained unchanged and the owned
server was stopped. This demonstrates a working native CUDA profile, not a
controlled universal comparison with Vulkan or vLLM.

Three candidates passed structural guards (1,160→112, 1,455→524, 1,742→295
estimator tokens); TRAIN0011 fell back for budget overflow. Original model usage
was respectively 1,582/67, 2,010/119, 2,378/90 and 1,775/211 input/output tokens.
The post-hoc audit found only one of two required documents in both generated
TRAIN0009 and TRAIN0010 outputs. Plausible answer text does not substitute for
complete source evidence or live held-out task quality. Artifacts and audit are in
`tmp/llama-native-qwen-20261004/integration-4b-cuda13`; hosted answering calls were
zero, local cost remains unknown, and no production quality admission is claimed.

An evidence-first development variant asks for `source_ids` before `summary` in
both the native instruction/example and Qwen JSON-schema property order. It asks
the model not to use facts from unselected optional groups. This changes generation
guidance, not the native candidate/report schema or semantic admission policy.
Tests exercise the real child instruction and verify optional-only ID enums and
field ordering. The first explicit 4B CUDA trial of this variant completed the
constructed incident (1,160→114 tokens), then TRAIN0009 timed out with unknown
usage; the protocol stopped rather than submitting its remaining two requests.
All frozen files remained stable; the owned server was stopped. A subsequent
system sample still showed 100% GPU utilization after teardown, with a game process
running, so this is not an isolated performance comparison or proof of causation.
Retain the timeout counterevidence in
`tmp/llama-native-qwen-20261004/evidence-first-4b-cuda13`; do not consume fresh
held-out validation to debug this profile.

A bounded October 5 follow-up completed four fresh generations per size on one
owned resident CUDA server each. Initial system GPU utilization was 8% for 4B
and 2% for 0.8B; per-attempt samples include the running model, not an isolated
measurement of competing applications. Both protocols retained every receipt,
usage and fallback, verified frozen files unchanged, and reaped their servers.
No hosted answering or held-out validation/test calls were made.

| Profile | Startup | Incident | TRAIN0009 | TRAIN0010 | TRAIN0011 |
|---|---:|---|---|---|---|
| 4B BF16 CUDA | 4.133 s | 1,160→114 | incomplete response (512 output tokens) | 1,742→608 | over-budget raw fallback |
| 0.8B BF16 CUDA | 1.526 s | over-budget raw fallback | 1,455→1,079 | 1,742→585 | incomplete response (512 output tokens) |

For TRAIN0010, both generated outputs now retain both annotated supporting
documents (three selected documents total), but **0.8B still produces false and
contradictory prose**. Its TRAIN0009 output retains only one of two required
documents and also contains unsupported claims. The 4B TRAIN0010 output includes
the source-stated festival location and Pontins company name, alongside unnecessary
distractor discussion. Native command times were 2.173–7.159 s for 4B and
0.894–2.062 s for 0.8B, excluding startup; these small observed training diagnostics
are not throughput, semantic admission, total-economics or superiority evidence.
Exact raw fallbacks are not generated-summary wins. Local cost remains unknown.
Artifacts and post-hoc document-only audits are in
`tmp/llama-native-qwen-20261004/evidence-first-4b-cuda13-idle-20261005` and
`tmp/llama-native-qwen-20261004/evidence-first-08b-cuda13-20261005`.

A separate three-call 0.8B CUDA diagnostic used the first source-order **training**
examples from USGS JSON, Apache logs and CPython signatures. Startup was 1.546 s;
USGS generated 8,092→434 tokens in a 1.274 s native command (10,538 input / 58
output model tokens). Its single literal evidence group is exactly the requested
event. The summary contains the requested place but emits the earthquake title,
not the exact requested place string. The generated envelope sits between the
preserved source prefix/suffix: its `features` entry is a summary/evidence object,
**not an original GeoJSON Feature**. This is not schema-preserving JSON compression
or downstream task success. Apache and CPython exhausted the 512-token output cap
and returned byte-exact raw fallbacks, not generation wins. All frozen inputs were
stable and the owned server was reaped. Local cost remains unknown; no hosted
answers or validation/test examples were consumed. Artifacts and the post-hoc
literal-only audit are in
`tmp/llama-native-qwen-20261004/cross-workload-08b-cuda13-training-20261005`.

The Qwen loopback decoding schema now requests between one and the number of
optional groups in `source_ids`, instead of an unbounded array that also allowed
empty evidence. Native compilation already refuses empty citations; native
candidate/report formats, evidence authorization, budgets and deadlines are
unchanged. This cardinality bound does not enforce uniqueness, entailment or
completeness. Direct Transformers generation does not apply the decoding schema.
Two bounded three-call training diagnostics (maximum-only, then nonempty bounded
IDs) returned all three payloads byte-identically to the preceding profile: the
USGS envelope and two incomplete-response raw fallbacks. This **did not fix** the
observed log/code output-limit failures. Both protocols froze the changed worker,
verified stable inputs and reaped their servers; no hosted or held-out calls were
made. Artifacts are in `bounded-ids-08b-cuda13-training-20261005` and
`nonempty-bounded-ids-08b-cuda13-training-20261005` under the same native study root.

A two-call research diagnostic inspected incomplete responses **in memory only**
and persisted structural counts, not rejected text. Both stopped at 512 output
tokens with no reasoning text and incomplete JSON. Apache had not closed its ID
array or reached the summary field; CPython had completed its one-ID array and
reached an unfinished summary. This distinguishes the observed failure stages;
it does not prove one universal cause. Native receipts still reported
`incomplete_response` and exact raw fallback. Artifacts are in
`incomplete-structure-08b-cuda13-training-20261005` under the native study root.

A separate **research-only**, three-call hypothesis represented optional group
IDs as short source-order `sN` aliases in the model prompt/schema, translating
successful selections back to authorized original IDs before unchanged native
compilation. Apache then completed with 93 output tokens (4,118 input, versus
4,854 previously), producing a 2,442→152-token envelope. Its summary is the exact
requested source line; native evidence preserves that line and one unnecessary
adjacent line byte-exactly. CPython still exhausted the output cap. Previously
accepted Hotpot TRAIN0010 also exhausted it with aliases, so this hypothesis is
**not enabled by default in the helper** or claimed as a general improvement.
Stable protocols and the post-hoc literal Apache audit are retained in
`short-id-08b-cuda13-training-20261005`. Both owned servers were reaped; no hosted,
held-out or downstream answering calls were made. Local cost remains unknown.

A six-call paired **4B** training diagnostic compared original versus short IDs
on one resident CUDA server, with the same three tasks and 512-token output cap.
Startup was 4.086 s; all attempts completed and frozen inputs stayed unchanged.
Both Apache variants produced the identical 100-token envelope with just the
requested literal line. Both CPython variants completed generation but fell back
over budget (118-token source, 88-token target). For Hotpot TRAIN0010, original
IDs produced 608 tokens with both required documents plus one distractor; short
IDs produced 345 tokens with exactly the two required documents. Compression
model usage was respectively 2,420/184 versus 2,299/67 input/output tokens. The
short-ID prose nevertheless repeats the question's unsupported "amusement park"
classification: complete document coverage still is not summary entailment.
Artifacts and document-only audit are in
`paired-id-4b-cuda13-training-20261005` under the native study root. The owned
server was reaped; this does not validate the prototype across other tasks/models.

A separate three-call downstream **training replay** fed raw, original-ID and
short-ID TRAIN0010 payloads to the same approved, descriptor-pinned Liquid free
route. Raw answered "Camber Sands, England" (EM 0, F1 0.8); both summaries answered
"Camber Sands" (EM/F1 1). The raw location is substantively consistent but differs
from the exact gold string, so this is not proof of semantic improvement. Student
usage was 1,903/810, 710/648 and 445/237 input/output tokens; all three calls
reported zero billed cost and stable provenance. Previous local generation usage
is referenced separately, not erased by replay or summed with student token units;
local cost and startup amortization remain unknown. No failed call was retried,
no new ACON/Headroom comparison was made, and no held-out qualification is claimed.
The replay artifacts are in `tmp/paired-id-4b-downstream-training-20261005`.

The helper now exposes this experiment explicitly as `--short-source-ids` for its
loopback `openai` backend only; default approvals and other backends are unchanged.
Optional group aliases are translated back before native checks. Unknown aliases,
including attempts outside the optional set, are rejected without retaining model
text; known usage also survives excessive translated candidate size. Regression
tests cover source immutability, protected exclusion, original/alias namespace
collisions, duplicate selections, unknown/type-invalid IDs, incomplete responses,
oversized translation and explicit/default approval arguments.

A five-call native 4B integration used the supported helper option, not the private
prototype, on source-order TRAIN0010/0012/0013/0014/0015. All candidates passed
structural gates, inputs remained stable, and TRAIN0010 was byte-identical to the
earlier prototype control. Four retained both annotated supporting documents;
**TRAIN0012 retained only one of two**, despite selecting two groups. No student
or held-out calls were made. This is evidence of integration and a remaining
completeness failure, not semantic admission or qualified superiority. Artifacts
and post-hoc document-only audit are in
`tmp/llama-native-qwen-20261004/helper-short-id-4b-training-20261005`.

The intermittent synthetic resident HTTP test was reproduced rather than hidden
with retries: 12 of 60 checks failed (11 Windows connection resets during response
headers, one timeout), and an instrumented 25-check run had six resets even after
the server finished writing a length-delimited response. A private HTTP/1.1
experiment then passed 60/60; the supported helper now uses HTTP/1.1 with the
stdlib's five-second handler timeout for headers, body and idle reads. A separate
60-check run of that supported implementation also passed without transport errors,
while preserving one model initialization, fresh requests and output-cap rejection.
The server remains serialized; callers should promptly close consumed connections.
These checks use mocked model components, not live quality/throughput evidence.
Diagnostic artifacts are in `tmp/resident-http-repro-20261005`,
`tmp/resident-http-server-repro-20261005`, `tmp/resident-http-keepalive-repro-20261005`
and `tmp/resident-http-fixed-repro-20261005`; no source or rejected model text was
logged, no inference retry or native safety/default change was introduced.

#### Structured API JSON development snapshots

`import_usgs.py` adds an offline importer for real historical
[USGS event-catalog GeoJSON responses](https://earthquake.usgs.gov/fdsnws/event/1/),
not a JSON projection of log records. It preserves complete response values and
field order, including metadata and geometry, with whitespace-only serialization.
Each event is an optional native source group. The query asks for the exact place
string of the source-order middle event; it does not select records by answer,
model outcome or annotation, or derive protected content from gold.

```sh
python eval/research/import_usgs.py --snapshot <snapshot-directory> --manifest <snapshot-manifest.json> --output-dir <new-output-directory>
python eval/research/test_import_usgs.py
```

The manifest declares `files` with `path`, `sha256`, official HTTPS query `url`,
`split` and conservative `cluster_id`. The importer requires `format=geojson`,
`contributor=us`, USGS-network features, unique IDs and bounded snapshots. It
refuses canonical-ID or `properties.ids` alias overlap (within or across snapshots),
duplicate source clusters, digest mismatches and existing
output directories before any model invocation. It does not download anything.
The CI regression also checks exact JSON reconstruction, source-only grouping and
exclusive output. The previously misindented summary-evidence CI step was corrected;
the workflow now parses and both evidence/importer checks are list entries.

The initial source-only snapshot has 32 events each from January 2022, 2023 and
2024, predeclared respectively as train/validation/test, with magnitude ≥4.5,
ascending time and an explicit API limit of 32. These are bounded result sets,
not complete monthly catalogs. All three native source/query guards and the smoke
freeze passed, with zero inference calls; validation/test remain unconsumed by
models. Artifacts are in `tmp/usgs-json-development-20261004` and
`tmp/campaign-usgs-json-development`. Three disjoint source windows do not prove
independence or meet the qualification sample floor. This is exact structured
lookup, not a full tool/agent task, scientific interpretation or medical guidance.
The [USGS copyright policy](https://www.usgs.gov/information-policies-and-instructions/copyrights-and-credits)
distinguishes USGS-produced material from third-party content; contributor rights
and attribution still require review before hosted export. No broader rights or
quality qualification is inferred from a `.gov` address.

### Rejected entity-grounding prompt experiment

A paired local training experiment added an explicit warning against adopting
question premises, transferring facts between subjects, and inventing links,
with a synthetic Company/Venue/Event example (no corpus entities or gold answers).
Pinned resident 0.8B and optional 2B each ran before/after on already observed
TRAIN10/12/15, original evidence IDs, unchanged budgets and native guards: twelve
fresh generations, no hosted or held-out calls. Frozen inputs remained stable.
The 2B after-prompt still falsely assigned Wildwood's operator/location to the
festival, grew its TRAIN10 frame from 772 to 874 tokens, and changed TRAIN12 from
accepted output to budget fallback. The 0.8B after-prompt still adopted an
unsupported amusement-park premise; both versions fell back on TRAIN12/15.
The prompt change was therefore reverted, not promoted as a faithfulness fix.
Artifacts live under `tmp/llama-native-qwen-20261004/entity-grounding-{08b,2b}-training-20261005`.
This counterevidence shows why instruction assertions and valid citations cannot
substitute for semantic and downstream evaluation.

### Optional 2B downstream training replay

Nine bounded calls to the approved pinned Liquid free route compared raw context
and previously generated original-ID/short-ID 2B frames on observed TRAIN10/12/15.
All replies completed and route/frozen provenance checks passed, without retries
or new local generation. Exact matches were raw 2/3, original-ID 1/3, short-ID 2/3.
The original-ID TRAIN10 answer was `Camber Sands, England` (F1 0.8), versus the
reference `Camber Sands`: do not describe this formatting difference as a proven
factual regression. Correct downstream answers despite invented summary links
also do not certify faithfulness.

Student input/output token totals were respectively 4,138/2,359 (raw), 2,181/740
(original-ID), and 1,271/638 (short-ID). All nine student calls reported zero
charges; local compressor costs remain unknown, prior generation usage is
retained separately, and its resident startup is not silently amortized away.
These are training observations, not fair ACON/Headroom or held-out qualification.
TRAIN12's question asks for a country while its fixed upstream reference is
`Jamnagar`; all three answers were `India`. Keep those scored failures and labels
unchanged and flag the mismatch for review, rather than excluding or relabeling
the case after observing results. Artifacts are in
`tmp/paired-id-2b-downstream-training-20261005`.
