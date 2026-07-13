# Fixture Registry — `eval/tasks/`

This file is the fixture-policy approval record required by `ENGINEERING.md`
("Fidelity Tests" → Fixture policy) and `ROADMAP.md` (Phase 2 exit checklist:
"Every fixture in `tests/fixtures/**` and `eval/tasks/**` has: data
classification, license/source note, PII scan result, retention owner,
approval record").

## Fixture set: `eval/tasks/smoke/`

10 hand-authored fixtures (`task_001.json` – `task_010.json`), covering four
transform IDs relevant to Phase 2 scope:

| `transform_id`      | Count | Fixtures                          |
|---------------------|-------|------------------------------------|
| `log_compaction`     | 3     | task_001, task_002, task_003       |
| `diff_compaction`     | 3     | task_004, task_005, task_006       |
| `json_minify`         | 2     | task_007, task_008                 |
| `schema_compaction`   | 2     | task_009, task_010                 |

Each fixture is a JSON object:

```json
{
  "id": "task_001",
  "transform_id": "log_compaction",
  "task_scope": "general",
  "original": "...",
  "compressed": "...",
  "critical_tokens": ["..."]
}
```

- `original` / `compressed` are short (a few lines each), hand-authored
  illustrations of that transform's documented behavior (see `PLAN.md`'s
  transform table and `ENGINEERING.md`'s ML-contributor prototyping guide),
  not output captured from the real Rust transforms.
- `critical_tokens` are 1-3 short substrings (a unique error code, file
  path, request/order ID, or schema field name) that must survive verbatim
  in `compressed` for the compressed text to remain trustworthy.

### Fixture policy checklist (`smoke`)

- **Data classification:** `public`. All 10 fixtures are synthetic,
  hand-authored placeholder text (fake log lines, fake diffs, fake JSON
  payloads, fake tool schemas). None of it is real user, customer, or
  production data.
- **License/source:** Originally authored for this project as part of the
  Phase 2 fidelity-smoke-gate bootstrap (this commit). No external source,
  no third-party license implications.
- **PII/secret scan result:** None — synthetic placeholder text only. No
  real secrets, credentials, names, email addresses, or identifiers. IDs
  such as `req-77a1`, `ORD-88231`, `cus_1029`, `INV-7742`, and `user:9931`
  are fabricated for illustration only and do not correspond to any real
  request, order, customer, invoice, or user.
- **Retention owner:** Project maintainer.
- **Approval record:** Authored and self-approved as part of the Phase 2
  fidelity-smoke-gate bootstrap (`eval/run_fidelity.py`, this fixture set).
  No separate reviewer sign-off has occurred yet; treat this fixture set as
  a mechanism-proving bootstrap, not a reviewed, first-consumer-representative
  corpus.

## Fixture set: `eval/tasks/full_lossy/`

16 hand-authored fixtures (`flp_001.json` – `flp_016.json`) backing the
`full-lossy-promotion` gate profile (ROADMAP.md Task 9): an accuracy@ratio
sweep, the contrastive KPI, and critical-token needle-survival for the two
transforms still behind `--experimental`, across all four content types in
scope for that profile (command output, git diff, JSON, prose).

| `transform_id`    | Count | Content types (2 fixtures each: light/heavy ratio) | Fixtures        |
|-------------------|-------|----------------------------------------------------|-----------------|
| `log_compaction`  | 8     | command_output, json, prose, git_diff              | flp_001-flp_008 |
| `diff_compaction` | 8     | git_diff, command_output, json, prose              | flp_009-flp_016 |

Each fixture is a JSON object with the same shape as `eval/tasks/smoke/`'s,
plus one additional field:

```json
{
  "id": "flp_001",
  "transform_id": "log_compaction",
  "task_scope": "general",
  "content_type": "command_output",
  "original": "...",
  "compressed": "...",
  "critical_tokens": ["..."]
}
```

- `content_type` is one of `command_output` / `git_diff` / `json` / `prose`
  — descriptive metadata for the accuracy@ratio breakdown, not consumed by
  `pipeline_for`'s `InputFormat` routing.
- Unlike `eval/tasks/smoke/`'s fixtures (which just illustrate documented
  behavior loosely), every `light`/`heavy` pair here is built by hand-tracing
  the real Rust algorithm each transform ships with (`transforms::logs::compact`'s
  `[repeated Nx]` adjacent-run collapsing; `transforms::diff::compact_diff`'s
  structural/change/context line classification, with `light` = `code_review`
  (`keep_line_bodies = true`, context-only dropped) and `heavy` = `change_summary`
  (`keep_line_bodies = false`, header-only) — see `crates/tokenfold-core/src/pipeline.rs`'s
  `keep_line_bodies = policy.task_scope != TaskScope::ChangeSummary`). `compressed`
  never rewords a surviving line/field/sentence — it only ever drops whole
  ones and inserts a bracketed evidence marker, exactly like the real
  transforms do. The `ratio` sweep point (`light` vs. `heavy`) comes from how
  much is dropped (more consecutive repeats for `log_compaction`; context-only
  vs. header-only for `diff_compaction`), not from paraphrasing what survives.
  **Correction (2026-07-12):** `flp_013`-`flp_016` (`diff_compaction`'s `json`/
  `prose` content-type pairs) originally violated this rule — their
  `compressed` fields depicted a hypothetical smarter, field-aware compaction
  (e.g. `"note": "[2 metadata fields dropped]"`) that `compact_diff` does not
  implement. Verified by compiling and running the real algorithm against
  each fixture's `original`; the actual output for all four is a single
  evidence marker (`"[25 context lines dropped]"` / `"[25 lines dropped]"` /
  `"[6 context lines dropped]"` / `"[6 lines dropped]"`), because `classify_line`
  only recognizes literal unified-diff line prefixes (`diff --git`, `index `,
  `--- `, `+++ `, `@@`, `+`, `-`) — JSON and prose content never contains
  those, so every line is `Context` and the whole input is dropped, critical
  tokens included. Fixtures were corrected to this real output; see "Scorer
  status" below for what that reveals about `diff_compaction`'s promotion
  eligibility.
