# Migrating from v0.4 to v0.5

v0.5.0 is a breaking interface release: compression *modes* became `Preset`
values, the opt-in lossy flag family became one recoverable pruning policy, and
public redaction lost its bypass. This matrix maps each v0.4 surface to its
v0.5 replacement so an upgrade does not silently change behavior. Full detail
for every surface is in the v0.5.0 entry of [CHANGELOG.md](CHANGELOG.md).

## CLI

| v0.4 | v0.5 | Notes |
|---|---|---|
| `--mode conservative\|balanced\|aggressive` | `--preset conservative\|balanced\|aggressive` | Default remains `balanced`. |
| `--lossy heuristic --lossy-ratio <R>` | `--prune --keep-ratio <R>` | Recoverable JSON array-item pruning; generic-JSON only, unchanged gate. |
| `--lossy-preserve <path>` (repeatable) | `--preserve <path>` (repeatable, requires `--prune`) | Same whole-array protection semantics. |
| `--unsafe-disable-redaction` | removed | Secret redaction is mandatory on every public compression path. |
| `--store-originals` (opt-in whole-payload capture) | removed; pruning always persists omitted values via `--retrieval-store` | No opt-in whole-payload storage remains; `--retrieval-store <path>` + `--retrieval-namespace <ns>` configure where pruning persists values. |
| — (hard token targets were always best-effort) | `--require-target` | New in v0.5; an unmet hard target emits its receipt, suppresses payload output, and **exits 7**. |
| — (reported inline) | `--receipt-file` / `--receipt-format json\|text` | Explicit receipt routing; `compress` emits payload on stdout and receipt on stderr. |
| `--encoding` | `--encoding json\|toon` | TOON output codec is opt-in and round-trip checked. |
| retrieve flag `--retrieve-namespace` (v0.4 name) | `--retrieval-namespace` | **Renamed**: `retrieve`/`compress` now take `--retrieval-namespace`; the old `--retrieve-namespace` spelling exits 2. |
| retrieve markers | raw hash, legacy marker, or JSON `$tf_ref` | Missing or expired retrieval references now **exit 8**. |

The `--lossy heuristic` backend name is gone: there is exactly one recoverable
pruning policy, selected by `--prune` plus a target or keep ratio.

## Python

| v0.4 | v0.5 |
|---|---|
| `CompressionMode.BALANCED` | `Preset.BALANCED` (`preset=` on `compress`) |
| `CompressionPolicy(lossy=...)` | `PruningPolicy(keep_ratio=..., preserve_paths=..., retrieval_store=..., retrieval_namespace=...)` |
| `compress_bytes`-style helpers | bytes-first v2 API: `compress(...)` returns `.payload` bytes + `report` |
| text helpers (lossy decode) | text helpers decode UTF-8 strictly |
| `inspect` echoes input | `inspect` returns the receipt (never writes retrieval state) |
| hard-target miss returned inline | raises `BudgetUnmetError` carrying the receipt |

## TypeScript

| v0.4 | v0.5 |
|---|---|
| `CompressionMode` | `Preset = "conservative" \| "balanced" \| "aggressive"` |
| lossy policy object | `PruningPolicy { keepRatio?, preservePaths?, retrievalStore?, retrievalNamespace? }` |
| string payload helpers | bytes-first `CompressionResult { payload: Uint8Array, text, report }` |
| — | `BudgetUnmetError` when `requireTarget` is not met |

## Receipts, redaction, and shared surfaces

- Compressor receipts move to **schema 2.0**: independent operation, budget,
  encoding, and pruning outcomes instead of one blended status.
- Redaction is mandatory on every public compression path; the public bypass
  was removed.
- Config, proxy metadata, and MCP compression share the `Preset` naming.
  MCP retrieval takes an explicit namespace, and MCP compression rejects
  `store_originals=true` rather than ignoring it.
- Rust: `CompressionMode` → `Preset`, and the lossy policy fields become
  `PruningPolicy { keep_ratio, preserve_paths, retrieval_store, retrieval_namespace }`.

## Automation that must change

- CI and scripts that passed `--mode` or `--lossy-*` flags must use
  `--preset` / `--prune --keep-ratio --preserve` — the old flags are gone and
  the CLI rejects unknown options.
- Anything parsing receipts should expect schema 2.0 fields and the new
  exit code 7 (unmet hard target) / 8 (missing or expired retrieval reference).