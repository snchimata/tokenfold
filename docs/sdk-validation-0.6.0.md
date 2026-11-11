# SDK 0.6.0 validation scope

**Select remains unqualified.** Local installs, hosted CI and registry publication
are separate checks. No golden output/report-contract change was intended.

## Local installation and correctness

Windows x64, Rust 1.98, Python 3.12.14/3.13.12 and Node 22.23.4/24.22.2:

| Check | Local result |
| --- | --- |
| fmt, workspace clippy with warnings denied, rustdoc with warnings denied | Pass |
| Workspace Rust tests | 773 passed; 2 explicit worker/approved-model entrypoints ignored |
| Installed Python suites, including offline examples/typing/fixtures/lifecycle | 77 passed plus 37 subtests on both Python versions |
| npm suites with native build, full packed-install lifecycle/parity and example | 42 passed, zero skips on both Node versions |
| Maintained runtime unit contracts | 18 tests, one Windows executable-symlink privilege skip |
| Version checker, active-only drift test, workflow actionlint | Pass |
| Fidelity smoke | Pass, deterministic synthetic gate; not Select qualification |

An exact staged-source export without local model/archive directories built wheels,
sdist and native addon outside the checkout. Fresh `tokenfold[select]` installation
used only those local wheels; the extra resolved exactly the lightweight runtime.
The sdist was rebuilt and its installed wheel passed the full Python suite. Base
usage is offline/model-free, with no compiler or Python required for Node consumers.
Runtime import/license/provenance checks loaded no ML. Tests use source fixtures,
not source implementation imports, for installed SDK execution.

