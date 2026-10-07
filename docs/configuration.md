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

## Experimental generative summarizer

```sh
tokenfold summarize context.json --experimental --query "What changed?" --target-tokens 512 --model-config approved-model.json --inference-timeout-ms 30000
```

Input uses the Select document shape without `fallback_score`: prefix, suffix and
ordered `{id,text,required}` groups. Prefix/suffix and required group text are copied
verbatim outside generation. This is **not** the existing literal-extract semantic
API and does not change compression defaults. Generated paraphrases remain
**unverified**, not production-admissible or proven superior to other compressors.

`approved-model.json` uses the approval fields documented for `--scorer-config` above.
The approved direct child receives the same request/response environment paths, with
`kind: "generative_summary"`, schema version 1, pinned model revision, instruction,
query, target tokens and the full source document. Its response must contain only:

```json
{"schema_version":1,"model_revision":"<pinned revision>","summary":"...","source_ids":["g1"]}
```

Only optional group IDs may be cited. Native code deduplicates citations and attaches
complete literal groups in source order alongside `summary_unverified`. Attribution
is not entailment or completeness verification. The full wrapper, evidence and
protected text count toward the target. Empty/malformed/unauthorized/oversized,
secret-shaped, over-budget or non-shrinking candidates and runtime failures return
the exact assembled source, with a visible reason and possibly `budget_met: false`.
Secret-shaped source/query is refused before model invocation or output.
The separate `generative_summary` receipt goes to stderr or `--receipt-file`.
Optional `inference_usage: {"input_tokens":123,"output_tokens":45}` in the runtime
response carries unsigned counters into the receipt, with `usage_provenance` set to
`approved-runtime-reported`. The local bridge supplies server-reported counters when
both are valid. These are not local-estimator counts or verified billing. Counts are
retained on native candidate rejection (including budget failure); malformed responses,
transport failures and interrupted calls may still have unknown usage (`null`). The local bridge also returns fixed `generation_failure` codes (`incomplete_response`, `model_mismatch`, `malformed_candidate`) with any reported counters; no rejected model reply is included. HTTP 4xx/5xx/other statuses, transport errors, socket timeouts and invalid runtime envelopes are classified as `http_client_error`, `http_server_error`, `http_unexpected_status`, `transport_failed`, `runtime_timeout` and `invalid_runtime_response`, respectively, without exporting server bodies or exception text. Native process/deadline failures still use `runtime_failed`. None is retried; all trigger raw fallback. Cost is
always unknown (`null`), never assumed zero. One-shot summarization adds an inference
call and may cost more than it saves.

### Local Qwen without Ollama

