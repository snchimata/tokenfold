# Evaluation harnesses

## ContextDoctor (experimental)

Compare a recorded baseline and **one** context transformation by independently
verified task success, regressions, and cost per success—not token savings alone.
ContextDoctor is a small Tokenfold evaluation tool, not an optimizer or a hosted
service. It uses Python 3.9+ and the standard library. Reporting needs no binary,
credentials, model, network, Docker, or optional tokenizer.

From the repository root:

```sh
python eval/contextdoctor.py --help
python eval/contextdoctor.py validate --manifest eval/tasks/contextdoctor/cheaper_worse/experiment.json --records eval/tasks/contextdoctor/cheaper_worse/episodes.jsonl
python eval/contextdoctor.py report --manifest eval/tasks/contextdoctor/cheaper_worse/experiment.json --records eval/tasks/contextdoctor/cheaper_worse/episodes.jsonl --out NEW_REPORT_DIRECTORY
python eval/contextdoctor.py readiness --manifest eval/tasks/contextdoctor/cheaper_worse/experiment.json --records eval/tasks/contextdoctor/cheaper_worse/episodes.jsonl
python eval/contextdoctor.py plan --manifest eval/tasks/contextdoctor/identity/experiment.json --out NEW_PLAN_DIRECTORY
python eval/test_contextdoctor.py
python eval/test_contextdoctor_run.py
```

Output directories must not exist. `validate`, `report`, `plan`, `readiness`, and
`import-legacy` never execute manifests, load models, import the execution harness,
or access the network. `plan` prints the already frozen arm order; it neither
chooses a post-result schedule nor grants execution permission.

The public fixtures contain **fabricated synthetic charges and verification
receipts**. They test accounting, not real model quality, billed savings, or
independent institutional review. The identity fixture demonstrates no effect.
The scaled cheaper-but-worse fixture spends $2.50 / 2 baseline successes = $1.25
per success versus $2 / 1 candidate success = $2: 20% less spend, but 60% worse cost
per success. The lost baseline-only success stays visible.

### Metrics and interpretation

- Each task/trial/arm is an episode. A successful independently verified episode
  contributes one success, not one per model/tool call. Scheduled repetitions are
  not retries. Automatic retries and resume are unsupported.
- The numerator retains **every operation**, including failed, invalid, unpaired,
  timed-out, cancelled, unfinished, excluded-category, and unallocated operations.
  Costs for excluded categories remain in the ledger but not in a narrower
  included-scope ratio. Unallocated spend blocks a complete comparison.
- `provider_billed`, `usage_priced`, `modeled`, and `unavailable` remain separate.
  A provider-reported charge awaiting reconciliation is retained under the billed
  ledger with its explicit incomplete status; it is not complete billed spend.
  Priced observed tokens are not invoices. There is no live repricing or currency
  conversion, and no billed-plus-token-reconstruction double counting.
- Five categories—model, transform, tool, verification, infrastructure—are
  explicitly included, excluded, or unknown, each with a basis/allocation rule.
  Excluded is not zero. A model-only ratio never becomes all-in economics.
- Unknown usage/costs are not zero. Known subtotals are lower bounds when coverage
  is incomplete. Complete-scope ratios become `null` with a reason. Zero verified
  successes yields `null` / `zero_verified_successes`. A zero baseline cost makes
  relative savings unavailable, including on entirely free model calls.
- Monetary inputs/rates are nonnegative decimal **strings**. Arithmetic uses
  `Decimal` with 100-digit intermediate precision. Ledger subtotals are unrounded;
  derived metrics render to 12 decimal places, `ROUND_HALF_EVEN`. No decision uses
  a rendered/rounded baseline ratio.
- Pairing and inference never supply the ledger numerator. All four outcomes,
  baseline-only losses, candidate-only gains, actual treatment exposure, provider
  versus local estimated tokens, role counts, and known/missing task clocks are
  reported. Wall-clock spans are not replaced by the sum of overlapping calls;
  small-sample p95 is deliberately not reported.
- Bootstrap samples resample entire task clusters, preserving both arms, repeated
  trials, and all allocated costs. Seeds/order/sample counts are frozen. Undefined
  zero-success draws are counted, not filtered into favorable finite intervals.
  One-cluster, incomplete, or degenerate intervals are withheld with reasons.

Reports are deterministic for the same immutable bytes. No render-time timestamp
is added. `report.md` embeds the exact JSON report, so missing evidence, unfavorable
results, uncertainty, and blockers cannot disappear in the readable version.

