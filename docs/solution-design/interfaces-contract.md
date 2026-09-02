# Tokenfold Interface Contract

| Field | Value |
| --- | --- |
| Status | Proposed breaking interface for the next pre-1.0 release |
| Scope | CLI, Core adapters, Python, TypeScript, MCP, receipts, TOON encoding, and retrieval |
| Compatibility policy | Clean break; do not retain deprecated aliases because the active user base is under five |
| Normative language | **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are requirements terms |

## 1. Design principles

| Principle | Contract |
| --- | --- |
| Simple default | `tokenfold compress` MUST use the balanced preset without requiring flags. |
| Separate concerns | Preset, token target, recoverable pruning, and output encoding MUST remain independent concepts. |
| Reversible presets | Every preset-selected transform MUST preserve the post-security semantic value and be mechanically decodable where it introduces framing. Presets MUST NOT omit source information. |
| Explicit omission | Removing rows from the immediate payload MUST require the explicit `--prune` option or an equivalent typed SDK policy. |
| Predictable encoding | Explicit TOON output MUST either produce verified TOON or fail. It MUST NOT silently fall back to JSON. |
| Self-contained decoding | Reversible output MUST carry self-describing framing. A receipt MUST NOT be required to decode it. |
| Byte safety | Payload APIs and file/stdout paths MUST preserve bytes. Text conveniences MUST decode strictly unless the caller explicitly chooses replacement behavior. |
| Fail closed | Redaction, protected-content checks, pruning persistence, and encoding round-trip verification MUST fail closed. |
| Exact reporting | Every completed operation MUST report exact final token counts using the resolved estimator and retain per-transform provenance. |
| Deterministic streams | Output routing MUST NOT change based on whether a stream is attached to a TTY or pipe. |

The reversibility baseline begins **after** the mandatory security pass. Secret redaction is an
intentional, irreversible safety operation: decoding MUST NOT restore redacted secrets. For JSON,
reversibility means equality of the sanitized JSON value, not original whitespace or equivalent
number spelling. For reversible text framing, it means byte identity with the sanitized text.

## 2. Public command surface

| Command | Purpose | Input default | Primary output |
| --- | --- | --- | --- |
| `tokenfold compress [INPUT]` | Compress and optionally encode a payload | stdin when omitted or `-` | Compressed payload |
| `tokenfold inspect [INPUT]` | Side-effect-free projection of the same policy | stdin when omitted or `-` | Receipt |
| `tokenfold decode [INPUT]` | Reverse TOON and self-describing reversible transforms | stdin when omitted or `-` | Restored payload |
| `tokenfold retrieve REFERENCE` | Restore bytes referenced by a retrieval marker | Required positional reference | Retrieved bytes |

### 2.1 Compression and inspection options

| Option | Type / values | Default | Validation and meaning |
| --- | --- | --- | --- |
| `--format` | `auto`, `json`, `openai`, `anthropic`, `text`, `command`, `diff` | `auto` | Classifies input modality and gates applicable transforms. |
| `--preset` | `conservative`, `balanced`, `aggressive` | `balanced` | Selects an ordered, format-aware set of reversible transforms and validated limits. |
| `--target-tokens` | Positive integer | None | Soft whole-document token goal unless `--require-target` is present. |
| `--require-target` | Flag | Off | Requires `--target-tokens`; an unmet target suppresses payload emission and exits `7`. |
| `--encoding` | `json`, `toon` | JSON for generic JSON; otherwise native representation | Valid only for generic JSON. TOON is applied last and round-trip verified. |
| `--prune` | Flag | Off | Enables recoverable omission using the current deterministic heuristic. |
| `--keep-ratio` | Float where `0.0 < R <= 1.0` | None | Token-weighted lower retention boundary across eligible candidates; requires `--prune`. |
| `--preserve` | Dotted object-key path, repeatable | Empty | Protects the named array or its conservative eligible ancestor; requires `--prune`. This is not JSONPath. |
| `--retrieval-store` | Directory path | Resolved XDG store | Overrides the content-addressed retrieval store; relevant to pruning and retrieval. |
| `--retrieval-namespace` | Valid namespace string | `default` or resolved configuration | Namespaces stored originals and emitted references. |
| `--receipt-file` | File path | None | Redirects the receipt from its normal stream into the file. It does not duplicate it. |
| `--receipt-format` | `json`, `text` | `json` | Controls receipt presentation only; it does not describe input or payload encoding. |
| `--quiet` | Flag | Off | Suppresses routine successful diagnostics. It MUST NOT suppress fatal errors. |

