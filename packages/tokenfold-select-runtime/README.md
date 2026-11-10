# tokenfold-select-runtime 0.6.0

Lightweight maintained Select v3 runtime. Importing this package does not import
Torch, load weights, download assets or start a process. It contains no weights.
The archived Select protocol 1 and hash-listed v3 model snapshot are unchanged.
See `PROVENANCE.json` and `LICENSE` for port origins and deliberate differences.

## Explicit setup

Acquire a complete local v3 bundle separately at an immutable revision. Install
this runtime package into Python >=3.12, then explicitly provision a new destination:

```sh
python -m tokenfold_select_runtime provision --bundle /absolute/bundle --manifest-sha256 EXACT_SHA256 --device cpu --config-out /absolute/select-runtime.json --approved-by OWNER --scratch-root /absolute/caller-owned-scratch
```

Only this command may fetch dependencies. It reserves a dedicated environment next
to the config, installs exact CPU or CUDA (`--device cuda:0`) package pins and
atomically publishes approval without overwriting an existing approval/environment.
Failed setup leaves the reserved environment unapproved; use a new destination.
CPU uses FP32. CUDA requires FP32 parameters, BF16 autocast with its cache disabled,
and TF32 disabled; it never falls back to another device/precision.

Approval pins the executable, maintained Python source bytes/version, local model
manifest/revision, profile, owner and scratch root. Protect those assets and the
config from modification; changing any pin requires explicit reapproval. Native
startup rechecks executable/source/manifest, and the offline worker checks complete
model files/geometry and exact package versions before inference. Dependency wheel
installation and functionality on a newly provisioned platform require their own
evidence; this repository's existing-environment functional probes are not that proof.

## Native lifecycle

`SelectRuntimeConfig::from_path`/`from_json` creates immutable configuration;
`scorer_info()` provides the expected Core pin. One `ScorerDispatcher` (also named
`ResidentScorer`) owns a lazy direct child and FIFO. `submit(request, cancellation,
queue_deadline)` returns immediately with a `ScoreTicket`. Native async bindings
must use `try_take()` and `on_complete()` to schedule host work, not block a Core or
libuv executor on admission. Completion callbacks must only schedule work. `wait()`
and synchronous `ResidentScorer::score()` are for synchronous host callers.
`ScoreCompletion::into_reply(action_id)` passes typed diagnostics and timings to
Core, which alone creates the receipt. Immediate submission failures can use
`RuntimeError::score_failure()`; Busy/Closed/Cancelled/Config are raised, not fallback
receipts. Drop cancels only unfinished tickets; dropping a consumed score cannot
cancel its enclosing fit.

Defaults are 32 waiting jobs (configurable 0..128), 5s queue wait, 120s cold
startup/load, 5s inference (configurable 1..30s) and 1s graceful cleanup. A saturated
queue rejects admission with Busy; queued cancellation/expiry does not touch the
active worker. `warmup()` explicitly loads weights and coalesces existing startup;
`close()` rejects admission, wakes waiters, kills/waits as needed and is idempotent.
The shared owner reaps its direct child on Drop too. No global pool, within-fit
retry, automatic provisioning, truncation or device/precision fallback exists.

Protocol 2 uses bounded little-endian length-prefixed JSON and monotonic IDs.
`load` initializes without weights unless explicitly warming up. `score` admits the
whole request/window layout, emits `ready/admitted`, loads weights if needed,
emits `ready/loaded`, then scores. The single cold startup/load deadline runs until
loaded; inference has a separate deadline. A post-start admission refusal counts
as an attempted `selection_limit`; native prestart size admission does not. An OS
spawn call that fails still counts as a start attempt. Malformed/duplicate frames,
stale IDs, nonfinite scores, crash, timeout and active cancellation invalidate/reap
the worker; only a later explicit request may cold-start again.

Cleared environments, isolated source imports, offline audit guards, bounded
reader/writer threads and confined owned scratch cleanup are not an OS sandbox or
hard memory quota. Applications own OS enforcement and permissions. Selection
quality remains unqualified, especially group counts outside training and multiple
windows. Callers must group/require semantic dependencies.

## Checks

```sh
python -m unittest discover -s packages/tokenfold-select-runtime/tests -v
cargo test -p tokenfold-adapters --locked --test select_runtime
python packages/tokenfold-select-runtime/tests/functional_runtime.py --python /absolute/existing/python --bundle /absolute/bundle --device cpu --scratch-root /absolute/owned/scratch
```

The last probe never installs anything. Invoke CUDA separately with `--device cuda:0`;
unavailable CUDA is explicitly skipped, never passed or replaced with CPU.