The five-target workflow covers Linux GNU x64/arm64, macOS x64/arm64 and Windows x64,
with Python 3.12/Node 22 and Python 3.13/Node 24 on each, plus sdist installation.
The [hosted CI run](https://github.com/snchimata/tokenfold/actions/runs/42892707678)
passed this full matrix and the sdist check at SDK commit
`3304fe2340b29ab72f96783283d7af0eebc9840a`. Release-preparation changes must pass
again on their exact head before merge. Fake workers are not real-model qualification.

## Measured local performance, not a promise

Windows 11 build 26300, AMD Ryzen 7 9800X3D (8 cores/16 logical), 31.06 GiB RAM;
NVIDIA RTX 5080, driver 610.88, 16303 MiB. Same 5195-byte, 50-row synthetic semantic
input, five warmups then 100 sequential samples per SDK case. Nearest-rank p95:

| Milliseconds | Python p50 / p95 | Node 22 p50 / p95 |
| --- | --- | --- |
| Opaque text fit | 0.4585 / 0.5188 | 0.5207 / 0.6874 |
| Typed JSON fit | 0.6421 / 0.7029 | 0.6741 / 0.8773 |
| Explicit folded fit | 2.8047 / 2.9695 | 2.9845 / 3.2840 |
| Standalone encoding counter | 0.3906 / 0.4057 | 0.4137 / 0.4547 |

Independent release-Rust component runs, 100 samples with preconstructed inputs:
JSON rendering 0.0034/0.0035ms, folded rendering including internal guide counts
1.7943/1.9251ms, native standalone counting 0.3674/0.3806ms. These are **not additive
exclusive SDK overhead attribution**. Receipt total/core/accounting timings remain
null. First process encoder initialization was 75.41ms; subsequent cached lookups
were below 0.0002ms resolution. Earlier untrimmed runs exposed approximately 83ms
reconstruction per fit; shared immutable bundled encoders remove that repeated work,
without changing counts or adding heuristic fallback. Earlier samples are retained.

### Resident inference

Existing pinned cu128 environment, explicit CPU/FP32 and CUDA FP32-parameters/BF16
profiles with no fallback. Per profile: five cold worker processes (not cold OS/file
cache), 25 resident fits and six closes. All 60 Python fits met the exact target with
scorer succeeded and selection applied; zero exceptions/refusals/fallbacks.

| Profile | Cold p50 / p95 ms (n=5) | Warm p50 / p95 ms (n=25) | Close p50 / p95 ms (n=6) |
| --- | --- | --- | --- |
| CPU | 7819.84 / 9658.69 | 101.96 / 103.68 | 381.53 / 536.55 |
| CUDA | 8060.53 / 8238.07 | 9.67 / 10.90 | 407.23 / 471.30 |

Warm maxima, retained rather than trimmed: CPU 255.61ms, CUDA 197.56ms. Five-sample
p95 is just the maximum, not a stable service-level estimate. OS/file caches were
uncontrolled; no causal cold-load speedup is claimed. Installed Node composed smoke
had one cold/two warm fits per profile: CPU 7775.85/104.61/105.05ms; CUDA
7952.00/11.02/9.48ms; closes 371.16/428.68ms. All six selected, met target and retained
the tiny payments oracle. This is too few samples for percentile claims.

SDK-process peak working set: Python 80.93 MiB; Node maxRSS 91.80 MiB. Worker/GPU peak
memory was **not measured**, not zero. No hard memory-quota/sandbox claim is made.
Training-group-count warnings are retained. These small local measurements do not
establish representative downstream quality or fresh-profile performance.

### Artifact identity

Performance above used working-tree-built artifacts (SHA-256):

| Artifact | SHA-256 |
| --- | --- |
| Python wheel | `b0cf47c31a19d4bbc6a129797c6662d7db36eeab673d709506496adbcacf5b1c` |
| Runtime wheel | `67ad48829dbeb1165753b5136c950e2e458af26b30a43325fc0913fdc537a324` |
| Node addon | `0829d43fad7f0069c4825800ad81209352d0846763c642f2b048ab303f775348` |

Their runtime source digest is `ceb72027a19b2d4e7e01ac71e6d89a5ff12ef7e9fb3958648e0f1bc8f16b2041`.
The tracked-source export uses LF source bytes, rather than Windows working-tree CRLF,
so its runtime source digest is `df523862e73e3de07000a62a0e78cb133bfaf68cb5af71e870a8ad3499311f86`.
It requires a **new explicit approval**, not reuse/relabeling of the previous config.
Clone-built artifacts and their functional receipts are distinct from the timings above.

## Bounded downstream attempt: inconclusive

Three synthetic tasks, two counterbalanced repetitions, baseline/candidate arms,
1024 output tokens per call, exact predeclared JSON oracles, no retry or provider
fallback. Exactly 12 requests to `liquid/lfm-2.5-2.6b:free` with the pinned
`liquid/fp8` endpoint all returned HTTP 400. There are **no valid paired answers**;
this is neither a quality win nor a measured 0% model-accuracy result.

All 12 attempts remain in the ledger. Provider usage and cost are unknown for all
12: a sum of known cost equal to zero is **not** evidence of zero spend. No modeled
cost substitutes for the missing actual cost. No additional calls were made.

Local candidate preparation did apply selection: rendered-text counts 173 -> 75
(target 95), 180 -> 85 (target 99), and 217 -> 99 (target 119). Those are exact local
`o200k_base` rendered-text counts, **not provider-request savings, billed usage,
answer-quality evidence or superiority qualification**.

## Actual user notebook

The ignored local `tokenfold_run/quickstart.ipynb` is distinct from the tracked
`examples/quickstart.ipynb`. Both were migrated; the tracked tour executes Core offline
by default and requires explicit environment consent plus an approved config for ML.
The actual notebook retains lowlevel redaction/recovery, uses 0.6 fit receipts, and
never installs into the old ML environment or downloads weights. The old notebook
backup and all execution attempts are retained; generated outputs are not committed.

All 11 code cells passed on each of six full runs: working-built and clone-built
artifacts, each on existing CPU, existing CUDA and a newly provisioned dedicated CPU
profile. That is 66 executed code cells, with all earlier attempts retained. The
fresh CPU environment was created through the installed explicit provision command
with `torch==2.11.0+cpu` and the pinned dependency profile, then exercised end to end.
This is a Windows fresh CPU-wheel functional result, not other-platform or fresh CUDA
provisioning certification. No existing ML environment was changed.

Actual notebook source SHA-256:
`06645b12d95e5c1a8784c57e6b24f0ea2cbf893d51c26b6615e827e39694577c`.
Clean-source functional artifacts used core wheel
`4c1e535d06ef21ba6af4389e2505b74e9eba774c39dd4d525b116ef75105cfd1`
and runtime wheel
`9069920c8e507f3da6da6fb878e5831818d2d67ee36c1774cd258323e671e26b`,
with new byte-pinned approvals for the LF source digest above. These notebook passes
do not change the inconclusive downstream result or replace exact-head release gates.