### Evidence contracts (`schema_version: "1.0"`)

These are a separate `contextdoctor.*` schema family; existing paired,
`MeasurementEvent`, `CompressionReport`, and public Tokenfold CLI schemas do not
change. Unknown versions, extra/missing contract fields, duplicate JSON keys/IDs,
invalid hashes, nonfinite/floating JSON numbers, booleans as counts, negative
money, mixed currencies, unsafe paths, and invalid event transitions are refused.
Use `null`, not omitted fields, for explicitly unavailable values. The two public
fixture directories and strict validators in `contextdoctor.py` are executable
examples of the exact field sets.

| Artifact | Required fields |
| --- | --- |
| Manifest | `schema_version`, `kind: contextdoctor.experiment`, `experiment_id`, `content_hash`, `arms`, `tasks`, `pins`, `verifier`, `schedule`, `limits`, `cost_policy`, `analysis`, `bindings`, `execution`, `rollout` |
| Arm definition | `name`, `transformation` (`identity`, `tokenfold`, `external` for recorded imports), `config_hash`; keys are exactly `baseline` and `candidate` |
| Task | `id`, `source_hash`, `prompt_hash`, `snapshot_hash`, `reference_hash`, `provenance` |
| Provenance | `class` (`synthetic_mechanics`, `development`, `held_out`), `family`, `selection`, `permission`, `exposure`; do not mix classes in one experiment |
| Runtime pins | `model`, `revision`, `runtime`, `device`, `dtype`, `tokenizer_hash`, `settings_hash`, `tools_hash`, `cache_policy` |
| Verifier | `digest`, `kind` (`recorded` or `exact_match`), `reference_health` (`passed`/`invalid`), `spec_hash` |
| Scheduled trial | `task_id`, `trial_id`, `order` (both arms, once each); every task has the same trial IDs |
| Limits | `max_operations`, `max_seconds`, `operation_seconds`, `max_output_bytes`, `max_tokens`, `retries: 0`, `max_cost`; time/money are decimal strings |
| Cost policy | `currency`, `rounding: ROUND_HALF_EVEN`, `categories`, `rates` |
| Each category | `coverage`, `basis`, `allocation` (`per_episode` or shared setup `equal_arms`) |
| Each rate | `model`, `metric` (`input`, `cache_read`, `cache_write`, `output`), `rate`, `per` (positive integer), `currency`, `evidence_hash` |
| Analysis | `mode` (`diagnostic`/`claim`), `seed`, `resamples` (100–10000), `confidence`, `min_tasks`, `min_trials`, `noninferiority_margin`, `regression_ceiling`, `method`, `multiplicity: single_contrast`, `sample_size_rationale`, `review_hash` |
| Bindings | `operations` (relative JSONL path), `artifacts` (digest → relative path); no network URLs, absolute paths, traversal, drive-relative paths, or symlink/junction escapes |
| Episode | `schema_version`, `kind: contextdoctor.episode`, `id`, `experiment_id`, `manifest_hash`, `task_id`, `trial_id`, `arm`, `pins_hash`, `source_hash`, `prompt_hash`, `snapshot_hash`, `transform_hash`, `status`, `request_hash`, `output_hash`, `verification_hash`, `treatment`, `operation_ids`, `wall_seconds`, `safety_violation` |
| Treatment | `enabled`, `disposition` (`applied`, `no_op`, `fallback`, `refused`, `failed`, `unknown`), `input_hash`, `output_hash`, `receipt_hash` |
| Operation event | `schema_version`, `kind: contextdoctor.operation`, `operation_id`, `episode_id` (null for shared setup), `experiment_id`, `manifest_hash`, `event`, `role`, `category`, `status`, `duration_seconds`, `usage`, `cost`, `request_id` |
| Usage | Null, or `model`, `source` (`provider_observed`/`local_estimate`), `normalization` (`disjoint_v1`/`cumulative_last_v1`), `snapshots` (the four nonoverlapping integer counters above) |
| Cost | `basis`, `amount`, `currency`, `receipt_hash`, `reconciled`, `missing_reason`; billed amounts require receipt digests, unreconciled amounts require a reason |
| Verification receipt | `schema_version`, `kind: contextdoctor.verification`, `episode_id`, `manifest_hash`, `output_hash`, `verifier_hash`, `independence` (`external`/`agent`), `reference_health`, `status`, `checks` (`id`, `result`), `evidence_hash` |

