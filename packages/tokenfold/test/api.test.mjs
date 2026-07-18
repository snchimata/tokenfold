import assert from "node:assert/strict";
import test from "node:test";
import {
  TokenFoldProcessError,
  binaryPath,
  compress,
  inspect,
  run,
} from "../dist/index.js";

const testBinary = process.env.TOKENFOLD_TEST_BINARY;
if (!testBinary) throw new Error("TOKENFOLD_TEST_BINARY must point to a tokenfold 0.3.2 binary");
process.env.TOKENFOLD_BINARY_PATH = testBinary;

test("resolves an explicit binary and reports its version", async () => {
  assert.equal(binaryPath(), testBinary);
  const result = await run(["--version"]);
  assert.equal(result.exitCode, 0);
  assert.match(Buffer.from(result.stdout).toString(), /^tokenfold 0\.3\.2/);
});

test("compress returns bytes and a canonical report", async () => {
  const input = Buffer.from('[{"status":"ok"},{"status":"ok"},{"status":"ok"},{"status":"ok"}]');
  const result = await compress(input, { format: "json", mode: "balanced" });
  assert.equal(result.report.schema_version, "1.0");
  assert.ok(result.payload.byteLength > 0);
  assert.equal(result.report.ledger, null);
});

test("compression options are forwarded without shell interpolation", async () => {
  const input = '{\n  "items": [1, 2, 3]\n}';
  const result = await compress(input, {
    disable: ["json_minify"],
    experimental: true,
    format: "json",
    mode: "aggressive",
    targetTokens: 1,
    taskScope: "debugging",
  });
  assert.equal(result.report.mode, "aggressive");
  assert.equal(result.report.task_scope, "debugging");
  assert.equal(result.report.budget?.target_tokens, 1);
  const jsonMinify = result.report.transforms.find(({ id }) => id === "json_minify");
  assert.notEqual(jsonMinify?.status, "applied");
});

test("inspect preserves arbitrary input bytes", async () => {
  const input = Uint8Array.from([0, 255, 1, 2, 3]);
  const result = await inspect(input, { format: "text" });
  assert.deepEqual(result.payload, input);
  assert.equal(result.report.schema_version, "1.0");
});

test("spawn failures use a stable error code", async () => {
  process.env.TOKENFOLD_BINARY_PATH = "definitely-not-a-tokenfold-binary";
  await assert.rejects(run(["--version"]), (error) => {
    assert.ok(error instanceof TokenFoldProcessError);
    assert.equal(error.code, "spawn_failed");
    return true;
  });
  process.env.TOKENFOLD_BINARY_PATH = testBinary;
});

test("a missing platform package uses the binary_not_found code", (context) => {
  delete process.env.TOKENFOLD_BINARY_PATH;
  try {
    binaryPath();
    context.skip("the native package for this platform is installed");
  } catch (error) {
    assert.ok(error instanceof TokenFoldProcessError);
    assert.equal(error.code, "binary_not_found");
  } finally {
    process.env.TOKENFOLD_BINARY_PATH = testBinary;
  }
});

test("low-level run returns non-zero status while high-level calls throw", async () => {
  const lowLevel = await run(["not-a-command"]);
  assert.notEqual(lowLevel.exitCode, 0);
  await assert.rejects(compress("payload", { targetTokens: -1 }), (error) => {
    assert.ok(error instanceof TokenFoldProcessError);
    assert.equal(error.code, "tokenfold_exit");
    return true;
  });
});

test("low-level run accepts string input, cwd, and deleted environment keys", async () => {
  const result = await run(["inspect", "--json", "--format", "text"], {
    cwd: process.cwd(),
    env: { TOKENFOLD_TEST_UNSET: undefined },
    stdin: "plain text",
  });
  assert.equal(result.exitCode, 0);
  assert.equal(JSON.parse(Buffer.from(result.stdout).toString()).schema_version, "1.0");
});

test("abort signals stop a programmatic call", async () => {
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(run(["--version"], { signal: controller.signal }), (error) => {
    assert.ok(error instanceof TokenFoldProcessError);
    assert.equal(error.code, "spawn_failed");
    return true;
  });
});

test("high-level calls forward abort signals", async () => {
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(compress("payload", { signal: controller.signal }), (error) => {
    assert.ok(error instanceof TokenFoldProcessError);
    assert.equal(error.code, "spawn_failed");
    return true;
  });
});
