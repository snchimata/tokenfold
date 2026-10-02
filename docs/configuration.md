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
| `TOKENFOLD_RETRIEVAL_AUTHORIZED_NAMESPACES` | Comma-separated namespaces; empty means unrestricted | No |
| `TOKENFOLD_RETRIEVAL_MAX_RESTORE_BYTES` | Non-negative integer bytes; unset means unbounded | No |
| `TOKENFOLD_RETRIEVAL_BACKEND` | `filesystem` or `memory` | Yes: retrieve |
| `TOKENFOLD_RETRIEVAL_STORE_PATH` | Filesystem store root | Yes: retrieve |
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
