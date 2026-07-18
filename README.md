<div align="center">

# TOKENFOLD

**Send less noise. Fit more context. Pay for fewer input tokens.**

Local, provider-neutral compression for prompts, tool schemas, JSON, logs, and diffs.

[![CI](https://img.shields.io/github/actions/workflow/status/snchimata/tokenfold/ci.yml?branch=main&label=tests&logo=github&style=flat-square)](https://github.com/snchimata/tokenfold/actions/workflows/ci.yml) [![Coverage](https://img.shields.io/github/actions/workflow/status/snchimata/tokenfold/ci.yml?branch=main&label=coverage&logo=github&style=flat-square)](https://github.com/snchimata/tokenfold/actions/workflows/ci.yml) [![GitHub Release](https://img.shields.io/github/v/release/snchimata/tokenfold?logo=github&style=flat-square)](https://github.com/snchimata/tokenfold/releases/latest) [![PyPI](https://img.shields.io/pypi/v/tokenfold?label=PyPI&style=flat-square)](https://pypi.org/project/tokenfold/) [![npm](https://img.shields.io/npm/v/tokenfold?label=npm&logo=npm&style=flat-square)](https://www.npmjs.com/package/tokenfold) [![Rust](https://img.shields.io/crates/v/tokenfold-core?label=Rust&style=flat-square)](https://docs.rs/crate/tokenfold-core/latest) [![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](LICENSE)

[Quick start](#quick-start) · [Why tokenfold](#why-tokenfold) · [Integrations](#pick-your-integration) · [Benchmarks](#reproduce-the-numbers)

</div>

---

## Proven compression, not projections

| Repetitive JSON | API responses | Tool schemas |
| :---: | :---: | :---: |
| **67.6% fewer tokens** | **61.3% fewer tokens** | **45.63% fewer tokens** |
| 50-record payload | 30-record payload | 1.8 MB OpenAI-style fixture |

All three results use exact `o200k_base` counts in balanced mode. The
JSON-data results are lossless. The schema benchmark preserves required
fields and descriptions while trimming redundant examples. Repetitive,
structured data benefits most.

## Quick start

Install the interface that fits your stack:

```bash
pip install tokenfold       # Python 3.9+
npm install tokenfold       # Node.js 22+
cargo add tokenfold-core    # Rust library
cargo install tokenfold-cli # Rust CLI
```

Or download the CLI for Linux, macOS, or Windows from
[GitHub Releases](https://github.com/snchimata/tokenfold/releases/latest),
then verify it with the adjacent `.sha256` file.

Or install the Python package:

```bash
pip install tokenfold
```

Compress an OpenAI-style request before sending it to your provider:

```python
import json
from pathlib import Path

from tokenfold import CompressionMode, compress_openai_payload

result = compress_openai_payload(
    Path("request.json").read_text(),
    mode=CompressionMode.BALANCED,
)
compressed_request = json.loads(result.payload)

print(f"saved {result.report.saved_tokens} tokens ({result.saved_pct():.1f}%)")
# Pass compressed_request to your existing OpenAI client.
```

The TypeScript package calls the same local Rust engine and returns bytes plus
the canonical compression receipt:

```typescript
import { compress } from "tokenfold";

const input = new TextEncoder().encode(JSON.stringify({
  results: [
    { id: 101, region: "us-east-1", plan: "pro" },
    { id: 102, region: "us-east-1", plan: "pro" },
    { id: 103, region: "us-east-1", plan: "pro" },
  ],
}, null, 2));

const { payload, report } = await compress(input, {
  format: "json",
  mode: "balanced",
});

console.log(`saved ${report.saved_tokens} tokens`);
console.log(new TextDecoder().decode(payload));
```

Want to try the CLI from source? Inspect the bundled request without changing it:

```bash
git clone https://github.com/snchimata/tokenfold.git
cd tokenfold
cargo run --release --locked -p tokenfold-cli -- \
  inspect examples/openai_payload.json --format openai
```

Across 100 requests with the same payload shape, those savings add up to:

```text
json_minify          34,600 → 22,900   saved 11,700
schema_compaction    22,900 → 21,300   saved  1,600
TOTAL                34,600 → 21,300   saved 13,300 (38.4% reduction, estimated)
```

## Why tokenfold

Models do not need the same object key hundreds of times. Providers still
count every token. Tokenfold removes that structural waste before the model
call, so you get:

- **Lower input cost** — send fewer billable tokens without changing providers.
- **More useful context** — reclaim room for instructions, evidence, and
  conversation history.
- **Less data movement** — shrink payloads crossing queues, proxies, logs,
  and evaluation runs.
- **Fewer blind spots** — inspect counts, transforms, and warnings.
- **No new data processor** — run locally, in-process, or behind your own
  loopback proxy.

```text
messages · schemas · JSON · logs · diffs
                    │
                    ▼
                tokenfold ──────▶ any LLM provider
                    │
                    └───────────▶ compressed payload + receipt
```

## What it improves

| Workload | User benefit |
| --- | --- |
| APIs and record sets | Store repeated keys and values once |
| Provider requests | Shrink messages and schemas without changing API shape |
| Agent logs and diffs | Keep evidence; collapse repetitive output |
| Token budgets | Meet the target or return an honest best effort |
| Sensitive workflows | Redact detected secrets before reports or storage |

## Pick your integration

One Rust engine powers every surface, so policies and receipts stay
consistent as your stack changes.

| Surface | Best for | Install or run |
| --- | --- | --- |
| Python | Applications and evaluation pipelines | `pip install tokenfold` |
| TypeScript | Node.js applications and automation | `npm install tokenfold` |
| Rust | Native embedding | `cargo add tokenfold-core` |
| CLI | Files and command output | [Download a release binary](https://github.com/snchimata/tokenfold/releases/latest) |
| HTTP proxy | Provider-shaped traffic | Build `tokenfold-proxy` from source |
| MCP server | MCP-compatible agents and editors | `tokenfold mcp serve` |

### Compress generic JSON

Use `format="JSON"` for API responses, record dumps, and other data that is
not an LLM request:

```python
import json
import tokenfold

result = tokenfold.compress(
    json.dumps({
        "results": [
            {"id": 101, "region": "us-east-1", "plan": "pro"},
            {"id": 102, "region": "us-east-1", "plan": "pro"},
            {"id": 103, "region": "us-east-1", "plan": "pro"},
        ]
    }),
    format="JSON",
    mode="BALANCED",
)

print(f"saved {result.report.saved_tokens} tokens")
```

### Run the proxy

```bash
cargo build --release --locked -p tokenfold-proxy
target/release/tokenfold-proxy \
  --upstream https://api.openai.com \
  --target-tokens 12000
```

The proxy listens on `127.0.0.1:8787` by default, streams SSE responses, and
returns the compression receipt in `X-TokenFold-*` headers.

## Safety you can inspect

Tokenfold recounts after every stage and stops when the target is met or the
allowed transform set is exhausted.

- **Never larger:** a transform stays only when it reduces the token count.
- **Reversible JSON:** every structural rewrite must pass an exact round trip.
- **Clear provenance:** exact tokenizer results and estimates are labeled separately.
- **Actionable receipts:** every result lists savings, transforms, warnings,
  and final status.
- **Honest limits:** unreachable targets return an explicit status instead of
  silently deleting more content.

Lossy log and diff transforms remain policy-gated. Optional originals can be
stored by SHA-256 hash; detected secret-shaped content is excluded.

## Reproduce the numbers

| Fixture | Exact token reduction | Source |
| --- | ---: | --- |
| Repetitive 50-record JSON | **67.6%** | [Changelog] |
| 30-record API response | **61.3%** | [Changelog] |
| 1.8 MB OpenAI tool schema | **45.63%** | [Thresholds] |

[changelog]: CHANGELOG.md
[thresholds]: crates/tokenfold-core/benches/THRESHOLDS.toml

Run the regression benchmark:

```bash
cargo bench -p tokenfold-core
```

Or inspect the small bundled JSON sample:

```bash
cargo run --release --locked -p tokenfold-cli -- \
  inspect examples/api_response.json --format json
```

The sample reports 382 → 206 estimated tokens, a **46.1% reduction**. Ragged
or compact inputs may save little; Tokenfold reports that result honestly.

## Contributing

Issues and pull requests are welcome. Run the core checks before opening a PR:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python eval/run_fidelity.py --gate --profile smoke-first-consumer
cd packages/tokenfold && npm ci && npm test
```

## License

[Apache-2.0](LICENSE)

## Reclaim your context window

Start with one representative payload. Install `tokenfold`, inspect the
receipt, and see how many tokens your application can stop sending today.

```bash
pip install tokenfold
```