- `critical_tokens` for `diff_compaction`'s `heavy` (header-only) fixtures are
  deliberately scoped to identifiers that survive in `Structural` lines (file
  names, hunk headers) rather than ones embedded only in dropped change
  bodies — this matches the transform's actual design intent (header-only
  mode is only ever selected for `TaskScope::ChangeSummary`, where file/hunk
  identity is what must survive, not exact code content). This assumes the
  input actually contains diff-syntax `Structural` lines in the first place;
  for the `json`/`prose` content-type fixtures there are none, so the chosen
  critical tokens (file name, error code) do not survive either — a real,
  now-corrected measured gap, not a fixture-choice problem.

### Fixture policy checklist (`full_lossy`)

- **Data classification:** `public`. All 16 fixtures are synthetic,
  hand-authored placeholder text (fake package-manager/CI output, fake git
  diffs and Terraform-style resource diffs, fake JSON log lines and diff
  records, fake incident/PR narration). None of it is real user, customer,
  or production data.
- **License/source:** Originally authored for this project as part of the
  Phase 5 / Task 9 full-lossy-promotion fidelity gate (this commit). No
  external source, no third-party license implications.
- **PII/secret scan result:** None — synthetic placeholder text only. No
  real secrets, credentials, names, email addresses, or identifiers. IDs
  such as `ORD-5521`, `ORD-9004`, `REL-4471`, `INV_TIMEOUT`, `INV_STALE`,
  and `RL_429` are fabricated for illustration only and do not correspond to
  any real order, release, or incident.
