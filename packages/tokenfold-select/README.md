# Tokenfold Select v2

**Smaller contexts. Original words. Local ONNX scoring.**

The optional `tokenfold-select` package defaults to the **Select v2 FP32 ONNX CPU
scorer**. It ranks whole caller-declared groups jointly in one bounded encoder
window; Tokenfold's existing allocator keeps required content, original order
and the final token-budget check. It never generates replacement text.

This is an **experimental preview**, not a quality- or performance-qualified
production model. Core compression stays model-free and deterministic. Installing
this package does not enable selection, download weights, send data to a service,
or change the `tokenfold` Python/TypeScript compression APIs.

## Install and inspect

```sh
python -m pip install 'tokenfold-select[onnx]==0.5.2'
python -m tokenfold_select inspect-onnx --artifact /absolute/select-v2-model
```

Provision the separate ONNX model directory before use. It must contain
`model.onnx`, `model.onnx.data`, the pinned tokenizer, runtime reference files and
`onnx-manifest.json`. The default manifest hash is
`267313718e80aa91a56923dea30363fd5c24fde3782c716d6d048891860a2054`;
model revision is the source checkpoint hash
`4dbdb5071e7d6316d8eb70dc161b1f2f77f555012f6616d7fa34311188e24cec`.
The package verifies every computational file before inference. Do not substitute
an untrusted manifest pin. Artifact inspection requires no inference dependencies.

## Use with the CLI

Create an existing, private scratch directory. Explicitly approve your native
Python executable, installed package/dependency directory and model files:

```sh
python -m tokenfold_select configure \
  --artifact /absolute/select-v2-model \
  --site-packages /absolute/venv/lib/python3.12/site-packages \
  --scratch-root /absolute/private-scratch \
  --approved-by your-name > select-scorer.json

tokenfold --experimental select context.json --query 'find the relevant item' \
  --target-tokens 256 --scorer-config select-scorer.json \
  --inference-timeout-ms 30000
```

Use actual absolute paths for your platform (Windows uses `Lib/site-packages`).
`configure` checks model-file integrity and emits the existing approved-scorer
JSON; it performs no inference. Inspect the file before use. The command pins the
**native base interpreter**, not a Windows venv shim; the isolated bootstrap loads
this installed package and the explicitly provisioned site-packages, without
`.pth` processing. Protect auxiliary code, libraries, model files and scratch
storage. This is not an OS sandbox.

A minimal context document is:

```json
{"prefix":"","suffix":"","groups":[{"id":"required","text":"Keep this.\n","required":true},{"id":"optional","text":"Other context.\n","fallback_score":0.1}]}
```

The worker defaults to `--backend onnx`, explicitly selects
`CPUExecutionProvider`, disables provider fallback and hides GPUs before loading
inference libraries. It uses bounded file IPC, no network or descendants, and an
owned direct-child deadline. A receipt with `used_scorer: false` means fallback,
not successful model-backed selection. Missing approval retains the existing
caller-declared deterministic fallback; there is no implicit model execution.

## Resident API

```python
from tokenfold_select import OnnxScorer

scorer = OnnxScorer('/absolute/select-v2-model', development=True)
request = {
    'schema_version': 1,
    'model_revision': scorer.revision,
    'query': 'find the relevant item',
    'groups': [{'id': 'item', 'text': 'Original context.'}],
}
response = scorer.score(request)  # finite, id-aligned scores only
```

Use a dedicated process with caller-owned deadlines for the resident API. The
CLI owns and waits for its one-shot worker; resident callers own their process.
Neither API owns grouping, protected-content policy or final allocation.

## Bounds and evidence

One complete window: at most 8,192 tokens, 512 groups, a 1 MiB request and a
64 KiB response. Overflow is refused, not truncated or silently windowed. ONNX
Runtime 1.30.0, NumPy 2.5.3 and tokenizers 0.23.2 are pinned; no PyTorch inference
dependency is required for this backend. No runtime binaries or weights are bundled.

The exported graph passed six synthetic CPU parity cases against the source
model, including dynamic lengths through 2,048 tokens and 512 pooled ranges.
This is mechanics evidence, **not** full-8k execution, representative downstream
quality, clean-user capacity or cold/warm compression latency qualification.
Historical PyTorch FP16 timings do not measure this ONNX CPU backend. Keep
`--experimental` and the explicit development acknowledgement until qualified.

## Legacy development backend

`DevelopmentScorer` and `inspect` retain the private PyTorch artifact contract.
Use `worker --backend pytorch` explicitly for that FP32 direct-child prototype;
it still requires its original pinned environment and artifact. The resident API
also retains its explicit development-only FP16 option. Select v1's separately
published LoRA model and historical benchmark results are not v2 evidence.