### 2.2 Command-specific options

| Option | Commands | Contract |
| --- | --- | --- |
| `-o, --output PATH` | `compress`, `decode`, `retrieve` | Writes raw result bytes directly to the path instead of stdout. No cross-platform atomic-replacement guarantee is made until implemented and tested. |
| `--from auto\|json\|toon\|text` | `decode` | Disambiguates the encoded source. `auto` MUST reject ambiguity rather than guess. |

### 2.3 Flag dependency rules

| Combination | Result |
| --- | --- |
| `--require-target` without `--target-tokens` | Configuration error, exit `5` |
| `--keep-ratio` without `--prune` | Configuration error, exit `5` |
| `--preserve` without `--prune` | Configuration error, exit `5` |
| `--prune` without either target or keep ratio | Configuration error, exit `5`; no invisible pruning default |
| `--encoding toon` with non-generic JSON | Invalid input/combination, exit `2` |
| `--output` on `inspect` | Configuration error; use `--receipt-file` |
| Unknown preset, format, encoding, or decode source | Argument/input error, exit `2` |

## 3. Stream and file contract

| Command | stdin | stdout | stderr | With `--receipt-file` |
| --- | --- | --- | --- | --- |
| `compress` | Payload if `INPUT` is absent | Payload only, unless `--output` or hard-target failure | Receipt plus diagnostics | Receipt is redirected to the file; diagnostics remain on stderr |
| `inspect` | Payload if `INPUT` is absent | Receipt only | Diagnostics only | Receipt is redirected to the file and stdout is empty |
| `decode` | Encoded payload if `INPUT` is absent | Restored payload unless `--output` | Diagnostics only | Not applicable |
| `retrieve` | Not used for the reference | Retrieved bytes unless `--output` | Diagnostics only | Not applicable |

| Stream rule | Requirement |
| --- | --- |
| Payload purity | Payload stdout MUST contain no receipt, progress, or diagnostic bytes. |
| Fatal diagnostics | Fatal failures MUST emit an actionable one-line diagnostic to stderr unless a future explicit machine-error format replaces it. |
| TTY behavior | Receipt routing and format MUST NOT change automatically based on TTY detection. |
| Hard-target failure | `compress --require-target` MUST emit no payload to stdout and MUST NOT create or replace `--output`. |
| Direct output | File output MUST write the same bytes that stdout would have received. Shell encoding and newline conversion MUST NOT be applied. |

Example hard-target diagnostic:

```text
tokenfold: budget unmet: achieved 3,412 tokens; target 3,000. Output suppressed (--require-target).
```

## 4. Exit-code contract

| Code | Meaning | Examples |
| ---: | --- | --- |
| `0` | Valid completed outcome | Compressed, passthrough, soft `best_effort`, or soft `unreachable` |
| `2` | Invalid input or malformed encoded payload | Invalid JSON/TOON, incompatible encoding, unknown reference syntax |
| `3` | Safety or redaction failure | Protected-content violation, mandatory redaction failure |
| `4` | Estimator failure | Required tokenizer unavailable or failed |
| `5` | Configuration or option-combination error | Missing pruning control, invalid ratio, missing required target |
| `6` | Internal, I/O, or retrieval-store failure | Read/write error, store cannot open, internal invariant failure |
| `7` | Target unmet with `--require-target` | `best_effort` or `unreachable` hard-budget result |
| `8` | Valid retrieval reference is unavailable | Missing or expired stored original |

