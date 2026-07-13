# Fixture Registry — `tests/fixtures/` and `tests/golden/`

This is the fixture-policy approval record required by `ENGINEERING.md`
("Fidelity Tests" → Fixture policy) and `ROADMAP.md` (Phase 4 exit checklist:
"Every fixture in `tests/fixtures/**` and `eval/tasks/**` has: data
classification, license/source note, PII scan result, retention owner,
approval record"). `eval/tasks/**` has its own record at
`eval/tasks/FIXTURES.md`; this file covers `tests/fixtures/**` plus
`tests/golden/**` (byte-exact golden fixtures, listed in
`tests/golden/MANIFEST.toml`), which are covered here for completeness even
though they sit outside the literal `tests/fixtures/**` glob.

## Fixture set: `tests/fixtures/mode_matrix.toml`

One TOML file: the mode/ratio matrix for the four non-mandatory transforms
(`json_minify`, `schema_compaction`, `log_compaction`, `diff_compaction`).
Contains no payload text at all — only transform IDs, version strings,
booleans, and ratio numbers mirrored from `crates/tokenfold-core/src/modes.rs`.

- **Data classification:** `public`. Structural config data, not content.
- **License/source:** Authored for this project (Phase 2); mirrors the Rust
  source of truth in `modes.rs`.
- **PII/secret scan result:** N/A — no free-text or payload fields exist in
  this file's schema.
- **Retention owner:** Project maintainer.
- **Approval record:** Authored and self-approved as part of the Phase 2
  transform/pipeline bootstrap; verified in CI by
  `integration.rs::mode_matrix_fixture_mirrors_the_rust_source_of_truth`.

## Fixture set: `tests/golden/**` (see `tests/golden/MANIFEST.toml`)

8 golden input/output pairs across `json_minify` (3), `log_compaction` (2),
`schema_compaction` (1), and `diff_compaction` (1), plus the manifest itself.

- **Data classification:** `public`. All inputs are short, hand-authored
  synthetic snippets (a toy JSON object, a fake tool schema, fake log lines,
  a fake unified diff) written specifically to exercise one documented
  transform behavior each (whitespace stripping, key-order preservation,
  duplicate-line collapsing, examples-array shortening, diff hunk
  preservation). None is captured output from a real system or real user
  content.
- **License/source:** Originally authored for this project as part of the
  Phase 2 golden-test bootstrap (`crates/tokenfold-core/tests/golden.rs`).
  No external source, no third-party license implications.
- **PII/secret scan result:** None — synthetic placeholder text only. No
  real names, credentials, IPs, or identifiers.
- **Retention owner:** Project maintainer.
- **Approval record:** Authored and self-approved as part of the Phase 2
  golden-test bootstrap. Byte-exactness is enforced by
  `tests/golden/MANIFEST.toml`'s SHA-256 field and the Rust golden-test
  runner; any change to expected bytes requires a deliberate manifest update
  per the "Updating golden fixtures" note at the bottom of that file.