Hash syntax is `sha256:` followed by 64 lowercase hex digits. Hash artifacts as
**raw bytes**. For manifest `content_hash`, hash UTF-8 JSON with sorted keys,
compact separators, `ensure_ascii=False`, and no floating values; exclude only
the top-level `content_hash` and local `bindings`. Other canonical pins hash the
entire supplied object. `canonical_digest` implements this rule. Bind local source,
prompt/reference and receipt paths to their raw content digests separately.
IDs permit 1–100 ASCII letters, digits, underscores, dots, and hyphens. Each input
artifact is limited to 32 MiB.

Operations require a durable dispatch followed by at most one terminal event.
Identity/category/role cannot change at terminal completion. Dispatch-only events
remain unknown completion/billing. Provider cumulative snapshots are monotonic
and **last-wins**, never repeatedly added. Disjoint snapshots must be singular.
Cached counters overlap the provider's total prompt count and must be subtracted
before constructing disjoint input. Missing provider breakdowns remain unknown.
Conflicting duplicate dispatches, terminal events and billing receipts are refused.
Supplied output/state, verifier-evidence and price-source artifacts are checked
against their digests. Raw outputs may be omitted when an independent receipt
attests their digest; omission is not permission to supply tampered bytes. Required
rate-source evidence must be available for complete usage-priced economics; its
absence keeps any calculated subtotal visible but blocks complete-scope ratios.

Independent success requires a healthy external verifier with consistent checks,
bound to the exact episode/output/verifier/manifest. Agent text saying “done”,
structural preservation, or compression reports cannot create a success. A hash
proves integrity, **not independent authorship**: an importer remains responsible
for provenance, truthful receipts, verifier independence, and lawful task supply.

### Conservative legacy import

```sh
python eval/contextdoctor.py import-legacy --manifest experiment.json --records old-paired.jsonl --out NEW_PRIVATE_IMPORT_DIRECTORY
```

The importer preserves the original JSONL bytes and every measurement. It groups
experiments/models before using the unchanged legacy paired helpers, so unrelated
experiments cannot pair. The only admitted legacy trial mapping is the predeclared
`s<seed>-a<attempt>` schedule ID. Experiment, model, policy hash and snapshot gaps
are explicit. Unclassified cost observations retain their amounts without
inventing a currency or basis. Legacy success labels cannot become independent
receipts or a complete economics ledger: this import is diagnostic-only, even if
legacy pairing looks favorable. Original benchmark artifacts are never rewritten.

### Explicit execution and approvals

Execution is opt-in, **not part of default CI**:

```sh
python eval/contextdoctor.py plan --manifest experiment.json --out NEW_PLAN_DIRECTORY
python eval/contextdoctor.py run --manifest experiment.json --approval local-approval.json --out NEW_PRIVATE_RUN_DIRECTORY
```

`execution` is null for recorded reports, otherwise it has exactly `adapter`,
`image`, `agent_argv`, `tokenfold_argv`, `model`, `provider`, `settings`, `catalog_hash`.
Settings are `temperature` (decimal string) and `max_tokens`; their canonical hash
must match `pins.settings_hash`. Input hashes resolve through `bindings.artifacts`.

The locally reviewed approval has exactly:

```json
{
  "schema_version": "1.0",
  "kind": "contextdoctor.approval",
  "manifest_hash": "sha256:<manifest digest>",
  "schedule_hash": "sha256:<from plan.json>",
  "execution_hash": "sha256:<from plan.json>",
  "implementation_hash": "sha256:<contextdoctor_run.implementation_hash()>",
  "python_hash": "sha256:<raw Python executable digest>",
  "expires_at": "<future UTC YYYY-MM-DDTHH:MM:SSZ>",
  "adapter": "container",
  "limits": {"...": "copy all exact frozen limits"},
  "image": "sha256:<locally present approved Linux image ID>",
  "docker_path": "<absolute local Docker executable>",
  "docker_hash": "sha256:<raw Docker executable digest>",
  "external_data": "approved_local_inputs",
  "credential_env": null
}
```

This is an illustrative template, not valid JSON evidence: replace placeholders
and copy exact limits. A downloaded manifest is not permission to execute it.
Review the tasks, executables/image, argv, data handling, caps, and expiry locally
before supplying the approval. `implementation_hash()` binds all three modules;
`executable_hash(Path(...))` streams executable hashing. Neither helper executes a
model. A changed implementation, schedule, manifest, image, executable or cap needs
a new approval. There is no automatic approval, retry, resume or provider switch.

