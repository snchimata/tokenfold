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
`TOKENFOLD_BIN` and `TOKENFOLD_LEARNED_MODULE`; RTK integration accepts `TOKENFOLD_RTK_BIN` and
`TOKENFOLD_RTK_DISABLED`. These are surface-specific controls rather than config-file overrides.