| Exit behavior | Requirement |
| --- | --- |
| Soft target | An unmet `--target-tokens` without `--require-target` MUST remain exit `0` and be represented in the receipt. |
| Inspect hard target | `inspect --require-target` MUST emit/redirect its receipt and exit `7` when the projected result misses the target. |
| SDK mapping | SDKs MUST use typed errors/outcomes rather than expose CLI integer codes as their primary interface. |

## 5. Compression execution order

| Phase | Operation | Invariants |
| ---: | --- | --- |
| 1 | Input detection and validation | Resolve modality; reject incompatible options before side effects. |
| 2 | Mandatory security pass | Redact detected secrets before reports or persistence. No public disable switch. |
| 3 | Reversible preset transforms | Apply ordered, applicable transforms; exact round-trip and never-worse gates decide adoption. |
| 4 | Recoverable pruning | Run only when explicitly requested and still warranted; persist each omission before replacing it with `$tf_ref`. |
| 5 | Output encoding | Keep native/JSON output or explicitly encode generic JSON as TOON; verify round trip. |
| 6 | Exact recount and receipt | Recount the actual emitted bytes and finalize operation, budget, transform, pruning, encoding, and retrieval reports. |
| 7 | Emission | Enforce stream/file and hard-target rules. |

### 5.1 Preset contract

| Preset rule | Requirement |
| --- | --- |
| Default | `balanced` |
| Reversibility | Every automatically selected transform MUST preserve the sanitized semantic value. Any transform that introduces framing MUST have a mechanical inverse and pass its round-trip gate. |
| Applicability | Inapplicable automatic transforms MUST be skipped with a typed reason, not treated as failures. |
| Ordering | Transform order MUST be canonical and versioned. Decoding uses the inverse order. |
| Never worse | An automatic transform that does not reduce exact tokens MUST be rolled back. |
| Irreversible transforms | `schema_compaction` (currently truncates `examples`), `log_compaction`, and `diff_compaction` MUST remain outside presets unless redesigned to preserve the sanitized semantic value and support the required inverse. Evidence or semantic-likelihood claims are not reversibility. |
| Manual transforms | Raw `--enable`, `--disable`, and `--experimental` controls are not part of the normal public interface. Research-only controls MAY remain hidden from standard help and MUST validate canonical IDs. |

## 6. Target and pruning semantics

### 6.1 Budget outcomes

| Budget status | Meaning |
| --- | --- |
| `not_requested` | No target was supplied. |
| `met` | Final token count is less than or equal to the requested target. |
| `best_effort` | The allowed pipeline completed but the final result remains above target. |
| `unreachable` | Protected content alone exceeds the target, so no permitted pipeline could meet it. |

Operation status and budget status MUST remain separate:

| Field | Values | Purpose |
| --- | --- | --- |
| `status` | `compressed`, `passthrough` | Describes whether emitted bytes changed. |
| `budget.status` | `not_requested`, `met`, `best_effort`, `unreachable` | Describes target attainment. |

### 6.2 Pruning rules

| Inputs | Behavior |
| --- | --- |
| Target met by reversible transforms | Skip pruning with reason `target_already_met`. |
| Target unmet and pruning enabled | Prune only until the target is reached or the configured retention boundary prevents further omission. |
| No target and keep ratio supplied | Use the keep ratio to constrain the token-weighted candidate pool. |
| Target and keep ratio supplied | Target is the goal; keep ratio is the lower retention boundary. Stop at whichever boundary is reached first. |
| No pruning controls | Never omit rows. |

