# Configuration and MCP clients

CLI flags override environment variables, which override `tokenfold.toml` / `.tokenfoldrc`, which
override built-in defaults. Boolean environment values accept `1/0`, `true/false`, `yes/no`, and
`on/off` (case-insensitive). Comma-separated transform lists ignore empty items.

## MCP stdio clients

Tokenfold serves newline-delimited MCP JSON-RPC on stdio:

```sh
tokenfold mcp serve
```

The server targets the [MCP 2025-11-25 protocol](https://modelcontextprotocol.io/specification/2025-11-25/schema).

Claude Code project configuration (`.mcp.json`), also installed by
`tokenfold init --agent claude-code`:

```json
{
  "mcpServers": {
    "tokenfold": {
      "command": "tokenfold",
      "args": ["mcp", "serve"],
      "env": {}
    }
  }
}
```

This project-scoped shape follows the [Claude Code MCP configuration](https://code.claude.com/docs/en/mcp).

Codex user configuration (`~/.codex/config.toml`), or run
`codex mcp add tokenfold -- tokenfold mcp serve`:

```toml
[mcp_servers.tokenfold]
command = "tokenfold"
args = ["mcp", "serve"]
enabled = true
```

The server exposes `tokenfold_compress`, `tokenfold_inspect`, `tokenfold_retrieve`, and
`tokenfold_stats`. `tokenfold_retrieve` and `tokenfold_stats` intentionally read only their listed
environment overrides; MCP compression arguments do not load a project config file.

For recoverable MCP pruning, configure `TOKENFOLD_RETRIEVAL_STORE_PATH` in the host's
MCP server environment. Compression and retrieval use this same trusted root, including
after restart. A tool argument cannot select another root. Without an override, both use
the default persistent store. Pruning requires the `filesystem` backend, and configured
`TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES` restricts writes as well as reads. A request
for another root or an unauthorized namespace returns a tool error before any write.
This is explicit payload/retrieval integration, not automatic history mutation or tool replay.

## Environment overrides

| Variable | Value | Applies to MCP |
| --- | --- | --- |
| `TOKENFOLD_CONFIG` | Explicit config-file path | No |
| `TOKENFOLD_COMPRESSION_MODE` | `conservative`, `balanced`, `aggressive` | No; use tool argument |
| `TOKENFOLD_COMPRESSION_TARGET_TOKENS` | Non-negative integer | No; use tool argument |
| `TOKENFOLD_COMPRESSION_FORMAT` | CLI format name | No; use tool argument |
| `TOKENFOLD_COMPRESSION_TASK_SCOPE` | Task-scope name | No |
| `TOKENFOLD_COMPRESSION_DISABLED` | Comma-separated transform IDs | No |
| `TOKENFOLD_COMPRESSION_ENABLE` | Comma-separated transform IDs | No |
| `TOKENFOLD_COMPRESSION_EXPERIMENTAL` | Boolean | No |
| `TOKENFOLD_COMPRESSION_PRESERVE_LATEST_USER_MESSAGE` | Boolean | No |
| `TOKENFOLD_SAFETY_UNSAFE_DISABLE_REDACTION` | Boolean | No |
| `TOKENFOLD_OUTPUT_JSON` | Boolean | No |
| `TOKENFOLD_OUTPUT_NO_COLOR` | Boolean | No |
| `NO_COLOR` | Presence disables color | No |
| `TOKENFOLD_OUTPUT_QUIET` | Boolean | No |
| `TOKENFOLD_RETRIEVAL_STORE_ORIGINALS` | Boolean | No |
| `TOKENFOLD_RETRIEVAL_NAMESPACE` | Store namespace | No; use retrieve argument |
| `TOKENFOLD_RETRIEVAL_TTL_SECONDS` | Non-negative integer seconds | No |
| `TOKENFOLD_RETRIEVAL_MAX_STORE_BYTES` | Non-negative integer bytes; reserved for GC | No |
| `TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES` | Comma-separated namespaces; empty means unrestricted | Yes: retrieve and pruning |
| `TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES` | Non-negative integer bytes; unset means unbounded | No |
| `TOKENFOLD_RETRIEVAL_BACKEND` | `filesystem` or `memory` | Yes: retrieve; pruning requires filesystem |
| `TOKENFOLD_RETRIEVAL_STORE_PATH` | Filesystem store root | Yes: retrieve and pruning |
| `TOKENFOLD_ANALYTICS_ENABLED` | Boolean | No |
| `TOKENFOLD_ANALYTICS_LEDGER_DB` | Ledger path | Yes: stats |
| `TOKENFOLD_ANALYTICS_RETENTION_DAYS` | Non-negative integer days | No |
| `TOKENFOLD_ANALYTICS_HASH_PROJECT_PATHS` | Boolean | No |
| `TOKENFOLD_FILTERS_ENABLED` | Boolean | No |
| `TOKENFOLD_FILTERS_PROJECT_FILTERS` | Project filter-pack path | No |
| `TOKENFOLD_FILTERS_USER_FILTERS` | User filter-pack path | No |
| `TOKENFOLD_FILTERS_TRUST_STORE` | Trust-store path | No |
| `TOKENFOLD_TRUST_PROJECT_FILTERS` | Boolean CI trust override | No |

The npm wrapper additionally accepts `TOKENFOLD_BINARY_PATH`; the evaluation harness accepts
`TOKENFOLD_BIN`, `TOKENFOLD_PROXY_BIN` (the paired runner's observation arms) and
`TOKENFOLD_LEARNED_MODULE`; RTK integration accepts `TOKENFOLD_RTK_BIN` and
`TOKENFOLD_RTK_DISABLED`. These are surface-specific controls rather than config-file overrides.

The paired runner's live arm additionally reads `OPENROUTER_API_KEY` (override the
name with `--live-api-key-env`). It is never logged, written to a report, or forwarded
into the proxy's own configuration; the client sends it and the proxy passes
`Authorization` through unchanged.

## Bounded, authorized retrieval

Retrieval can be constrained on two independent axes. Both are **off by default**, so an
existing installation behaves exactly as before until they are set.

**Namespace authorization.** Setting `TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES`
(a comma-separated list, or `[retrieval].authorized_namespaces` in `tokenfold.toml`)
restricts which namespaces may be read. A request outside the set is refused with
`unauthorized` — HTTP `403` on the proxy, `status: "unauthorized"` over MCP, exit code `8`
from `tokenfold retrieve`. The refusal is identical whether or not the hash exists, so it
cannot be used to probe another namespace's contents.

**Restore budget.** Setting `TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES` (or
`[retrieval].max_restore_bytes`) caps how much one retrieval may restore. An entry larger
than the budget is refused with `over_budget` — HTTP `413` on the proxy, exit code `8` from
the CLI — reporting the whole-entry size against the limit. The entry is **never returned
truncated**: a partial JSON row presented as the original would silently corrupt the
recovered context, so the budget refuses rather than cuts.

Authorization is evaluated before the budget, so an unauthorized caller never learns an
entry's size.

### Leases and store upgrades

`RetrievalStore` metadata is versioned. An entry written before versioning existed carries no
lease field and cannot be shown to be unleased, so it is treated conservatively: it is never
size-evicted before its own TTL has actually elapsed, and it is removed normally once that TTL
passes. Upgrading therefore needs no migration step and no downtime — existing entries keep
working.

A **lease** is a durable promise that an entry stays retrievable until a given time, recorded
in the entry's own metadata. This is what lets a reference outlive the process that created
it: a second process running GC will not delete a leased entry, even one whose TTL has passed,
and a leased entry is still served on retrieval. Leases are released explicitly and
independently of their promised expiry, and re-acquiring one for the same holder never shortens
an existing promise.

**Stop/upgrade/resume procedure.** No action is required for existing stores. When enabling
lease-writing or quota-enforcing behavior on a shared store root:

1. Stop every process that writes to or runs GC on that root. A pre-EP-05 GC process does not
   understand leases and would delete a promised entry.
2. Upgrade all writers and GC callers to this build.
3. Resume. Entries written before the upgrade are read as version 0 and protected
   conservatively until their own TTL elapses.

A quota is a **rejection, not an eviction**: a write that would exceed it fails with
`TokenFoldError::QuotaExceeded` (exit code `5`) and nothing is written or deleted. Running GC
first, or raising the quota, is a deliberate operator decision.

`store_batch_within` checks admission and publishes under one backend lock, including across
processes. Duplicate keys in one batch count once; corrupt metadata refuses quota admission.
Re-storing identical content preserves other holders' leases and cannot shorten an existing
TTL. Entries with an unexpired finite TTL now survive size-pressure GC even without a lease;
releasing a lease does not cancel that TTL. GC can therefore remain over its requested cap,
reporting protected entries rather than breaking published references. New writes without a
finite TTL or lease remain size-evictable (legacy metadata remains conservatively protected).

`[retrieval].max_store_bytes` / `TOKENFOLD_RETRIEVAL_MAX_STORE_BYTES` now bounds
compression admission as well as GC. Python `compress(..., retrieval_max_store_bytes=...)` / `inspect(...)`
and the Rust builder expose the same limit. MCP reads the host environment only.
Rejected compression writes keep their rows inline; they do not emit unrecoverable
references or fail the entire compression. A full original receipt also consumes quota.
Preview cannot know durable occupancy and conservatively projects no drops when a quota
is configured. Upgrade all writers/GC together before relying on retention.

### Host-published MCP evidence search

`tokenfold_retrieve` also accepts `query`, `namespace` and `top_k` (default 5, maximum 20).
Search is opt-in: set `TOKENFOLD_RETRIEVAL_SEARCH_MANIFEST` to a host-owned JSON file:

```json
{"version":1,"namespaces":{"default":["<bare SHA-256 hash>"]}}
```

The caller cannot choose that path or enumerate the store. The host publishes only hashes
it authorizes for discovery. Existing namespace authorization also applies. The file is
limited to 64 KiB and 512 hashes per namespace; each indexable entry to 16 KiB; the index
to 1 MiB; the query to 4096 bytes. The index is rebuilt for each query and live entries are
rechecked for deletion/expiry. Duplicate hashes count once.

Responses contain whole entries (`hash`, `namespace`, `content`, `score`), never incomplete
rows. Total returned content is capped by `TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES` and an
absolute 64 KiB ceiling. Oversized hits are omitted with `over_budget`; no matches give
`no_match`. These are per-call content limits, not a cumulative session token budget.
Exact-hash retrieval remains available separately. Neither path re-executes a tool.


### Restart-safe observation commitments

`tokenfold-proxy --observations --observation-ledger-path <host-owned-file>` opts into
restart-safe observation idempotence. The default remains in-memory. The snapshot contains
only SHA-256 session/content fingerprints and expiry metadata, never raw session IDs or
observation text. The host must protect this file and its directory with service-account
permissions. One proxy owns an OS file lock for its lifetime; simultaneous owners refuse
startup rather than overwrite each other's commitments.

Snapshots are staged, synced and renamed, limited to 64 MiB, the configured session count
and 256 fingerprints per session. Expired commitments are not revived on restart; reduced
TTLs are applied to original commitment age. Corruption, incompatible versions or future
wall-clock metadata refuse startup. A failed write clears trust and stops new commitments
until restart. With a regressed wall clock, operators must not restore an older snapshot as
an authoritative current ledger. This does not promise provider cache hits or compact history.

### Explicit context selection (experimental)

```sh
tokenfold select context.json --experimental --query "find the failing parser" --target-tokens 512
```

The input is `{"prefix":"...","groups":[{"id":"g1","text":"...","required":true,
"fallback_score":1.0}],"suffix":"..."}`. IDs must be unique; groups are indivisible;
required text and prefix/suffix survive exactly. Without a scorer, declared fallback scores
are used. The assembled prompt is recounted, optional groups are trimmed whole if needed,
and empty/over-budget/no-token-gain selections keep the complete baseline. Token counts
carry local estimator provenance, not a provider chat-template guarantee. Secret-shaped
input is refused before output or scorer invocation. This is an independently gated lossy
selection API; it does not alter compression presets or the existing pruning algorithm.

`--scorer-config approved.json` permits a specifically approved direct-child executable:

```json
{"executable":"<absolute executable path>","executable_sha256":"<approved SHA-256>",
 "arguments":[],"model_revision":"<pinned revision>","approved_by":"<owner>",
 "scratch_root":"<absolute private existing directory>"}
```

The runtime receives paths through `TOKENFOLD_SELECT_REQUEST_PATH` and
`TOKENFOLD_SELECT_RESPONSE_PATH`. The request contains schema version 1, pinned revision,
query and ordered `{id,text}` groups. The response must contain only schema version 1,
matching `model_revision` and `scores: [["g1", 0.5], ...]`, exactly one finite score per ID.
Input is bounded; output is limited to 64 KiB. The executable hash is checked on each run;
no shell or inherited environment is used and stdout/stderr are discarded. Unknown IDs,
revision/schema mismatches, failure or timeout fall back to declared scores. The direct
child is killed and reaped on inference timeout (default 5 s, max 30 s) or library cancellation.
Malformed response diagnostics never echo model-provided values.

The approved runtime **must not launch descendants**; this is a direct-process deadline,
not an OS/network sandbox. Approvers also own auxiliary model/script files, command arguments,
private scratch permissions and resource limits. No model is installed or downloaded.
The receipt is separate version-1 `context_selection` JSON on stderr (or `--receipt-file`),
with no raw context/query/group IDs; selected text goes to stdout (or `--output`). An
unreachable budget is visible in `budget_met: false`, never disguised by empty context.
