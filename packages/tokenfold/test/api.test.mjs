import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  BudgetUnmetError,
  TokenFoldProcessError,
  binaryPath,
  compress,
  decode,
  inspect,
  retrieve,
  run,
} from "../dist/index.js";

const testBinary = process.env.TOKENFOLD_TEST_BINARY;
if (!testBinary) throw new Error("TOKENFOLD_TEST_BINARY must point to a tokenfold binary");
process.env.TOKENFOLD_BINARY_PATH = testBinary;

test("resolves an explicit binary", async () => {
  assert.equal(binaryPath(), testBinary);
  assert.match(Buffer.from((await run(["--version"])).stdout).toString(), /^tokenfold /);
});

test("compress returns bytes and a v2 receipt", async () => {
  const result = await compress('{ "items": [1, 2, 3] }', {
    format: "json",
    preset: "aggressive",
  });
  assert.ok(result.payload instanceof Uint8Array);
  assert.equal(result.report.schema_version, "2.0");
  assert.equal(result.report.preset, "aggressive");
  assert.equal(result.report.output_encoding, "json");
  assert.equal(result.text, Buffer.from(result.payload).toString("utf8"));
});

test("inspect returns only the receipt", async () => {
  const receipt = await inspect(Uint8Array.from([0, 255, 1]), { format: "text" });
  assert.equal(receipt.schema_version, "2.0");
  assert.equal(receipt.format, "plain_text");
  assert.equal("payload" in receipt, false);
});

test("TOON output decodes to the same JSON value", async () => {
  const value = { users: [{ id: 1, active: true }, { id: 2, active: false }] };
  const encoded = await compress(JSON.stringify(value), { format: "json", encoding: "toon" });
  assert.equal(encoded.report.encoding?.roundtrip_verified, true);
  assert.deepEqual(JSON.parse(Buffer.from(await decode(encoded.payload, { from: "toon" })).toString()), value);
});

test("requireTarget raises BudgetUnmetError with its receipt", async () => {
  await assert.rejects(
    compress('{"protected":"content that cannot reach zero tokens"}', {
      format: "json",
      targetTokens: 1,
      requireTarget: true,
    }),
    (error) => {
      assert.ok(error instanceof BudgetUnmetError);
      assert.equal(error.receipt.schema_version, "2.0");
      assert.ok(["best_effort", "unreachable"].includes(error.receipt.budget?.status));
      return true;
    },
  );
});

test("invalid options use the stable process error", async () => {
  await assert.rejects(compress("payload", { targetTokens: -1 }), (error) => {
    assert.ok(error instanceof TokenFoldProcessError);
    assert.equal(error.code, "tokenfold_exit");
    return true;
  });
});

test("pruned evidence can be retrieved", async () => {
  const input = await readFile(path.join(import.meta.dirname, "..", "..", "..", "examples", "incident_feed.json"));
  const store = path.join(mkdtempSync(path.join(tmpdir(), "tokenfold-")), "store");
  const result = await compress(input, {
    format: "json",
    targetTokens: 50,
    pruning: { keepRatio: 0.05, retrievalStore: store, retrievalNamespace: "npm-test" },
  });
  const marker = JSON.stringify(JSON.parse(result.text)).match(/[a-f0-9]{64}/)?.[0];
  assert.ok(marker, "expected a retrieval marker");
  assert.ok((await retrieve(marker, { retrievalStore: store, namespace: "npm-test" })).byteLength > 0);
});

test("spawn and abort failures retain stable error codes", async () => {
  process.env.TOKENFOLD_BINARY_PATH = "definitely-not-a-tokenfold-binary";
  await assert.rejects(run(["--version"]), (error) => error instanceof TokenFoldProcessError && error.code === "spawn_failed");
  process.env.TOKENFOLD_BINARY_PATH = testBinary;
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(compress("payload", { signal: controller.signal }), (error) => error instanceof TokenFoldProcessError && error.code === "spawn_failed");
});