| Pruning invariant | Requirement |
| --- | --- |
| Ratio meaning | Token-weighted share of eligible candidate cost, not a percentage of row count and not a whole-document guarantee. |
| Candidate economy | An item MUST remain inline when its retrieval marker costs at least as many tokens. |
| Global allocation | Selection MAY allocate across all eligible arrays rather than applying `ceil(array_length * ratio)` independently. |
| Small arrays | Do not invent a count-derived retention rule. Exact marker cost, global selection, preservation, and the final never-worse gate govern behavior. |
| Preservation syntax | Dotted object keys only. Wildcards, filters, and array selectors are unsupported. |
| Existing markers | Input already containing reserved retrieval markers MUST be rejected or explicitly protected; markers MUST never be recursively pruned. |
| Persistence | A row leaves the payload only after the durable store accepts it. Refused writes leave the row inline. |
| Preview | `inspect` performs the same selection projection but MUST NOT write to the store; projected references MUST be marked non-retrievable in the receipt. |
| Provider envelopes | Pruning generic OpenAI or Anthropic message envelopes is forbidden. |

## 7. TOON encoding contract

| Topic | Requirement |
| --- | --- |
| Classification | TOON is an output encoding of the JSON data model, not a preset, transform-selection flag, or pruning strategy. |
| Eligibility | Only generic JSON is eligible initially. Provider request envelopes MUST be rejected. |
| Position | TOON encoding runs after reversible transforms and optional pruning. |
| Explicitness | A preset MUST never select TOON implicitly. |
| Verification | Decode emitted TOON and compare the resulting JSON value with the pre-encoding value. Mismatch is a hard failure with no payload. |
| Size increase | Explicit `--encoding toon` MUST still emit verified TOON when it is larger, with a typed warning and signed token delta. It MUST NOT fall back to JSON. |
| Automatic choice | `--encoding best`/`auto` is out of scope until a consumer contract exists for variable output syntax. |
| Receipt | Report tokens before and after encoding, signed delta, verification result, codec/version, and output encoding. |

Current research evidence supports explicit rather than automatic TOON selection:

| Representation | Seven-case corpus tokens | Difference from compact JSON |
| --- | ---: | ---: |
| Compact JSON | 10,281 | Baseline |
| Tokenfold current checkout | 9,071 | -11.77% |
| TOON 4.1.1 | 11,044 | +7.42% |

The corpus includes a flat-uniform case where TOON performs well and several nested cases where it does not. These figures are research evidence, not a release claim.

## 8. Decode contract

### 8.1 Inversion branches

| Detected/source format | Inversion order | Output |
| --- | --- | --- |
| TOON | TOON to JSON AST, value-dictionary inverse, column-fold inverse | Canonical JSON bytes |
| JSON | Value-dictionary inverse, column-fold inverse | Canonical JSON bytes |
| Text/command | Reversible log-fold inverse | Original text bytes |
| Unframed input | No-op only when unambiguously already decoded; otherwise a clear invalid-input error | Unchanged bytes or error |

### 8.2 Self-description

| Framing | Purpose |
| --- | --- |
| `__tf_cols__` + `__tf_rows__` | Self-describing JSON column fold |
| `__tf_dict__` + `__tf_data__` + `__tf_ref__` | Self-describing JSON value dictionary |
| `__tf_logfold1__` | Self-describing reversible log fold |

| Decode invariant | Requirement |
| --- | --- |
| Receipt independence | Decode MUST NOT require a receipt. A receipt can be missing, mismatched, or stale. |
| Security boundary | Decode restores the value presented to reversible transforms after mandatory redaction; it MUST NOT reconstruct redacted secrets. |
| JSON identity | Reversibility is defined as sanitized JSON-value identity; original whitespace and equivalent number spelling are not preserved. |
| Text identity | Reversible text transforms MUST restore the sanitized pre-transform bytes exactly. |
| Pruned payloads | Decode reverses syntax and structural transforms but leaves `$tf_ref` markers. Retrieval is a separate operation. |
| Collision safety | Real input resembling a sentinel MUST remain unchanged unless it is a well-formed Tokenfold frame; transform adoption round-trip gates remain mandatory. |

