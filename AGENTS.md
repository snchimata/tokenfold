# Repository Guidelines

These instructions apply to the entire repository. More specific `AGENTS.md` files may override them for their directory.

## Working principles

- Make the smallest correct change and reuse existing patterns before adding abstractions or dependencies.
- Read the implementation, its callers, and nearby tests before editing behavior.
- Keep changes focused. Do not modify unrelated work already present in the working tree.
- Treat public CLI output, serialized reports, configuration, and language bindings as compatibility-sensitive interfaces.
- Never commit secrets, local configuration, generated build output, or benchmark/evaluation caches.

## Repository map

- `crates/tokenfold-core`: compression pipeline, policies, transforms, reports, and token accounting.
- `crates/tokenfold-cli`: command-line interface and end-to-end CLI behavior.
- `crates/tokenfold-proxy`: HTTP proxy and forwarding behavior.
- `crates/tokenfold-py`: PyO3 Python bindings.
- `crates/tokenfold-{adapters,admin,image,learn,output,rag}`: optional integration and feature crates.
- `packages/tokenfold`: TypeScript API and npm CLI wrapper.
- `tests/`: shared fixtures and byte-exact golden cases.
- `python-tests/`: tests for the built Python extension.
- `eval/`: Python fidelity harness and transform evaluation.
- Design notes (architecture, interfaces, testing policy, roadmap decisions) are kept untracked and are not part of a clone; treat the code, tests, and `CHANGELOG.md` as the authoritative record.

## Implementation conventions

- Use the pinned Rust toolchain from `rust-toolchain.toml` and the workspace's Rust 2024 edition.
- Put shared compression behavior in `tokenfold-core`; keep CLI, proxy, and bindings as thin adapters where practical.
- Preserve JSON insertion order and stable report fields. Do not change schema versions, output formats, exit codes, or defaults accidentally.
- Preserve fail-closed safety behavior, redaction guarantees, protected content, and token-budget accounting.
- Keep lossy or unvalidated transforms behind the existing experimental/fidelity gates.
- Add tests beside the affected behavior. Prefer focused regression tests over broad new test infrastructure.
- Golden outputs are versioned contracts. Change them only for an intentional behavior change, explain why, and keep `tests/golden/MANIFEST.toml` synchronized.
- Avoid new dependencies unless existing workspace crates or the standard library cannot reasonably solve the problem.

## Validation

Run the narrowest relevant checks while iterating, then validate every affected surface before finishing.

For Rust changes:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

For npm/TypeScript changes:

```sh
cd packages/tokenfold
npm ci
npm test
```

For Python binding changes, build the extension before running its tests:

```sh
maturin build --release -m crates/tokenfold-py/Cargo.toml
pytest python-tests
```

For changes to compression quality, lossy transforms, or promotion gates:

```sh
python eval/run_fidelity.py --gate --profile smoke-first-consumer
```

If a check cannot run in the current environment, report exactly which check was skipped and why.

## Branches and pull requests

- Never push to `main`. It is protected: every change lands through a pull request with green CI, including release-preparation commits. See [CONTRIBUTING.md](CONTRIBUTING.md).
- Branch from current `main` using `feat/`, `fix/`, `chore/`, `docs/`, or `release/` prefixes, and keep the branch short-lived.
- `main` is the only long-lived branch. Do not create environment branches (`develop`, `preprod`, `production`); releases are selected by `vX.Y.Z` tags, not by branches.
- Released tags are immutable. Never delete or force-move a `v*` tag.

## Documentation and releases

- Update user-facing documentation when commands, configuration, output, or supported integrations change.
- Use `.github/workflows/`, the code, and tests as the source of truth for validation and release behavior when planning documents disagree with shipped behavior.
- Do not publish packages, create releases, push tags, or regenerate release artifacts unless explicitly requested.