- **Retention owner:** Project maintainer.
- **Approval record:** Authored and self-approved as part of the Phase 5 /
  Task 9 full-lossy-promotion gate (`eval/run_fidelity.py`, this fixture
  set). No separate reviewer sign-off has occurred yet — same caveat as the
  smoke fixture set above: this is a mechanism-and-signal-proving bootstrap
  corpus, not a reviewed, first-consumer-representative one.

## Scorer status: deterministic bootstrap, not a real quality judge

Both `eval/run_fidelity.py` profiles score every fixture with a
**deterministic lexical-overlap proxy** (whitespace-token set overlap
between `original` and `compressed`) instead of a real downstream-task
quality judgment, because no live model judge is wired up yet (no
`ANTHROPIC_API_KEY` is configured, and the real thresholds are still an open
decision — see `ROADMAP.md` § D-005, status OPEN). `smoke-first-consumer`
hardcodes `contrastive_failure_rate` to `0.0` for every fixture (no
contrastive check is attempted there at all); `full-lossy-promotion`
computes a less trivial, but still deterministic and still not
live-judged, contrastive proxy (see `run_fidelity.py`'s
`_contrastive_failure_proxy`). Upgrading to a real LLM-judged accuracy@ratio
harness — with a pinned model version and live credentials, producing
genuine `quality_retention` and `contrastive_failure_rate` numbers against
real downstream tasks — is tracked as future work, to be done once D-005's
fidelity thresholds are finalized. Until then, a passing gate here proves
the harness mechanism (and, for `full-lossy-promotion`, a real if crude
lexical signal) works — it does not certify that any transform meets the
real fidelity bar a live judge would apply.

Note on `full-lossy-promotion`'s measured result (see `ROADMAP.md` Task 9,
2026-07-12 re-investigation): the lexical-overlap proxy is known to
underscore transforms that deliberately drop unchanged context on the
theory that context is recoverable (a live judge could recognize that; this
proxy just counts missing words). That alone was the original hypothesis
for why `diff_compaction` failed the gate — its fixture set blends the
default, body-preserving form (`task_scope` != `change_summary`, 4
fixtures) with the lossier header-only `change_summary` form (4 fixtures)
into one blended `quality_retention`/`contrastive_failure_rate` number for
`diff_compaction`. `run_fidelity.py`'s `per_variant` breakdown (added this
pass) separates them, and the hypothesis is *confirmed but incomplete*:
splitting the two variants does not rescue the default form.

The full explanation has two parts:
1. **Confirmed:** the two `task_scope` variants were blended together, and
   the header-only form is (by design) much lossier than the default form.
2. **The real, deeper cause:** 4 of the 8 `diff_compaction` fixtures
   (`flp_013`-`flp_016`, the `json`/`prose` content types) had fabricated
   `compressed` fields that did not match the real transform's output (see
   the "Correction" note above). Once corrected to the verified real
   output, both `task_scope` variants score far *worse*, not better,
   because `compact_diff` has no fallback for non-diff-shaped input: it
   drops literally everything (including critical tokens) when no line
   matches a recognized unified-diff prefix. This is a genuine property of
   the shipped transform, not a scorer artifact.

Measured 2026-07-12 (after both the `per_variant` split and the fixture
correction): `diff_compaction`'s default form (`task_scope=code_review`)
scores `quality_retention=0.362`, `contrastive_failure_rate=0.5`,
`critical_token_survival_rate=0.5` — all three miss the D-005 draft
Balanced thresholds (`>=0.95` / `<=0.005` / `>=0.99`), and the header-only
`change_summary` form scores worse still (`quality_retention=0.211`,
`contrastive_failure_rate=0.75`, `critical_token_survival_rate=0.5`).
Per this task's "do not fudge the numbers" instruction, `diff_compaction`
stays `--experimental` in its entirety — neither form clears the bar on its
own — while `log_compaction` (whose behavior doesn't branch on
`task_scope` and clears every threshold at `1.0`/`0.0`/`1.0`) is promoted.
See `ROADMAP.md`'s Phase 5 fidelity-harness and promotion bullets for the
final disposition.