## 9. Retrieval contract

### 9.1 Accepted references

| Reference form | Example | Supported behavior |
| --- | --- | --- |
| Raw hash | `a1b2...` (64 lowercase/uppercase hex characters) | Resolve using explicit or configured namespace |
| Legacy text marker | `[tokenfold:retrieve hash=a1b2... namespace=default]` | Parse embedded hash and namespace |
| Serialized JSON marker | `{"$tf_ref":{"alg":"sha256","hash":"a1b2...","namespace":"default"}}` | Parse the nested marker object |
| SDK object/dictionary | Equivalent in-memory `$tf_ref` object | SDK validates and resolves without requiring manual serialization |
| Compression report path | Not currently retrievable | Reject clearly until the report carries a specific storable content hash |

### 9.2 Resolution and storage

| Topic | Contract |
| --- | --- |
| Namespace precedence | An explicit retrieval namespace wins; otherwise use the marker namespace; otherwise use configured default. |
| Store precedence | Explicit option/SDK value, environment/configuration, then platform default. |
| Environment | `TOKENFOLD_RETRIEVAL_STORE_PATH` and `TOKENFOLD_RETRIEVAL_NAMESPACE`. |
| Default store | `$XDG_DATA_HOME/tokenfold/retrieve`; fallback `<home>/.local/share/tokenfold/retrieve`. |
| Result | Success returns the exact stored bytes. Missing and expired references are distinguishable typed outcomes and CLI exit `8`. |
| Secret safety | Secret-shaped originals MUST NOT be persisted. Refusal leaves source content inline. |

## 10. Receipt contract

### 10.1 Required top-level areas

| Area | Required fields / behavior |
| --- | --- |
| Schema | Stable `schema_version`; insertion order remains deterministic. |
| Operation | `status`, request identifier where available, input format, output encoding, task scope/preset. |
| Metrics | Original, final, and saved tokens; savings ratio and percentage; resolved estimator metadata. |
| Budget | Status, target, protected floor, achieved tokens. |
| Transforms | Ordered per-transform ID, version, before/after/saved tokens, elapsed time, status, skipped reason, and typed warnings. |
| Pruning | Requested/applied status, candidate/retained/pruned counts, preserve paths, preview state, and evidence-reference count. |
| Encoding | Codec/version, round-trip verification, tokens before/after, and signed delta. |
| Retrieval | Store namespace, hash algorithm, marker count, TTL, persisted/skipped bytes; no fabricated single batch hash. |
| Warnings | Stable code, severity, optional transform ID, and message. |

### 10.2 Representative receipt

```json
{
  "schema_version": "2",
  "status": "compressed",
  "input_format": "json",
  "output_encoding": "toon",
  "preset": "balanced",
  "original_tokens": 5210,
  "compressed_tokens": 3412,
  "saved_tokens": 1798,
  "budget": {
    "status": "met",
    "target_tokens": 3500,
    "protected_floor": 410,
    "achieved_tokens": 3412
  },
  "encoding": {
    "codec": "toon",
    "version": "4.1.1",
    "roundtrip_verified": true,
    "tokens_before": 3540,
    "tokens_after": 3412,
    "token_delta": -128,
    "warnings": []
  },
  "pruning": {
    "requested": true,
    "applied": true,
    "preview": false,
    "retained_items": 23,
    "pruned_items": 42,
    "evidence_refs": 42,
    "preserve_paths": ["data.critical"]
  },
  "transforms": [
    {
      "id": "json_field_fold",
      "version": "1.0.0",
      "status": "applied",
      "tokens_before": 5210,
      "tokens_after": 3540,
      "saved_tokens": 1670,
      "skipped_reason": null,
      "warnings": []
    }
  ],
  "warnings": []
}
```

