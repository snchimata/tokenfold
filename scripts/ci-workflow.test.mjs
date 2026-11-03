import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");

function job(id) {
  const jobs = [...workflow.matchAll(/^  ([a-z][a-z0-9-]*):\s*$/gm)];
  const index = jobs.findIndex(match => match[1] === id);
  assert.notEqual(index, -1, `missing job: ${id}`);
  return workflow.slice(jobs[index].index, jobs[index + 1]?.index);
}

test("one Linux release build feeds both Node versions and evaluation", () => {
  assert.equal((workflow.match(/run: cargo build --release --locked -p tokenfold-cli/g) ?? []).length, 1);
  const producer = job("linux-binaries");
  assert.match(producer, /runs-on: ubuntu-latest/);
  assert.match(producer, /cargo build --release --locked -p tokenfold-cli -p tokenfold-proxy/);
  assert.match(producer, /actions\/upload-artifact@v7/);
  assert.match(producer, /name: linux-binaries\n\s+path:/);
  assert.match(producer, /if-no-files-found: error/);
  assert.match(producer, /target\/release\/tokenfold\n\s+target\/release\/tokenfold-proxy/);
  for (const id of ["node-api", "eval-harness"]) {
    const consumer = job(id);
    assert.match(consumer, /needs: linux-binaries/);
    assert.match(consumer, /runs-on: ubuntu-latest/);
    assert.match(consumer, /actions\/download-artifact@v8/);
    assert.match(consumer, /name: linux-binaries\n\s+path: target\/release/);
    assert.match(consumer, /chmod \+x target\/release\/tokenfold target\/release\/tokenfold-proxy/);
    assert.doesNotMatch(consumer, /cargo build|rust-toolchain/);
  }
  assert.match(job("node-api"), /node: \[22, 24\]/);
  assert.match(job("node-api"), /TOKENFOLD_TEST_BINARY: \$\{\{ github.workspace \}\}\/target\/release\/tokenfold/);
  assert.match(job("eval-harness"), /TOKENFOLD_PROXY_BIN: \$\{\{ github.workspace \}\}\/target\/release\/tokenfold-proxy/);
});

test("only superseded PR CI runs share a cancellation group", () => {
  // PR numbers and push-run IDs must stay disjoint even if their numeric values match.
  const group = (event, pr, run) => workflow.match(/^  group: (.+)$/m)[1]
    .replace("${{ github.workflow }}", "CI")
    .replace("${{ github.event_name }}", event)
    .replace("${{ github.event.pull_request.number || github.run_id }}", String(pr || run));
  assert.notEqual(group("pull_request", 7, 101), group("push", null, 7));
  assert.equal(group("pull_request", 7, 101), group("pull_request", 7, 102));
  assert.notEqual(group("push", null, 7), group("push", null, 8));
  assert.match(workflow, /group: ci-\$\{\{ github.workflow \}\}-\$\{\{ github.event_name \}\}-\$\{\{ github.event.pull_request.number \|\| github.run_id \}\}/);
  assert.match(workflow, /cancel-in-progress: \$\{\{ github.event_name == 'pull_request' \}\}/);
  assert.doesNotMatch(workflow, /paths-ignore:|if:.*cache-hit/);
});

test("required consumers fail on an unsuccessful producer instead of silently skipping", () => {
  for (const id of ["node-api", "eval-harness"]) {
    const consumer = job(id);
    assert.match(consumer, /if: \$\{\{ !cancelled\(\) \}\}/);
    assert.match(consumer, /LINUX_BUILD_RESULT: \$\{\{ needs.linux-binaries.result \}\}/);
    assert.match(consumer, /run: test "\$LINUX_BUILD_RESULT" = success/);
    assert.ok(consumer.indexOf("Require successful Linux build") < consumer.indexOf("actions/checkout"));
  }
});

test("Rust caches follow toolchain setup and only trusted main pushes save them", () => {
  for (const id of ["lint", "test", "windows-smoke", "linux-binaries", "coverage", "golden-cross-platform", "bench-smoke"]) {
    const block = job(id);
    assert.ok(block.indexOf("rust-toolchain@stable") < block.indexOf("Swatinem/rust-cache@v2"));
    assert.ok(block.indexOf("Swatinem/rust-cache@v2") < block.indexOf("- run: cargo"));
    assert.match(block, /key: \$\{\{ runner.os \}\}-\$\{\{ runner.arch \}\}/);
    assert.match(block, /save-if: \$\{\{ github.event_name == 'push' && github.ref == 'refs\/heads\/main' \}\}/);
  }
});

test("required test, platform, binding, safety and fidelity lanes remain", () => {
  for (const id of ["lint", "test", "windows-smoke", "python-wheel", "select-runtime-contracts", "python-abi3-smoke", "node-api", "coverage", "golden-cross-platform", "security", "bench-smoke", "fidelity-smoke", "eval-harness"]) job(id);
  assert.match(job("lint"), /cargo fmt --all --check/);
  assert.match(job("lint"), /python3 scripts\/check-release-versions\.py/);
  assert.match(job("lint"), /cargo clippy --workspace --all-targets -- -D warnings/);
  assert.match(job("test"), /cargo test --workspace --locked/);
  assert.match(job("windows-smoke"), /runs-on: windows-latest/);
  assert.match(job("python-wheel"), /manylinux: auto/);
  assert.match(job("python-wheel"), /python -m pytest python-tests -q/);
  assert.match(job("python-abi3-smoke"), /python: \["3.9", "3.13"\]/);
  assert.match(job("select-runtime-contracts"), /os: \[ubuntu-latest, windows-latest\]/);
  assert.match(job("golden-cross-platform"), /os: \[macos-15, macos-15-intel, ubuntu-latest\]/);
  assert.match(job("security"), /cargo deny check advisories bans licenses sources/);
  assert.match(job("security"), /gitleaks\/gitleaks-action@v2/);
  assert.match(job("fidelity-smoke"), /python eval\/run_fidelity.py --gate --profile smoke-first-consumer/);
  assert.match(job("eval-harness"), /TOKENFOLD_LEARNED_MODULE: json/);
  assert.match(job("eval-harness"), /python eval\/run_baselines.py --gate/);
  assert.match(job("eval-harness"), /python eval\/audit_quality_sample.py --check/);
});

test("bounded hosted validation caps jobs and artifact retention without skipping checks", () => {
  for (const id of ["lint", "test", "windows-smoke", "python-wheel", "select-runtime-contracts", "python-abi3-smoke", "linux-binaries", "node-api", "coverage", "golden-cross-platform", "security", "bench-smoke", "fidelity-smoke", "eval-harness"]) assert.match(job(id), /^    timeout-minutes: 10$/m);
  assert.equal((workflow.match(/^          retention-days: 1$/gm) ?? []).length, 3);
  const security = readFileSync(new URL("../.github/workflows/security-analysis.yml", import.meta.url), "utf8");
  const codeql = security.slice(security.indexOf("  codeql:"), security.indexOf("  parser-property-stress:"));
  assert.match(codeql, /^    timeout-minutes: 10$/m);
  assert.match(codeql, /language: \[rust, javascript-typescript, python\]/);
});
