# Research harnesses

Nothing here is on a served compression path.

- `prompt_cache.py` measures whether a proposed output preserves a declared byte prefix and models
  repeated-input cost using caller-supplied prices. Use this before proposing a real cache API.
- `near_dedup.py` reports likely duplicate JSONL records using deterministic token-set Jaccard
  similarity. It never deletes or rewrites data.
- `toon_benchmark.py` compares compact JSON, Tokenfold, and the pinned official TOON CLI on
  caller-supplied JSON files. It reports `o200k_base` tokens (when `tiktoken` is installed),
  encoded bytes, median encode/decode latency, determinism, and value/round-trip checks across a
  versioned seven-case project corpus, including a flat projection where TOON should be strongest.
  Run it from the repository root:

  ```sh
  npm ci --prefix eval/research
  uv run --with tiktoken python eval/research/toon_benchmark.py --require-exact \
    --manifest eval/research/toon_corpus/manifest.json \
    --tokenfold-revision c1c2c8fc4cb7284a96a1fb52086bc9bc01541989 \
    --output eval/research/toon_results.json
  ```

  The harness uses the local `@toon-format/cli@4.1.1` research dependency, falling back to `npx`
  when it is absent. It does not add TOON to a served path or require a model.
- `provider_benchmark.py` compares Tokenfold with Headroom's default local generic-JSON API on
  the same six versioned inputs, counts both outputs with `o200k_base`, and verifies recovered
  values. Headroom remains an isolated research dependency:

  ```sh
  cargo build --release --locked -p tokenfold-cli
  uv run --python 3.12 \
    --with "headroom-ai[proxy] @ git+https://github.com/headroomlabs-ai/headroom.git@4c9c29c421224920dee682a0cb0c688c1c71e64e" \
    python eval/research/provider_benchmark.py \
    --manifest eval/research/provider_corpus/manifest.json \
    --headroom-revision 4c9c29c421224920dee682a0cb0c688c1c71e64e \
    --tokenfold-revision c1c2c8fc4cb7284a96a1fb52086bc9bc01541989 \
    --output eval/research/provider_results.json
  ```

  The checked-in report is the evidence used by the README comparison. This is a local API
  comparison, not a claim about either hosted proxy.
- Learned pruning experiments remain in `eval/learned/`; production integration stays blocked on
  a completed `eval/tasks/v04/HUMAN_AUDIT.md` and evaluation provenance review.

These small tools establish evidence cheaply; add embeddings, provider APIs, or a served selector
only when the deterministic baselines show a measurable gap they can close.