The example illustrates shape, not a promise that these measured values coexist for a real input.

## 11. SDK contract

### 11.1 Cross-language concepts

| Concept | Rust/Core | Python | TypeScript |
| --- | --- | --- | --- |
| Preset | Typed enum | `Preset` enum | String union |
| Output encoding | Typed enum | `Encoding` enum | String union |
| Pruning | Optional typed policy | Optional `PruningPolicy` | Optional nested `PruningPolicy` |
| Payload | `Vec<u8>` | `bytes` | `Uint8Array` |
| Receipt | Typed report | Typed `CompressionReceipt` | Typed `CompressionReceipt` |
| Hard target | Typed adapter option/error | `BudgetUnmetError` containing receipt | Rejected `BudgetUnmetError` containing receipt |
| Retrieval miss | Typed outcome/error | `RetrievalError` with missing/expired reason | Typed process/API error with missing/expired reason |

### 11.2 Python surface

| API | Signature contract | Result |
| --- | --- | --- |
| `compress` | `compress(data, *, format=AUTO, preset=BALANCED, target_tokens=None, require_target=False, encoding=None, pruning=None)` | `CompressionResult` |
| `inspect` | Same policy inputs as `compress`; always side-effect free | `CompressionReceipt` |
| `decode` | `decode(data, *, from_format="auto")` | `bytes` |
| `retrieve` | `retrieve(reference, *, retrieval_store=None, namespace=None)` | `bytes` |

| Python type | Contract |
| --- | --- |
| `CompressionResult.payload` | `bytes`; primary binary-safe payload |
| `CompressionResult.text` | Strict UTF-8 property; raises `UnicodeDecodeError` for non-UTF-8 output |
| `PruningPolicy.keep_ratio` | Optional validated float in `(0, 1]` |
| `PruningPolicy.preserve_paths` | Immutable/default-empty sequence of dotted paths |
| `PruningPolicy.retrieval_store` | Optional path |
| `PruningPolicy.retrieval_namespace` | Optional namespace |
| Receipt properties | Typed fields; MUST NOT collapse warnings or nested reports into untyped dictionaries as the primary API |

Callers that deliberately accept invalid UTF-8 can explicitly use `result.payload.decode("utf-8", errors="replace")`; Tokenfold MUST NOT do so implicitly.

### 11.3 TypeScript surface

| API | Signature contract | Result |
| --- | --- | --- |
| `compress` | `compress(data, { format, preset, targetTokens, requireTarget, encoding, pruning })` | `Promise<CompressionResult>` |
| `inspect` | Same policy object; no store writes | `Promise<CompressionReceipt>` |
| `decode` | `decode(data, { from })` | `Promise<Uint8Array>` |
| `retrieve` | `retrieve(reference, { retrievalStore, namespace })` | `Promise<Uint8Array>` |

| TypeScript type | Contract |
| --- | --- |
| `CompressionResult.payload` | `Uint8Array` |
| Text convenience | Strict UTF-8 decoding; malformed sequences MUST throw rather than replace silently |
| `PruningPolicy` | Nested object containing optional `keepRatio`, `preservePaths`, `retrievalStore`, and `retrievalNamespace` |
| Receipt | Preserve typed warning codes/severities, all transform status variants, skipped reasons, retrieval metadata, and schema version |

### 11.4 Core and adapter boundary

| Responsibility | Core | CLI/SDK adapter |
| --- | --- | --- |
| Compression, pruning selection, safety gates, recounting | Yes | No duplication |
| TOON codec and verification | Shared implementation reachable from every adapter | Argument/type mapping only |
| Stream/file routing | No | Yes |
| CLI exit codes | No | CLI only |
| `require_target` enforcement | Returns enough typed budget information; MAY expose a typed helper | Suppresses payload/raises typed error according to surface |
| Receipt schema | Canonical definition | Thin serialization/binding |

