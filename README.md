<div align="center">

# TOKENFOLD

**Send less noise. Fit more context. Pay for fewer input tokens.**

**Up to 92% fewer input tokens · lossless by default · exact receipts**

CLI · Python · TypeScript · Rust · proxy · MCP · local-first · provider-neutral

[![CI](https://img.shields.io/github/actions/workflow/status/snchimata/tokenfold/ci.yml?branch=main&label=tests&logo=github&style=flat-square)](https://github.com/snchimata/tokenfold/actions/workflows/ci.yml) [![Coverage](https://img.shields.io/github/actions/workflow/status/snchimata/tokenfold/ci.yml?branch=main&label=coverage&logo=github&style=flat-square)](https://github.com/snchimata/tokenfold/actions/workflows/ci.yml) [![GitHub Release](https://img.shields.io/github/v/release/snchimata/tokenfold?logo=github&style=flat-square)](https://github.com/snchimata/tokenfold/releases/latest) [![PyPI](https://img.shields.io/pypi/v/tokenfold?label=PyPI&style=flat-square)](https://pypi.org/project/tokenfold/) [![npm](https://img.shields.io/npm/v/tokenfold?label=npm&logo=npm&style=flat-square)](https://www.npmjs.com/package/tokenfold) [![Rust](https://img.shields.io/crates/v/tokenfold-core?label=Rust&style=flat-square)](https://docs.rs/crate/tokenfold-core/latest) [![License](https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square)](https://github.com/snchimata/tokenfold/blob/main/LICENSE)

[Measured results](#measured-results) · [Platform](#one-platform-two-engines) · [Quick start](#quick-start) · [Lossy pruning](#recoverable-lossy-pruning) · [Select model](#tokenfold-select) · [Reproduce](#reproduce-the-results)

</div>

---

## Measured results

### Compression

| Search / RAG results | Repetitive JSON | API responses | Tool schemas |
| :---: | :---: | :---: | :---: |
| **91.7% fewer tokens** | **67.6% fewer tokens** | **63.9% fewer tokens** | **45.6% fewer tokens** |
| 84,032 → 6,951 | 50-record payload | 30-record payload | 1.8 MB OpenAI fixture |
| opt-in, recoverable | lossless | lossless | lossless |

All four use exact `o200k_base` counts in balanced mode. The 91.7% showcase
keeps the planted `503` result inline and makes the other 98 long rows
retrievable by hash. Run it with `python examples/lossy_pruning.py`.

### Fine-tuned relevance ranking

[Tokenfold Select][tokenfold-select] is the optional query-aware model for
choosing what fills a tight context window after structural compression ends.

| Token budget kept | Tokenfold Select task success | BM25 reference | Lift |
| ---: | ---: | ---: | ---: |
| 50% | **86.3%** | 79.4% | **+8.7%** |
| 25% | **70.3%** | 60.7% | **+15.8%** |
| 10% | **39.9%** | 37.7% | **+5.8%** |

At a 25% budget, the fine-tuned ranker keeps the answer **70% of the time** and
beats the strongest free heuristic by **15.8%**. Critical-content survival is
**100% at every measured budget** through allocator force-keep. Results are
three-seed repeated subsampling over roughly 73,000 training and 24,000
held-out fixtures per run; the [model card][tokenfold-select] publishes the
training recipe and full baseline set. Here, task success means the literal
gold answer survived selection under the stated budget.

## One platform, two engines

| Capability | What you get |
| --- | --- |
| **Tokenfold Core** | Fast deterministic compression for JSON, schemas, provider requests, logs, diffs, and command output |
| **Recoverable pruning** | Drop low-signal JSON rows only after storing them locally; fetch any omission with `tokenfold retrieve` |
| **Tokenfold Select** | LoRA-fine-tuned, query-aware span ranking with up to **15.8% lift** over BM25 at the same budget |
| **Budget control** | Conservative, balanced, and aggressive modes; nine task scopes; `--target-tokens` with honest best-effort reporting |
| **Exact receipts** | Before/after token counts, applied transforms, warnings, provenance, and final status on every call |
| **Realized savings** | `gain`, `stats`, and `session` report measured savings; `learn` proposes policy improvements without silently applying them |
| **Every integration** | CLI, Python, TypeScript, Rust, HTTP proxy, and MCP share the same Core engine and report shape |
| **Local-first safety** | Secret redaction, protected-content gates, reversible structural transforms, and no hosted data processor |

### Lossless or recoverable lossy

| | Lossless — default | Recoverable lossy — opt-in |
| --- | --- | --- |
| **What it does** | Minifies, folds repeated keys into columns, and stores repeated values once | Adds deterministic ranking and retrieval markers for selected array rows |
| **Best on** | Uniform records, schemas, logs, diffs | Search results, mixed event feeds, agent traces, long arrays |
| **Measured here** | **45.6–67.6%** fewer tokens | **51.7–91.7%** fewer tokens |
| **Recovery** | All data remains in the payload | Every emitted marker resolves through the local retrieval store |

## Quick start

Install the surface that fits your stack:

```bash
pip install tokenfold       # Python 3.9+
npm install tokenfold       # Node.js 22+
cargo add tokenfold-core    # Rust library
cargo install tokenfold-cli # Rust CLI
```

Or download a signed CLI build for Linux, macOS, or Windows from
[GitHub Releases](https://github.com/snchimata/tokenfold/releases/latest) and
verify it with the adjacent `.sha256` file.

Preview first, then compress:

```bash
tokenfold inspect payload.json --format json
tokenfold compress payload.json --format json --output payload.compact.json
```

Python uses the same Core engine and typed receipt:

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
```

| Surface | Best for | Start here |
| --- | --- | --- |
| CLI | Files, stdin, diffs, and command output | `tokenfold compress`, `inspect`, `diff`, `wrap` |
| Python | Applications and evaluation pipelines | `pip install tokenfold` |
| TypeScript | Node.js applications and automation | `npm install tokenfold` |
| Rust | Native embedding | `cargo add tokenfold-core` |
| HTTP proxy | Transparent provider-shaped traffic | Build `tokenfold-proxy` |
| MCP | Agents and editors | `tokenfold mcp serve` |

`tokenfold init --agent <agent>` installs a durable host integration;
`tokenfold doctor` verifies it. Trusted filters for Git, build, and test output
are available through `tokenfold filters list`.

Runnable examples, one per surface, all under [`examples/`](examples):

```bash
python examples/quickstart.py      # Python: compress messages, request bodies, JSON data
node examples/quickstart.mjs       # TypeScript/Node: compress, inspect, read the receipt
cargo run -p tokenfold-core --example quickstart   # Rust: the embedded core API
python examples/lossy_pruning.py   # CLI: opt-in recoverable pruning, end to end
```

[`examples/quickstart.ipynb`](examples/quickstart.ipynb) is the guided tour of the whole
Python surface: everything `quickstart.py` shows, plus previews, budgets and modes,
provider payloads, recoverable pruning with retrieval, and where Tokenfold Select fits.

## Recoverable lossy pruning

Lossless folding has a ceiling on heterogeneous arrays. `--lossy` ranks rows,
keeps the strongest signals, and replaces selected rows with compact
`{"$tf_ref": {...}}` handles. A row leaves the payload only after the local
store accepts it.

```bash
tokenfold compress examples/incident_feed.json --format json \
  --lossy heuristic --lossy-ratio 0.35 --output feed.compact.json
```

On the bundled 40-event feed:

| Mode | Exact tokens | Reduction | Events kept | Incident kept |
| --- | ---: | ---: | ---: | :---: |
| Lossless | 6,144 | 14.9% | 40 | ✅ |
| `--lossy-ratio 0.35` | 3,485 | **51.7%** | 13 | ✅ |
| `--lossy-ratio 0.05` | 2,294 | **68.2%** | 1 | ✅ |

The planted `503` with `success: false` and `retries: 7` survives every shown
setting because typed failure signals outrank position and length — at
`--lossy-ratio 0.05` it is the only row kept. Long rows
amortize marker overhead further: the 100-result showcase reaches **91.7%**.

Fetch a dropped row:

```bash
tokenfold retrieve cb13cc59cca0c218c579cd1d4b3cbab58d6dea265eb995cc9c00faf0cd0a6856
# {"seq":1,"ts":"2026-08-15T00:01:11Z","subsystem":"index-writer",...}
```

What the flags mean:

- `--lossy-ratio` is an aggression hint over eligible array items. Lower keeps
  fewer rows; it is not a whole-document guarantee.
- `--target-tokens` is the whole-document goal. Tokenfold stops when it reaches
  the target losslessly and reports `best_effort` when the safe transform set
  cannot reach it (`unreachable_target` when protected content alone exceeds
  the target).
- `--lossy-preserve <path>` protects a named array; nested paths conservatively
  protect their nearest eligible ancestor.
- Generic JSON only: lossy pruning does not run on OpenAI or Anthropic message
  payloads.
- Storage is fail-closed: refused rows stay inline, and detected secret-shaped
  bytes are never persisted.

Preview the projected savings with no store writes:

```bash
tokenfold compress examples/incident_feed.json --format json \
  --lossy heuristic --lossy-ratio 0.35 --dry-run
```

The same flags, and the same fail-closed contract, from Python and TypeScript:

```python
from tokenfold import CompressionPolicy, InputFormat, LossyPath, compress, retrieve

policy = CompressionPolicy(lossy=LossyPath.HEURISTIC, lossy_ratio=0.35)
result = compress(feed_bytes, format=InputFormat.JSON, policy=policy)
original = retrieve(marker["$tf_ref"]["hash"])   # any dropped row, verbatim
```

```ts
import { compress, retrieve } from "tokenfold";

const { payload, report } = await compress(feed, {
  format: "json",
  lossy: "heuristic",
  lossyRatio: 0.35,
});
const original = await retrieve(hash); // any dropped row, verbatim
```

<details>
<summary><strong>Current Phase 1 constraints</strong></summary>

Treat `$tf_ref` as reserved and do not enable lossy pruning on documents that
already contain retrieval markers. A filesystem failure after partial writes
may also leave unreferenced entries until their configured TTL expires. Both
require location-based transactional materialization before promotion.

Preview is a projection rather than a filesystem transaction, so a real run may
keep more rows if storage becomes unavailable.

</details>

## Tokenfold Select

**When structure ends, rank what matters.**

Tokenfold Select is an Apache-2.0 LoRA adapter on
`ibm-granite/granite-embedding-reranker-english-r2`. It scores candidate spans
against a query; your allocator applies the token budget and force-keeps
required content. Core remains model-free and deterministic, while Select adds
relevance when lexical heuristics stop being enough.

| | Tokenfold Core | Tokenfold Select |
| --- | --- | --- |
| Best at | Structural compression | Query-conditioned span ranking |
| Runtime | Static Rust binary | Granite reranker + LoRA adapter |
| Output | Compressed payload + exact receipt | Ranking logits |

<details>
<summary><strong>Python: load the model and score spans</strong></summary>

```python
from pathlib import Path

import torch
from huggingface_hub import snapshot_download
from peft import PeftModel
from transformers import AutoModelForSequenceClassification, AutoTokenizer

base_id = "ibm-granite/granite-embedding-reranker-english-r2"
repo_dir = Path(snapshot_download("snchimata/tokenfold-select"))
adapter_dir = repo_dir / "adapter"
tokenizer = AutoTokenizer.from_pretrained(adapter_dir)
base = AutoModelForSequenceClassification.from_pretrained(
    base_id,
    dtype=torch.float32,
)
model = PeftModel.from_pretrained(base, adapter_dir).eval()

def score(query: str, spans: list[str]) -> list[float]:
    if not spans:
        return []
    encoded = tokenizer(
        [query] * len(spans),
        spans,
        padding=True,
        truncation=True,
        max_length=8192,
        return_tensors="pt",
    )
    with torch.no_grad():
        output = model(
            input_ids=encoded["input_ids"],
            attention_mask=encoded["attention_mask"],
        )
    return output.logits.view(-1).float().tolist()
```

</details>

See the [Tokenfold Select model card][tokenfold-select] for setup, evaluation,
training data, and limitations.

## Safety and auditability

- **Never larger:** Core keeps a transform only when exact recounting shows a
  reduction; a lossy branch must also beat the lossless result.
- **Reversible structure:** JSON folds must pass an exact round trip.
- **Protected content:** provider system messages and latest-user content are
  held behind format-aware safety gates.
- **Clear provenance:** exact tokenizer counts, heuristics, and extrapolations
  are labeled separately.
- **Actionable receipts:** every result lists savings, transforms, warnings,
  retrieval state, and final status.
- **Local control:** detected secrets are redacted before reports or storage;
  policy learning changes configuration only with `--apply`.

## Reproduce the results

```bash
# 91.7% maximum-compression showcase, mixed-feed curve, retrieval, and preserve
python examples/lossy_pruning.py

# Lossless transform benchmarks
cargo bench -p tokenfold-core

# Small exact-token CLI example
cargo run --release --locked -p tokenfold-cli -- \
  inspect examples/api_response.json --format json
```

The bundled 30-record API response reports 3,812 → 1,376 tokens, a **63.9%
lossless reduction**. Benchmark sources and thresholds live in [CHANGELOG.md](https://github.com/snchimata/tokenfold/blob/main/CHANGELOG.md)
and [`crates/tokenfold-core/benches/THRESHOLDS.toml`](https://github.com/snchimata/tokenfold/blob/main/crates/tokenfold-core/benches/THRESHOLDS.toml).

## Contributing

Issues and pull requests are welcome. Run the relevant checks before opening a
PR:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python eval/run_fidelity.py --gate --profile smoke-first-consumer
cd packages/tokenfold && npm ci && npm test
```

## License

[Apache-2.0](https://github.com/snchimata/tokenfold/blob/main/LICENSE)

---

Start with one representative payload, inspect the receipt, and see how many
tokens your application can stop sending today.

```bash
pip install tokenfold
```

If Tokenfold earns a place in your stack, a ⭐ on
[GitHub](https://github.com/snchimata/tokenfold) helps the next team find it.

[tokenfold-select]: https://huggingface.co/snchimata/tokenfold-select
