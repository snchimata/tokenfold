# New-feature implementation status

Verified against `15269dd` plus the accompanying working-tree fixes on 2026-10-02.
These changes target version **0.5.1**; the existing v0.5.0 tag is unchanged.
This is an implementation audit, not a release approval or a semantic-quality certification.
The local design backlog is historical planning material; an exported helper and passing
synthetic tests do not mean its complete acceptance criteria have been met.

## Package inventory

| Package | Implemented surface | Remaining acceptance work |
| --- | --- | --- |
| EP-01 / NF-01 | Immutable OpenAI call/result fields in `tokenfold-adapters/src/lib.rs`; strict observation envelope/group checks in `observation.rs` | Broader provider/opaque-block paths are not activated by this prototype. |
| EP-02 / NF-08,17 | Core measurement events and model-aware estimator provenance; bounded JSON/SSE/NDJSON proxy usage observation; Rust/Python/Node versioned receipt readers | Session-level peaks, retrieval/transform aggregates, provider preflight counting and tokenizer/chat-template support are not complete. Existing stats placeholders are not actual measurements. |
| EP-03 / NF-06,07,09,10 | `eval/run_paired.py`: paired records, CFR, clustered uncertainty, resettable dummy sandbox, real-proxy observation arm, bounded local/free live-model runner (64 upstream attempts including retries/readiness, 1..8 turns, 1024 max output tokens, no redirects); paid models refused | Held-out workload calibration, transform ablations/causal attribution, matched total economics, and an enforced approved monetary spend cap. Paid execution is blocked even with the legacy allow flag until a priced spend-capped runner exists. |
| EP-04 / NF-01,02,16 | Disabled-by-default OpenAI tool-result JSON adapter, group validation, envelope preservation, thresholds, no-growth fallback and a bounded session commit ledger with opt-in locked SHA-256 fingerprint snapshots, restart expiry and corruption/write-failure guards | Independent history triggers/reserves/cooldowns, full history compaction and measured provider-cache/quality qualification. Observation commitments survive proxy restarts only with the explicit host-owned ledger path. |
| EP-05 / NF-05,15,21 | Versioned metadata, leases, batched quota API wired into Rust/CLI/MCP/Python compression, TTL protection, bounded authorized exact retrieval, MCP trusted-root alignment and restart recovery tests; existing Claude setup | Continuation/total session restoration budget, automatic host-owned history insertion, and install/edit/uninstall/cancellation/task recovery matrix. No Codex interception installation is supplied. |
| EP-06 / NF-13,19 | Atomic caller-declared group allocator and versioned score validation in Core; checked/saturating cost admission; experimental CLI/library adapter maps text/IDs, invokes an explicitly approved pinned direct-child scorer with deadline/cancellation, validates responses and recounts complete prompts | Bundled reviewed model/runtime, OS/descendant sandboxing, provider chat-template qualification and quality/latency curves. `json_prune` retains its existing separate allocation algorithm. |
| EP-07 / NF-03 | Exact tool-result duplicate helper with public-decoder byte round-trip and never-larger gates | Escaping/versioned self-contained dictionary frame, token rather than byte economics, host integration, and task-quality qualification. Non-canonical input or marker collisions keep the baseline. |
| EP-08 / NF-04 | Caller-declared top-level JSON state facts; explicit task/revision manifest rejects missing source identity/required keys, conflicts, truncation and full serialized-size overflow | Host task/revision association and staleness enforcement, prompt insertion and late-turn task qualification. Legacy fact collection still permits absent provenance; task-manifest activation refuses it. |
| EP-09 / NF-14 | `tokenfold-rag::EvidenceIndex`: caller-listed hashes, namespace isolation, rebuildable BM25, live expiry/deletion re-check; opt-in MCP query endpoint uses host-owned hash publication, bounded indexing and whole-entry total content budget | Automated host publication lifecycle, cumulative session token budgets, and measured retrieval/task recall. Oversized originals remain exact-hash retrievable, never truncated. |
| EP-10 / NF-11,12 | Quality floor, deterministic candidate selection, explicit named approval/rollback and revalidated knob application in Core (target tokens and consented pruning ratio; unsupported knobs reject) | versioned workload/model/corpus profiles, bounded parameter generation, optimizer/CLI activation, approval timestamps and held-out quality/cost evidence. Existing presets are unchanged. |
| EP-11 / NF-18 | Session-stable hash assignment, signed byte/output-token deltas with estimator provenance, literal guards and protected JSON-pointer value equality | Provider request-level authorized shaping, full schema validation, randomized experiment specification and output-cost reporting. A value guard is not general semantic preservation. |
| EP-12 / NF-20 | Whole-definition catalog helpers preserving JSON values/order and forced/declared companion tools; explicit query-aware lexical ranking retains caller-owned discovery and falls back for invalid/incomplete catalogs | Authorized discovery/fallback execution, protocol qualification, cache economics and executable multi-step tool tasks. Legacy selection remains first-N; value parity is not original JSON-byte parity. |
| EP-13 / NF-22 | Optional strategy trait with visible baseline fallback; only claim-backed literal extracts admitted | Reviewed generative factual/citation verification, enforceable inference deadline/cancellation, consent/routing/resource controls, independent observation/history/combined quality and drift ablations. A post-return timeout cannot stop a hung strategy. |

## Contracts repaired by this audit

- Quota checking and publication now share the backend lock, including across processes.
  Duplicate keys count once and corrupt metadata fails admission rather than hiding occupancy.
- Re-publishing an original preserves other holders' leases and cannot shorten its TTL.
  Finite unexpired TTLs survive size-pressure GC. Released leases do not cancel TTLs.
  All writers/GC processes must be upgraded together; older GC cannot honor this rule.