## 12. Breaking-change map

| Current interface | Replacement | Migration rule |
| --- | --- | --- |
| `--mode` | `--preset` | Rename; no compatibility alias |
| `--lossy heuristic` | `--prune` | Strategy argument removed while only one implementation exists |
| `--lossy-ratio` | `--keep-ratio` | Rename to describe the value's direction |
| `--lossy-preserve` | `--preserve` | Rename; dotted-path semantics retained |
| `--retrieve-namespace` | `--retrieval-namespace` | Rename consistently with retrieval store terminology |
| `compress --dry-run` | `inspect` | Remove duplicated preview path |
| `--json` | `--receipt-format json` | Remove input/output ambiguity |
| `--unsafe-disable-redaction` | Removed | Security invariant becomes non-disableable publicly |
| Public `--enable`, `--disable`, `--experimental` | Removed from normal surface | Keep only hidden research controls if needed |
| No output codec | `--encoding json\|toon` | TOON explicit and generic-JSON-only |
| No decode command | `decode` | Self-describing inverse path |
| Soft target only | `--require-target` | Hard target with exit `7` and no payload |

## 13. Acceptance criteria

| Area | Required regression test |
| --- | --- |
| Default CLI | `compress` with no policy flags resolves balanced and emits payload only on stdout. |
| Stream purity | Piped payload contains no receipt/diagnostic bytes; inspect emits only its receipt. |
| Receipt redirection | `inspect --receipt-file` leaves stdout empty; `compress --receipt-file` leaves stderr free of the routine receipt. |
| Hard target | Met target emits payload and exits `0`; unmet target emits no payload/file, emits receipt, and exits `7`. |
| Soft target | Unmet target emits best-effort payload, exits `0`, and reports the correct budget status. |
| Pruning dependencies | Invalid option combinations fail before store writes. |
| Pruning fail-closed | Store failure leaves every candidate inline and never emits a dangling live reference. |
| Pruning projection | Inspect performs no writes and labels projected references non-retrievable. |
| Pruning economics | Marker-costly items remain inline; exact final output is never worse than the non-pruned branch. |
| Preset reversibility | Every preset transform preserves the sanitized semantic value through `decode`; schema truncation, irreversible log compaction, and diff compaction are absent. |
| JSON decode | Dictionary then column inverses reproduce the original JSON value. |
| Text decode | Log-fold inverse reproduces exact original bytes. |
| Sentinel collision | Real data resembling each reserved frame is not corrupted. |
| TOON | Explicit TOON emits TOON even when larger, reports signed delta, and hard-fails without output on round-trip mismatch. |
| Provider guard | TOON and pruning are rejected for OpenAI/Anthropic envelopes. |
| Retrieval references | Raw hash, legacy marker, serialized marker, and SDK object resolve identically. |
| Retrieval outcomes | Missing/expired returns typed outcome and CLI exit `8`; malformed reference exits `2`. |
| UTF-8 convenience | Strict text helpers reject invalid UTF-8; raw payload bytes remain accessible. |
| Cross-surface parity | CLI, Python, TypeScript, MCP, proxy, and Rust serialize the same canonical receipt fields. |

## 14. Explicitly deferred work

| Deferred item | Reason / trigger |
| --- | --- |
| Automatic `best` encoding | Variable output syntax complicates consumers; add only with an explicit negotiation contract. |
| Additional pruning rankers | Do not add `--ranker` until a second production implementation exists. |
| Full JSONPath preservation | Dotted paths cover the current use case without a parser/dependency. |
| Receipt-guided decoding | Self-describing output is safer than reliance on external, possibly stale metadata. |
| Cross-platform atomic overwrite | Requires explicit overwrite semantics and Windows/Unix durability tests; direct byte-safe `--output` remains supported. |
| Restore original JSON formatting | Compression preserves JSON value, not insignificant whitespace or number spelling. |