The current local executor is deliberately narrow:

- Immutable **text** snapshots; each task's snapshot equals its source hash.
  Source/query inputs, never reference answers or the other arm's output, reach
  the assigned agent. The fixed host verifier compares stripped UTF-8 output and
  private reference text after the agent ends. Its digest is
  `hash_bytes(EXACT_VERIFIER.encode())`; `spec_hash` is the canonical digest of
  `{"algorithm": EXACT_VERIFIER, "references": {task_id: reference_hash}}`.
- Locally approved, already present Linux Docker image; no image downloads, mounts,
  host networking, GPUs, inherited user home/config/credentials, shell interpolation,
  image volumes, or arbitrary host verifier/plugin code. Fresh nonroot containers
  use network none, read-only root, dropped capabilities, no-new-privileges,
  CPU/memory/PID limits, and a bounded temporary filesystem. The approved image
  must not embed task references/hidden checks. This is an OS isolation boundary,
  not a claim that containers are suitable for arbitrary hostile code.
- `agent_argv` is an approved absolute image executable plus arguments. It receives
  UTF-8 JSON `{"context": "...", "query": "..."}` on stdin and returns answer
  bytes on stdout. Generated output is never executed. There is no universal
  agent/tool/session framework. Model usage is unavailable unless an admitted
  adapter actually observes it; plain process output does not invent billing.
- Identity or the existing Tokenfold CLI only. `tokenfold_argv` starts with an
  absolute executable, `compress`, `-`; supported option/value pairs are
  `--format`, `--target-tokens`, `--preset`, `--encoding`. Its canonical digest
  must equal the arm config hash. The real stderr compression receipt (supported
  versions 1.0/2.0) is preserved; bypass is fallback, not successful compression.
  Pruning/retrieval, required-group/Select execution and arbitrary transforms need
  separately admitted adapters; they are refused rather than silently losing
  recovery semantics. Recorded external evidence is still supported.
- Every transform, agent, verifier and setup operation reserves a call/time slot,
  flushes dispatch before starting and terminal facts before the next operation.
  Setup uses frozen `infrastructure.allocation: equal_arms`. Aggregate/per-operation
  time, combined stdout/stderr, tokens, and operation limits apply. Execution is
  capped at one hour, 8 MiB IPC/output and 4096 output tokens per operation; all
  paid execution is refused (`max_cost` must be zero).
- Timeout/excess output/operational error stops the scope. Remaining cells stay
  unexecuted. Container force-removal cancels descendants; uncertain cleanup is
  failure, never permission to continue. Trusted transport shims are not themselves
  an untrusted-host-code sandbox. If Docker/isolation cannot be established, refuse.

Raw inputs, output and provider/compression receipts remain under the new run's
`private/` directory. Keep that directory and approval private and out of Git.
POSIX creation modes restrict access; Windows inherits the destination's ACLs, so
choose a private local parent. Reports contain aggregates and opaque IDs/digests,
not prompts or answer text. Hashes are **not anonymization**. No upload or automatic
deletion/retention policy is provided. A hard kill/power loss may leave an unfinished
dispatch; offline reporting still retains it and its missing terminal evidence.

### OpenRouter free-model testing (opt-in)

Only exact `author/model:free` IDs are admitted—not paid IDs, automatic routers,
model arrays, BYOK, tools/plugins, redirects, retries, or provider fallbacks.
Select an exact provider endpoint tag, retrieve its zero-price metadata, and freeze
that catalog in the manifest's artifact bindings:

```sh
# Set OPENROUTER_API_KEY privately in the local environment; never put it in files/chat.
python eval/contextdoctor_openrouter.py catalog --model AUTHOR/MODEL:free --provider EXACT/PROVIDER_TAG --out NEW_CATALOG.json
```

Catalog collection explicitly accesses the network but makes no model call. The
catalog has `schema_version`, `kind: contextdoctor.openrouter_catalog`, `model`,
`canonical_model`, `provider`, `pricing`, and `supported_parameters`. Missing
prompt/completion prices, unknown or nonzero price categories, and missing
token-limit support are refused. An explicitly zero `discount` metadata field is
removed before freezing the catalog; nonzero/unknown discounts are refused.
Omitted request/cache prices stay omitted: zero request ceilings still apply, but
unreported applicable prices block complete economics rather than becoming zero.
Availability of particular models/providers changes; no model is chosen by default.

