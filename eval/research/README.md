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
  python -m pip install -e "eval[exact]"
  python eval/research/toon_benchmark.py --require-exact \
    --manifest eval/research/toon_corpus/manifest.json
  ```

  The harness uses the local `@toon-format/cli@4.1.1` research dependency, falling back to `npx`
  when it is absent. It does not add TOON to a served path or require a model.
- Learned pruning experiments remain in `eval/learned/`; production integration stays blocked on
  a completed `eval/tasks/v04/HUMAN_AUDIT.md` and evaluation provenance review.

These small tools establish evidence cheaply; add embeddings, provider APIs, or a served selector
only when the deterministic baselines show a measurable gap they can close.
