#!/usr/bin/env bash
# One CycloneDX SBOM per released binary target, so each SBOM lists only the
# crates that target's binary links. Keep TARGETS in sync with the
# cross-compile matrix in .github/workflows/release.yml (checked by
# scripts/ci-workflow.test.mjs).
set -euo pipefail

TARGETS=(
  x86_64-apple-darwin
  aarch64-apple-darwin
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
  x86_64-pc-windows-msvc
)

for target in "${TARGETS[@]}"; do
  cargo cyclonedx --format json --target "$target" --target-in-filename
done

# Fail when any target's CLI SBOM is missing.
for target in "${TARGETS[@]}"; do
  compgen -G "crates/tokenfold-cli/*${target}*.cdx.json" > /dev/null \
    || { echo "missing tokenfold-cli SBOM for ${target}" >&2; exit 1; }
done
