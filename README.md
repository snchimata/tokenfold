<div align="center">

# tokenfold

**Fold the noise. Keep the signal.**

The local compression layer for LLM apps and agents. Shrink messages, tool schemas,
logs, and diffs before inference—with exact token counts and a safety receipt for every change.

[![CI](https://github.com/snchimata/tokenfold/actions/workflows/ci.yml/badge.svg)](https://github.com/snchimata/tokenfold/actions/workflows/ci.yml)
[![Python 3.9+](https://img.shields.io/badge/python-3.9%2B-3776AB?logo=python&logoColor=white)](crates/tokenfold-py/pyproject.toml)
[![Rust 1.97](https://img.shields.io/badge/rust-1.97-000000?logo=rust&logoColor=white)](rust-toolchain.toml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](Cargo.toml)

[Quick start](#quick-start) · [Why tokenfold](#why-tokenfold) · [Integrations](#one-engine-five-ways-to-use-it) · [Safety](#safe-by-design) · [Benchmark](#measured-not-marketed)

</div>

> **45.63% fewer tokens** on the checked-in structured-JSON benchmark, measured with the
> exact tokenizer. Reproduce it with `cargo bench -p tokenfold-core`.

LLM systems routinely spend context on formatting, repeated logs, verbose schemas, and
low-signal command output. That waste costs money and crowds useful information out of the
context window.

tokenfold sits between your application and any model provider:

```text
app or agent  ──  messages · schemas · logs · diffs  ──▶  tokenfold  ──▶  any LLM
                                                            │
                                                            └── payload + receipt
```

It removes what the model does not need, protects secret-shaped values first, and reports
exactly what changed. Preview every transformation before applying it.

## Quick start

The library and Python package are published — `cargo add tokenfold-core` and
`pip install tokenfold`. To use the CLI, build it from source with the pinned Rust toolchain:

```bash
git clone https://github.com/snchimata/tokenfold.git
cd tokenfold
cargo build --release --locked -p tokenfold-cli
```

Preview savings on the bundled OpenAI payload without changing it:

```bash
target/release/tokenfold inspect examples/openai_payload.json --format openai
```

The abridged receipt:

```text
TRANSFORM              MODE          EST TOKENS BEFORE→AFTER      SAVED       %  STATUS
secret_redaction       all                           346→346          0    0.0%  no_op
json_minify            all                           346→229        117   33.8%  applied
schema_compaction      all                           229→213         16    7.0%  applied
TOTAL  346 → 213 tokens   saved 133 (38.4% reduction)
```

Compress to a budget. The payload goes to stdout and the receipt goes to stderr, so the
result is safe to pipe:

```bash
target/release/tokenfold compress examples/openai_payload.json \
  --format openai \
  --target-tokens 250 > compressed.json
```

Compress generic JSON **data** (API responses, records, logs — not just message payloads).
tokenfold folds arrays of same-shape objects into columnar form and dictionaries repeated
values, all losslessly:

```bash
target/release/tokenfold inspect examples/api_response.json --format json
# larger, more repetitive data compresses further — a 50-record blob folds 0% → ~68%
```

Or trim noisy output from any command:

```bash
target/release/tokenfold wrap -- git diff
```

## Why tokenfold

- **Spend context on the task.** Remove formatting, repetition, and low-signal content
  before it consumes the model's attention or your budget.
- **Know what you saved.** Exact `o200k_base` and `cl100k_base` counts drive budget decisions;
  heuristics are never presented as exact.
- **Keep control.** Every call returns a typed `CompressionReport` with before/after counts,
  per-transform savings, estimator provenance, warnings, and final status.
- **Fail honestly.** If the safe transform set cannot meet a target, tokenfold reports
  `BEST_EFFORT` or `UNREACHABLE_TARGET` instead of pretending it succeeded.
- **Stay provider-neutral.** Use the same Rust core from the CLI, Python, an HTTP proxy,
  an MCP server, or your own Rust application.
- **Run locally.** The CLI is a native binary with no service account, hosted control plane,
  or runtime dependency.

## One engine, five ways to use it

| Surface | Entry point | Best for |
| --- | --- | --- |
| CLI | `tokenfold` | Files, stdin, command output, and local agent workflows |
| Python | `import tokenfold` | Python applications and evaluation pipelines |
| HTTP proxy | `tokenfold-proxy` | OpenAI- or Anthropic-shaped traffic in flight |
| MCP server | `tokenfold mcp serve` | MCP-compatible agents and editors |
| Rust library | `tokenfold-core` | Native embedding with typed policies and reports |

### Python

The Python package is a native `pyo3` binding for CPython 3.9+:

```bash
pip install tokenfold
```

A Windows wheel is published; on Linux/macOS pip builds the sdist from source (needs a Rust
toolchain). To build from a checkout instead, use [maturin](https://www.maturin.rs/):

```bash
python -m venv .venv
source .venv/bin/activate  # PowerShell: .venv\Scripts\Activate.ps1
python -m pip install maturin
maturin develop --release -m crates/tokenfold-py/Cargo.toml
```

```python
from pathlib import Path
from tokenfold import CompressionMode, compress_openai_payload

payload = Path("examples/openai_payload.json").read_text()
result = compress_openai_payload(
    payload,
    target_tokens=250,
    mode=CompressionMode.BALANCED,
)

print(result.payload.decode())
print(f"saved {result.report.saved_tokens} tokens ({result.saved_pct():.1f}%)")
```

Also available: `compress`, `inspect`, `compress_anthropic_payload`, and
`compress_messages`. Errors use a typed hierarchy rooted at `TokenFoldError`.

### HTTP proxy

```bash
cargo build --release --locked -p tokenfold-proxy
target/release/tokenfold-proxy \
  --upstream https://api.openai.com \
  --target-tokens 12000
```

The proxy binds to `127.0.0.1:8787` by default, requires an HTTPS upstream, streams SSE
without buffering, and returns `X-TokenFold-*` receipt headers.

### MCP server

```bash
target/release/tokenfold mcp serve
```

The stdio server exposes `tokenfold_compress`, `tokenfold_inspect`, `tokenfold_retrieve`,
and `tokenfold_stats` over JSON-RPC 2.0.

## Safe by design

tokenfold applies ordered transforms, recounts after every stage, and stops when the target
is met or the safe transform set is exhausted.

```text
input  ──▶  redact secrets  ──▶  compact  ──▶  recount  ──▶  payload + report
```

| Stage | Availability | What it does |
| --- | --- | --- |
| `secret_redaction` | Always on | Redacts secret-shaped values before reporting, logging, or persistence |
| `json_minify` | Default | Removes insignificant JSON whitespace |
| `schema_compaction` | Default | Shortens examples while preserving descriptions and required fields |
| `json_field_fold` | Balanced / aggressive | Folds arrays of same-shape objects into columnar `{cols, rows}` — each key once, not once per row (generic JSON data) |
| `json_value_dict` | Balanced / aggressive | Replaces repeated large values with dictionary references (generic JSON data) |
| `log_compaction` | Balanced / aggressive | Collapses repeated, low-signal log lines |
| `diff_compaction` | Experimental | Reduces low-signal diff context |

`json_field_fold` and `json_value_dict` are **lossless and reversible**: each is applied only
if it passes an exact round-trip check (`unfold(fold(x)) == x`) *and* lowers the exact token
count, so a payload can never come out larger or altered. Lossy transforms must clear a
downstream fidelity gate before promotion. Originals can also be stored by SHA-256 hash when
reversibility is requested; secret-shaped content is never persisted.

## Measured, not marketed

The structured-JSON benchmark measures **45.63% exact token savings** in balanced mode on a
deterministic, pretty-printed OpenAI tool-schema fixture. On generic JSON **data**, the v0.2
columnar fold + value dictionary reach **~61–68% exact savings** on repetitive records
(30–50 rows), losslessly. All numbers are quoted against exact `o200k_base` counts and are
reproducible benchmarks, not a promise for every workload — ragged or already-compact JSON
correctly reports single-digit or no savings rather than pretending.

```bash
cargo bench -p tokenfold-core
```

The tokenizer, fixture, hardware record, and regression thresholds are documented in
[`crates/tokenfold-core/benches/THRESHOLDS.toml`](crates/tokenfold-core/benches/THRESHOLDS.toml)
and [`CHANGELOG.md`](CHANGELOG.md).

<details>
<summary><strong>CLI command map</strong></summary>

| Command | Purpose |
| --- | --- |
| `inspect` | Preview achievable savings |
| `compress` | Compress a file or stdin |
| `diff` | Compare raw and compressed payloads |
| `wrap` (`shell`) | Run a command and compress its output |
| `benchmark` | Measure fixtures with before/after token counts |
| `init` / `uninit` | Install or remove an agent-host integration |
| `doctor` | Check estimator, config, and integration health |
| `mcp serve` | Start the MCP server over stdio |
| `retrieve` | Restore an original saved by hash |
| `stats` / `gain` | Report local token savings |
| `session` | Report command-wrapping coverage |
| `filters` | Manage command-output filters |
| `output-savings` | Report measured or estimated output-token savings |
| `learn` (`discover`) | Propose policy changes from local history |

Run `tokenfold <command> --help` for flags and examples.

</details>

## Status

tokenfold is source-only `0.1.0` software. Prebuilt binaries, crates.io packages, and PyPI
wheels have not been published yet. The CLI, Python binding, proxy, MCP server, and optional
extension crates are implemented and tested in this repository.

## Contributing

Issues and pull requests are welcome. Run the same core checks as CI before opening a PR:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python eval/run_fidelity.py --gate --profile smoke-first-consumer
```

Security and dependency checks:

```bash
cargo audit
cargo deny check advisories bans licenses sources
```

## License

[Apache-2.0](Cargo.toml)