For `execution.adapter: openrouter_free`, freeze `model`, `provider`,
`catalog_hash`, settings and synthetic task inputs; set the approval's
`external_data: synthetic_only`, `credential_env: OPENROUTER_API_KEY`, and matching
adapter/image. With identity transformations, image/Docker fields can be null.
Containerized Tokenfold transformations additionally need the image/Docker pins.
Before model dispatch, a separately accounted metadata request checks the frozen
endpoint catalog again. Requests pin provider `only`, disable fallbacks, require
supported parameters, and set zero prompt/completion/request/image price ceilings.
The token/call/time/output caps apply to transport subprocesses. Credentials go
only to the data-only HTTPS worker, never into artifacts or local containers.

Only explicitly approved synthetic text is admitted by this remote adapter.
Remote output is data, not an executable agent. Provider revision/device/cache
control cannot be independently guaranteed; results are diagnostic, not held-out
qualification. A timeout cannot prove remote inference stopped or billing was
reconciled; stop and retain unknown evidence. Nonzero returned charges are retained
as unreconciled provider-reported charges, and stop the scope. No free access claim
means zero hardware, subscription, human-time or all-in cost. Usage-priced rates
for live free-model execution must include all four metrics, be zero, use USD, and
bind to the frozen catalog. Imported records with absent price provenance remain
incomplete; missing usage/cache breakdowns stay unavailable.

For free-model tests, evaluate input/total token counts, independently verified
answers, paired regressions and API latency **separately from dollars**. Missing
billing evidence does not erase those diagnostics. Valid provider usage and request
IDs survive rejected/truncated responses, but missing final answers do not become
verified successes. Label local-tokenizer counts separately from provider-observed
counts; neither lossless JSON parity nor one successful canary proves task quality
on a real workload. Latency comparisons need repeated counterbalanced trials;
one ordered pair is descriptive only. A zero charge cannot establish monetary
savings, and incomplete economics can remain inconclusive while token/quality
observations are still useful.

