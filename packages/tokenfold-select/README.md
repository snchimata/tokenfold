# Tokenfold Select development scorer

**Private development prototype, not a released or qualified Select v2 preview.**
No weights, datasets, hosted inference or model downloads are included. Nothing
changes Tokenfold's production defaults. Artifact inspection and protocol helpers
use only Python's standard library; actual scoring requires the separately
provisioned, pinned local ML environment recorded in the clean model recipe.

The package contains exact copies of the trained model's canonical joint encoder
and source-context packer. Their content hashes must match the caller-provisioned
clean compact12 artifact. No inference code imports a research checkout, ignored
training directory or developer-local path.

## Inspect without loading a model

```sh
python -m tokenfold_select inspect --artifact /absolute/model-directory --revision CHECKPOINT_SHA256
```

This verifies bounded metadata/file integrity and a declared completed, support-
passing development decision. It does **not** validate checkpoint tensors,
redistribution rights, task quality, latency, release approval or the local GPU.
Failed/unfinished student artifacts are refused.

## Existing direct-child protocol

Use the existing experimental Select approved-scorer configuration. The executable
must be the **native** Python interpreter, not a Windows venv shim or shell. Its
arguments are:

```text
-I -S /installed/tokenfold_select/bootstrap.py
--site-packages /absolute/provisioned/site-packages
worker --artifact /absolute/model-directory --revision CHECKPOINT_SHA256 --development
```

The caller supplies the existing `TOKENFOLD_SELECT_REQUEST_PATH` and
`TOKENFOLD_SELECT_RESPONSE_PATH`, an executable SHA256, model revision, scratch
root, explicit approval and a maximum 30-second child deadline. Protect auxiliary
code/model files and scratch storage as required by the existing Select adapter.
This is not an OS sandbox. The worker forbids network and descendants, emits only
finite id-aligned scores, refuses rather than truncates, and leaves renderer,
redaction, protected content, budgets and declared fallback to the existing
Tokenfold CLI. Never interpret fallback as a successful model-backed result.

The direct-child prototype is **FP32 only**. It does not enable an unqualified
FP16 cold profile. Actual native CUDA integration/parity and timing remain to be
verified before distribution.

## Dedicated-process resident API

`DevelopmentScorer(artifact, revision, cache_directory=absolute_path,
development=True, precision="fp32")` lazily admits a request and then loads the
model. `score(request)` uses the same schema-version-one query/id/text packet and
returns its standard score response. Model/tokenizer objects, not model results,
are retained. The cache must be outside the immutable artifact directory.

An explicit `precision="fp16"` casts the verified FP32 source model on the tested
RTX 5080/capability-12.0 environment only. No CPU/dtype fallback is attempted.
This experimental choice has not passed a complete launch comparison; it is not
a quality or speed claim. Use a dedicated process because execution settings
(threads, deterministic algorithms, TF32) are intentionally pinned there.

## Bounds and remaining gates

The component admits one complete 8,192-token window, 1–512 original groups,
a 1 MiB request and 64 KiB response. It refuses multi-window input before loading
a model. No source text is generated, cropped or split.

The unchanged score protocol contains no workload or retention-ratio field: it
cannot certify generic API-response grouping or 50% retention. Those belong to
the controller/renderer. This component is not the complete proposed bounded
JSON/API-response preview and does not replace full-v2 workload/long-source scope.

Remaining: real CUDA/native parity, complete matched-budget quality and warm/cold
latency evidence, separate contiguous pipeline reporting, grouping/controller
integration, source isolation and rights/accurate trained-model notices, clean-user
GPU provisioning, CI and publication approval. Do not ship model weights or claim
release readiness from CPU/mock/install checks.
