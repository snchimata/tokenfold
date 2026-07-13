# tokenfold

Token-aware compression for LLM payloads — a Rust CLI (and core library) that
shrinks JSON tool-call payloads, command output, and diffs before they hit a
model, without silently corrupting them. See `plan.md`, `roadmap.md`,
`interfaces.md`, and `engineering.md` for the full spec, contracts, and
build/test/release process.

> Personal/internal tool first (see `plan.md` § Identity decision). This
> README is a working quickstart, not marketing copy.

## Install (from source)

No prebuilt binaries are published yet (Phase 4, in progress — see
`roadmap.md`).

```bash
git clone https://github.com/snchimata/tokenfold
cd tokenfold
cargo build --release --locked -p tokenfold-cli
# binary at target/release/tokenfold (target/release/tokenfold.exe on Windows)
```

Requires the Rust toolchain pinned in `rust-toolchain.toml` (`rustup show`
installs it automatically).

## Quickstart

Preview savings on the bundled example payload, no target set:

```bash
tokenfold inspect examples/openai_payload.json --format openai
```

Compress it to a token budget (payload on stdout, human report on stderr —
safe to pipe):

```bash
tokenfold compress examples/openai_payload.json --format openai --target-tokens 250
```

Wrap an arbitrary command and compress its output:

```bash
tokenfold wrap -- git diff
```

`secret_redaction` always runs first and cannot be disabled via `--disable`.
In v0.1, lossless JSON transforms (`json_minify`, `schema_compaction`) are
default-enabled; lossy transforms (`log_compaction`, `diff_compaction`) stay
behind `--experimental` until their fidelity gate promotes them (see
`roadmap.md` F-012/F-013) — so plain-text/diff input only sees redaction by
default today.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo bench --workspace --locked   # regression-gated by crates/tokenfold-core/benches/THRESHOLDS.toml
cargo audit
cargo deny check advisories bans licenses sources
python eval/run_fidelity.py --gate --profile smoke-first-consumer
```

See `engineering.md` for the full testing strategy, CI/CD pipeline, risk
register, and contributing guide (including the prototype-then-port workflow
for lossy transforms).