- MCP pruning uses the host-configured root that retrieval reads. Alternate roots and
  unauthorized write namespaces are refused before persistence; restoration succeeds after
  an MCP process restart without re-executing a tool.
- Dedup verifies its public decoder instead of an internal decoder that knew which positions
  were substituted. Previously a candidate could pass the internal gate but corrupt or fail
  public decoding. Pretty-printed inputs no longer falsely certify byte-exact recovery.
- Semantic validation checks the text actually emitted, not just claims supplied alongside it.
  Unrelated text with valid claims now falls back. Paraphrase verification is not implemented.
- Scorer responses must match the pinned model revision; negative scores retain ordering.
  NaN/infinite quality floors cannot approve profiles.
- Node receipts use a versioned reader, with archived v1 compatibility and unavailable sections
  left null. No receipt schema or golden output was changed.

The initial regression runs reproduced six Core failures, three adapter failures and two MCP
failures. Each was repaired and rerun; the multiprocessing and corruption regressions add
coverage for the storage trust boundary.

## Validation

Run from the repository root unless noted:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --locked`
- `cargo build --release --locked -p tokenfold-cli -p tokenfold-proxy`
- In `packages/tokenfold`: `npm ci`; set `TOKENFOLD_TEST_BINARY` to the built CLI; `npm test`.
- `uv tool run maturin build --release --locked -m crates/tokenfold-py/Cargo.toml`;
  install that wheel into the existing isolated `target/py-test` environment; run
  `target/py-test/Scripts/python.exe -m pytest python-tests -q --basetemp target/pytest-backlog051`.
- With `TOKENFOLD_BIN`/`TOKENFOLD_PROXY_BIN` pointing to the built release executables:
  `python eval/test_baseline_fixture_contract.py`, `python eval/test_baseline_cli.py`,
  `python eval/test_paired_runner.py`, `python eval/test_learned_selector_wiring.py`,
  `python eval/audit_quality_sample.py --check`, `python eval/test_audit_quality_sample.py`.
- `python eval/run_fidelity.py --gate --profile smoke-first-consumer`;
  `TOKENFOLD_LEARNED_MODULE=json python eval/run_baselines.py --gate`.

The deterministic fidelity gate and the baseline gate passed; baseline accounting used the
explicitly inexact heuristic estimator (1,240 cases), not actual provider request usage.
Offline/dummy task results are proxies, not evidence of live quality non-inferiority.
Formatting, strict Clippy, all 692 workspace tests, all 12 Node tests and all 19 Python tests
passed. The CLI/proxy release executables and abi3 Python wheel were rebuilt locally; no
artifacts were published. A pre-existing README encoding glitch that failed its ASCII
contract was corrected. No listed local check was skipped.

Not run: live provider/model task and cache trials, paid experiments, AppWorld/OfficeBench
qualification, multi-platform/load/security deployment trials, host installation, publishing,
or release promotion. These need their stated reviewed boundaries, data/resources, quality
margin and (where relevant) spend authorization. Optional strategies remain unqualified and
must not silently replace default presets.

## Release preparation boundary

`tokenfold-adapters` and `tokenfold-rag` now carry descriptions, crate READMEs and registry
version constraints for Core. Publication is ordered Core -> adapters/RAG -> CLI (output
and learn remain before CLI). CI verifies the new libraries by packaging and compiling
against Cargo's isolated temporary registry, not merely building workspace path dependencies.
Workspace, npm root and exact platform-package dependency versions are aligned at 0.5.1.
Platform staging tests derive the version from the manifest so release bumps do not stale them.

Verified locally: isolated Cargo packages for Core/adapters/RAG/output/learn/CLI, Windows CLI/proxy builds,
abi3 wheel and installed-wheel tests, npm build/tests and pack dry run. Local artifacts are
ignored and must not be committed. Optional strategy/catalog/state APIs remain opt-in and
carry their qualification limits in crate documentation. No registry package, Git tag or
GitHub release has been published/modified, and no host installation was performed.

**Still required before release promotion:** green release CI for Linux/macOS/Windows and
registry artifact smoke tests; approve the prepared version and normal PR/tag workflow.
**Not resolved by packaging:** the remaining integration/research acceptance work in the
inventory above, especially durable history compaction, a qualified bundled Select/generative
runtime, automatic host insertion/recovery, provider cache trials and independent held-out
quality/economics. They must not be advertised as implemented or promoted into default presets.

For 0.5.1 preparation, optional npm platform packages are not yet on the registry;
`npm ci` validates/builds the wrapper while API tests use the freshly built explicit CLI.
The release matrix installs staged tarballs directly and must pass before publication.
The pre-release lockfile must not invent platform tarball checksums.


## Follow-up integration closure

Added actual experimental `tokenfold select` activation and an executable scorer boundary,
not merely synthetic score structs. Tests spawn a real isolated worker, prove revision fallback,
kill/reap a hung worker, cancel an active call and verify whole-prompt recount/protected groups.
The worker test is intentionally ignored for direct harness invocation and executed explicitly
by the parent regression; it is not skipped validation of a shipped runtime.

Added proxy restart persistence for observation commitments with exclusive ownership, no raw
payload/ID storage, atomic snapshots and fail-closed TTL/corruption/capacity/write guards.
A black-box test kills/restarts the proxy and verifies byte-identical committed forwarding.
These close invocation/idempotence integration gaps, not history semantic-quality acceptance.

Public Rust documentation is also checked with warnings denied (`RUSTDOCFLAGS="-D warnings"
 cargo doc --workspace --no-deps --locked`) and protected by CI. Stale private/redundant
links and bare URLs were fixed without changing behavior. The CLI selection regressions
also refuse JSON-escaped secrets before either output or process invocation.