The helper now defaults to a **resident local Transformers/PyTorch model**, without
Ollama. Start the model once explicitly; repeated CLI requests reuse its tokenizer
and weights. **Qwen3.5-0.8B** is the default; `--size 2b` and
`--size 4b` are explicit capacity options, not verified quality guarantees.
A tokenizer alone cannot generate summaries: the worker loads both tokenizer and
[Qwen's model weights](https://huggingface.co/Qwen/Qwen3.5-0.8B).

This follows [ACON's load-once pattern](https://github.com/microsoft/acon/blob/d63f9ae18959dc7215ff62899c94c5e8c56847ae/src/productive_agents/llm.py#L524-L580):
its local vLLM constructor creates the engine and tokenizer, and generation reuses
those objects; its server client delegates to a separately running engine.
For repeated requests, keep one server per explicitly selected model running.
The short-lived Tokenfold client does not import PyTorch or reload weights.
Use the native llama.cpp route below on supported Windows hardware, or resident
Transformers for direct official-checkpoint loading; one-shot Transformers is a
diagnostic path, not the repeated-request setup. Model residency avoids loading
cost, but does not cache generated summaries or establish a fastest runtime.

```sh
# Optional dependencies in your own environment (not Rust dependencies):
python -m pip install torch transformers huggingface_hub
# Explicit one-time download; replace <commit-sha> with a pinned HF revision:
hf download Qwen/Qwen3.5-0.8B --revision <commit-sha> --local-dir local-qwen-08b
# Terminal 1: load once and keep running until you stop it (Ctrl+C).
python scripts/local-summarizer.py --serve --model-dir local-qwen-08b --port 8000 --max-output-tokens 256
# Add --device cuda after installing compatible CUDA-enabled PyTorch.
# Terminal 2: approve a bounded, stdlib-only loopback client, not a model reloader.
mkdir private-summarizer-scratch
python scripts/local-summarizer.py --write-config approved-model.json --backend resident --port 8000 --max-output-tokens 256 --scratch-root private-summarizer-scratch --approved-by "your name"
tokenfold summarize context.json --experimental --query "What changed?" --target-tokens 512 --model-config approved-model.json --inference-timeout-ms 30000
# Optional: download Qwen/Qwen3.5-2B or Qwen/Qwen3.5-4B separately,
# then configure --size 2b/4b with the corresponding --model-dir.
```

Use a Transformers release supporting `Qwen3_5ForCausalLM` and its official
multimodal-to-text checkpoint mapping. The text-only loader skips unused vision
weights and refuses missing or mismatched language weights. Oversized inputs are
rejected before generation; one-shot mode rejects them before weights load. The
worker uses local files only, safetensors, no remote Python code, offline/telemetry-off
Hub settings, non-thinking generation, SDPA attention, KV caching and inference mode.
CUDA is selected when your PyTorch supports it; otherwise CPU float32 is used.
For NVIDIA GPUs, install the matching CUDA-enabled wheel using
[PyTorch's platform selector](https://pytorch.org/get-started/locally/); an ordinary
Windows `pip install torch` may install a CPU-only build even when a GPU is present.
Check `python -c "import torch; print(torch.__version__, torch.cuda.is_available())"`
before generating approval. Add `--device cuda` to configuration generation to
require CUDA and refuse silent CPU fallback; `--device cpu` forces CPU, and
`--device auto` preserves automatic selection. For resident mode set the device on
`--serve`, not the client approval. No driver or runtime upgrade is implicit.
In a cleared Windows child, the worker derives `SystemRoot` and the current OS
username through Windows APIs so GPU libraries can initialize; it does not inherit
caller credentials, proxy settings or the caller's broader environment.
CUDA uses bfloat16 when supported, otherwise float16. There is no automatic download,
server start, retry, model escalation or substitution. Complete input plus output
allowance is capped at 16,384 model tokens, without source truncation. Output without
EOS triggers incomplete-response fallback and retains consumed model-token counts.
Direct decoding has no JSON grammar: malformed or unauthorized output is still
rejected by native guards. All summaries remain semantically unverified.

Resident mode loads one model/tokenizer once and serializes requests in a single
explicitly started loopback process. `/health` reports readiness and its model ID.
Each request performs a fresh generation and reports its consumed token counts;
this is model reuse, not summary/answer caching or cross-request conversation history.
The client sends the approved output allowance; exceeding the server's configured
maximum is rejected rather than silently downgraded. Native input/evidence/secret,
protected-text and full-budget gates remain unchanged. Server startup is separate
from warm request latency; include it when reporting deployment economics.

The server keeps weights until you stop it and does not log/save request bodies.
It is a caller-owned model runner, not an authenticated compression API or OS sandbox:
only use it on a trusted local machine through Tokenfold's guarded native client.
Client timeouts/disconnects do not guarantee cancellation of an in-flight server
kernel. Generation remains bounded by context/output allowances; there is no retry,
automatic daemon launch, model download, model escalation or history recovery.

Explicit `--backend transformers --model-dir ...` configuration retains the
one-shot direct-child alternative, which loads weights per generation. Cold loading
or CPU inference may exceed the native deadline (default 5 s, maximum 30 s), returning
raw source. Same-input artifact reuse below avoids generation only for identical
inputs. For higher concurrent/batched throughput, an already warmed engine such as
[vLLM](https://huggingface.co/Qwen/Qwen3.5-0.8B#quickstart) is optional:

```sh
vllm serve /absolute/path/to/local-qwen-08b --host 127.0.0.1 --port 8000 --served-model-name Qwen/Qwen3.5-0.8B
python scripts/local-summarizer.py --write-config approved-server.json --backend openai --scratch-root private-summarizer-scratch --approved-by "your name"
```

For a native Windows runner without Ollama or Python in the inference server,
the same bridge also works with **llama.cpp**. A development integration used
the official `b11399` Vulkan x64 build and converted the pinned official
Qwen3.5-0.8B snapshot to BF16 GGUF with that build's `convert_hf_to_gguf.py`.
Download and verify the official runtime separately; keep its companion DLLs.
Install the converter requirements in your own Python environment once. Do not
substitute an unverified community checkpoint or treat the served alias as weight
attestation. Pin the converter, runtime/DLLs, original weights and resulting GGUF.

```sh
# One-time conversion using the matching, pinned llama.cpp source checkout:
python llama.cpp/convert_hf_to_gguf.py local-qwen-08b --outtype bf16 --outfile local-qwen-08b-bf16.gguf
# Terminal 1: keep the model loaded; select a device reported by --list-devices.
llama-server --model local-qwen-08b-bf16.gguf --alias Qwen/Qwen3.5-0.8B --host 127.0.0.1 --port 8000 --ctx-size 16384 --parallel 1 --device Vulkan0 --n-gpu-layers 99 --flash-attn on --no-context-shift --predict 512 --chat-template-kwargs '{"enable_thinking":false}' --log-disable
# Terminal 2: reuse the existing bounded client and native guards.
python scripts/local-summarizer.py --write-config approved-native-server.json --backend openai --port 8000 --max-output-tokens 512 --scratch-root private-summarizer-scratch --approved-by "your name"
tokenfold summarize context.json --experimental --query "What changed?" --target-tokens 512 --model-config approved-native-server.json --inference-timeout-ms 30000
```

The example shell must pass the JSON argument literally (PowerShell versions can
require different native-argument quoting). BF16 conversion is not integer
quantization and does not guarantee identical outputs across engines. Choose a
runtime/device supported by your hardware; there is no automatic fallback or
download. The server may reuse prompt-prefix state, but every request still
generates a new candidate. Include startup and first-request warmup in measurements.
The four-call development run completed without runtime failures, but one request
fell back for budget overflow and another generated a false nationality claim.
This establishes runner integration, **not quality qualification or universal
performance superiority**. 2B/4B remain explicit, separately provisioned options.

On the tested NVIDIA RTX 5080, the official `b11399` **CUDA 13.4 x64** build
completed the optional 4B profile without the Vulkan trial's runtime timeouts.
Provision and verify both the matching `llama-b11399-bin-win-cuda-13.4-x64.zip`
and `cudart-llama-bin-win-cuda-13.4-x64.zip`; make the latter's DLL directory
available on the server process's `PATH`. Confirm `llama-server --list-devices`
and explicitly choose `--device CUDA0` instead of `Vulkan0`. Use your own
compatible driver/runtime; Tokenfold does not install or switch them.
For 4B, separately convert its pinned official snapshot and change both the
server alias to `Qwen/Qwen3.5-4B` and client approval to `--size 4b`.
The observed 4B CUDA run took 5.645 seconds to start and 2.620–6.165 seconds per
native receipt, but still missed evidence and returned one budget fallback.
This is a useful tested NVIDIA route, not a general hardware ranking or a
verified higher-quality default.

The same pinned native CUDA runtime also completed a resident **2B** BF16
development trial on five previously observed training cases, with original and
short evidence IDs (ten fresh generations in one server process). Startup was
2.588 seconds; command times were 1.633–1.877 seconds with original IDs and
1.208–1.370 seconds with short IDs. Original IDs produced one budget fallback;
short IDs passed structural/budget guards on all five cases, but still missed
required evidence on one case and generated false company/location relationships
on another. These are small training measurements, not held-out quality evidence
or a reason to automatically select 2B or short IDs. Weights stay loaded across
requests; native acceptance still does not certify summary truth.

For an opt-in short evidence-ID experiment with an already running native 4B
server (matching alias and port), create a **new** approval:

```sh
python scripts/local-summarizer.py --write-config ./qwen35-4b-shortids.approval.json --backend openai --size 4b --port 8080 --short-source-ids --max-output-tokens 512 --scratch-root ./private-summarizer-scratch --approved-by local-user
```

`--short-source-ids` is loopback `openai`-backend-only and off by default. It sends
source-order `sN` aliases for optional group ID fields, leaving original source
text and protected context unchanged, and translates selected aliases back before
native authorization/evidence compilation. Unknown aliases fail closed, including
IDs outside the optional set; known inference usage survives rejected or oversized
translated candidates. The option is bound into the approval arguments. It does
not load another model, retry, cache answers or verify summary semantics. Training
results are mixed: 4B retained more economical evidence in one retrieval case,
but another training case still omitted a supporting document; 0.8B regressed on
one retrieval case. Do not treat this option as a qualified quality default.

The server bridge uses only `127.0.0.1`, port 8000 in generated configurations
(`--port` overrides it), without credentials, proxies, redirects or retries.
The resident Transformers server uses length-delimited HTTP/1.1 replies; its
five-second socket timeout bounds header/body and idle connection reads, not
model inference. Requests remain serialized: close consumed client connections
promptly rather than leaving an idle connection ahead of another request.
It requests non-thinking structured JSON; unsupported settings fail visibly.
Existing explicit port-only bridge arguments remain usable. The model sees only
optional group IDs and protected context without its IDs; native output keeps
protected text exactly. 2B/4B and GPU throughput are not yet live-qualified here.

Configuration generation requires an existing private scratch directory and a local
model directory for one-shot direct inference, approves the absolute Python executable's
SHA-256, and refuses overwrite. Pin/protect the snapshot, Python environment, worker
script and arguments yourself: executable approval alone does not verify weights.
The configured model ID is a label, not artifact attestation. Keep local weights,
approvals and generated artifacts out of Git. No hosted private-data export is enabled.

The process timeout bounds the direct child only, not an optional server's ongoing
inference. Approval is not an OS sandbox; custom runtimes must not launch descendants.
Default child timeout is 5 seconds, maximum 30 seconds. Source is
bounded to 1 MiB assembled/2 MiB serialized, query to 4 KiB, groups to 512, model
response to 64 KiB. Token counts report the local estimator, not provider templates.

Local integration check (build the debug CLI first):

```sh
cargo build -p tokenfold-cli --locked
python scripts/test_local_summarizer.py
```

Checks include synthetic direct-loader tests and native CLI-to-loopback integration,
not a model-quality or throughput benchmark. Local compute cost is unknown, not zero.

Before invoking the runtime, `summarize` keeps the exact assembled source if it already
fits the target (`already_within_budget`), contains no optional groups
(`no_optional_groups`), or protected content alone reaches the target
(`protected_budget`). These receipts set `runtime_invoked: false`, with unknown/null
usage, not fabricated zero-cost inference. Input/query safety checks still run.

### Explicit same-input summary reuse

```sh
# The first successful generation saves an unverified candidate, not the source:
tokenfold summarize context.json --experimental --query "What changed?" --target-tokens 512 --model-config approved-model.json --save-summary private-summary.json --inference-timeout-ms 30000
# Later, only with exactly the same source/grouping, query, approval, target and estimator:
tokenfold summarize context.json --experimental --query "What changed?" --target-tokens 512 --model-config approved-model.json --reuse-summary private-summary.json
```

Artifact persistence is **opt-in** and may contain private generated facts and source
IDs: protect its parent directory and file. Saves refuse overwrite and require paths
separate from output/receipt files. Source/query are represented by hashes, not copied
into the artifact. This is not anonymous storage (the summary itself contains facts).
Only candidates passing the native evidence/output/budget/no-growth gates are saved.

Reuse never calls the model or silently regenerates on mismatch. Every reuse reruns
current decoded-input guards, source-ID authorization, protected-text assembly,
secret detection and full-payload token accounting. Mismatched/unavailable/malformed
artifacts return the exact current source with a visible fixed fallback reason.
Changed queries, required flags, target, estimator or approval are not cache hits.
It is **not** cross-query or changed-history reuse, nor a semantic proof or signed
artifact. Tampered factual paraphrases may remain unverifiable; protect the artifact.

Receipts distinguish `artifact_reused` and `runtime_invoked`. `inference_usage` stays
null for the reuse; original counters appear separately in `reused_inference_usage`
and must not be billed again. Model revision describes the recorded candidate, not
verification of current server weights or availability. Count the first generation's
cost once across genuine repeated requests; do not count repetitions as independent
quality trials or claim cached latency as cold generation latency.
## Experimental validation-only Select

`tokenfold --experimental select --query validate-only --target-tokens 1 --validate-only`
reads the existing context input schema and performs the same bounded raw/decoded
secret checks, but does not initialize a tokenizer, select groups, invoke a scorer
or emit source text. It conflicts with scorer configuration, output and receipt
options. Successful validation is not semantic admission or a compression result.
Success emits no stdout/stderr; secret refusal preserves exit code 3 and does not
echo secret text. Missing `--experimental` uses the existing configuration-error
exit code 5; incompatible output/scorer options are rejected by argument parsing.