Controls follow OpenRouter's official [provider routing](https://openrouter.ai/docs/guides/routing/provider-selection),
[endpoint metadata](https://openrouter.ai/docs/api/api-reference/endpoints/list-all-endpoints-for-a-model),
and [response/usage documentation](https://openrouter.ai/docs/api/reference/overview).
API contract tests are offline fakes, not proof that a live provider honors controls.
HTTP failures retain only a validated status code (for example,
`provider_http_429`), not the error body, headers or URL, and stop without retry.
Only `finish_reason: stop` is accepted as a final answer; truncated or filtered
text retains its valid usage/receipt but cannot become a verified success.

### Live comparison status (2026-11-09)

A fresh synthetic pilot froze four task families, four trials each, randomized
task order and two raw-first/two compressed-first trials per task. It used
`liquid/lfm-2.5-2.6b:free` at the exact `liquid/fp8` endpoint, temperature zero,
4096 output tokens per call, at most 32 calls/30 minutes, and no retries/fallback.
The trusted native Tokenfold 0.5.3 CLI precomputed **conservative** JSON contexts;
value and insertion-order parity were asserted before dispatch. This is not a
Select, pruning, reversible dictionary-encoding or end-to-end agent comparison.

The scope stopped on call six with `process_failed`; the precise remote cause
and that call's completion/usage remain unknown. It was not resumed. Of 32 planned
cells, six were attempted, five had independently exact-verified final answers,
and 26 were unexecuted. Two pairs completed; the third task has a successful
unpaired candidate. No repeated trial completed, so the broader P3 pilot remains
**incomplete**. Previous truncated-baseline scopes remain failures, not discarded
attempts or retries in this scope.

| Completed task pair | Provider input tokens raw / compressed | Provider total tokens raw / compressed | Verified answers raw / compressed | API seconds raw / compressed |
| --- | --- | --- | --- | --- |
| Negation/disabled override | 389 / 250 | 518 / 388 | pass / pass | 0.828 / 0.656 |
| Numeric active-order filter | 387 / 248 | 560 / 362 | pass / pass | 2.172 / 1.688 |

These two completed pairs observed 35.8% fewer provider input tokens and 30.4%
fewer total tokens, with no answer regression **in those pairs only**. They are
not complete-scope savings: the unpaired candidate consumed another 272 input/
347 output tokens and 1.734 API seconds, and the failed call has unknown usage.
API timing includes transport/process overhead, excludes precomputed compression
and rate-limit spacing, and is descriptive only. Two synthetic pairs with no
executed repetitions cannot establish latency improvement, general response-quality
preservation, or meaningful claim uncertainty. Free reported charges are not
invoices or evidence of monetary savings.

The private frozen manifest is
`sha256:62631f840f3ea8290a68c41245331030de7deb9bbfba6a5fe6487910e3719486`.
Raw inputs/answers, receipts, approvals and partial events stay local and out of
Git. This summary is not a distributable reproduction bundle. A fresh exact scope
is required for further calls; held-out supply, independently reviewed inference
and repeat-use/maintenance evidence remain absent. P4 claims and P5 expansion
therefore remain blocked; keep the experimental capability small.

### Claim readiness, rollout and exit codes

`diagnostic` is the default. Verdicts are `invalid`, `inconclusive`, `regressed`,
`no_effect`, and conditionally `beneficial_for_declared_scope`. Observed losses and
safety violations are never erased by gains or lower token counts. Separate
`quality_gate`, `economics_gate`, `evidence_complete`, and `claim_blockers` expose
why a favorable point estimate is insufficient.

Claim mode requires held-out provenance, sufficient predeclared independent tasks
and trials, lawful exposure/selection, complete comparable single-basis economics,
healthy independent verification, actual changed treatment, no observed losses or
safety violations, positive nondegenerate task-cluster cost improvement intervals,
and a supported independently reviewed policy. The only supported conservative
quality-bound policy is `task_hoeffding_bonferroni`: task-level success/loss rates,
Hoeffding bounds with Bonferroni allocation, success-delta lower bound and CFR
upper bound compared with the frozen noninferiority margin/regression ceiling.
The cost bootstrap still requires independent methodological review; there is no
universal threshold or inferred sample-size justification.

A `contextdoctor.method_review` artifact contains `schema_version`, `kind`,
`analysis_hash`, `method`, `independent`, `approved`, `reviewer`, `evidence_hash`.
Its analysis hash excludes only `review_hash`. A declaration/hashing mechanism
cannot authenticate the reviewer, task independence, representative sampling or
true independence. Obtain those reviews outside the tool before interpreting a
claim gate. Passing it is not Select qualification or a release waiver.

`rollout` is null or `pilot_hash`, `maintenance_minutes`, `benefit_minutes`,
`repeat_uses`, `decision` (`keep_small`, `stop`, `expand`). A prior
`contextdoctor.pilot` receipt binds `report_hash`, `approval_hash`, `adapter`,
`completed`, and `evidence_class` plus schema/kind. Bind those prior artifacts in
the new experiment. `run` produces `pilot.json` even for partial/synthetic runs;
that does **not** make them completed development evidence. Readiness checks prior
report/approval binding, development classification and completeness. Expansion
also needs at least two repeat uses, recorded user benefit exceeding maintenance,
and an explicit expand decision. Missing evidence keeps the capability small.

The software supports contract/report/execution/readiness/integration work, but
does not manufacture P3 pilot completion, P4 independent review/held-out results,
or P5 demonstrated repeat-use value. `readiness` is offline and does not probe
runtime availability. No standalone platform or extra public CLI wrapper is added.

| Exit | Meaning |
| --- | --- |
| 0 | Successful diagnostic/report operation, even if the observed result regressed |
| 1 | Execution or I/O failure; preserve partial artifacts |
| 2 | Invalid arguments, unsupported/malformed contract or approval |
| 3 | `report --require-benefit` requested a gate that was not met; report still written |

Default CI runs only synthetic/offline checks. Existing fidelity, compression,
release, proxy, paired and binding gates remain separate. A real pilot requires a
frozen approved scope; rerunning after a failure is a new scope, not an implicit retry.

For an explicitly approved **already present** test image containing
`/usr/bin/python3`, set `CONTEXTDOCTOR_TEST_IMAGE` to its immutable image ID and
`CONTEXTDOCTOR_DOCKER_PATH` to the absolute approved Docker executable before
running `test_contextdoctor_run.py`. This opts into a real-container boundary
check; without both settings it is explicitly skipped. No image is pulled.
The opt-in check covers nonroot execution, credential absence, read-only root,
blocked external networking, and timeout removal of a container with a child
process. These checks passed locally on Windows with Docker Desktop and an
already-present immutable image, under actual Python 3.9.25. The Windows junction
escape test also passed; three Windows symlink cases remained skipped for lack
of symlink privileges. This does not qualify arbitrary hostile host processes or
all Windows filesystem races. Hosted CI separately tests offline contracts on
Windows/Linux with Python 3.9 and 3.12; it does not run providers or Docker.
