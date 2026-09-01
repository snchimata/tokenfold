import assert from "node:assert/strict";
import { mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import {
  TokenFoldProcessError,
  binaryPath,
  compress,
  inspect,
  retrieve,
  run,
} from "../dist/index.js";

const testBinary = process.env.TOKENFOLD_TEST_BINARY;
if (!testBinary) throw new Error("TOKENFOLD_TEST_BINARY must point to a tokenfold 0.4.1 binary");
process.env.TOKENFOLD_BINARY_PATH = testBinary;

// A heterogeneous incident feed: lossless folding has a ceiling on it, so lossy pruning
// actually drops items here. Shared with examples/lossy_pruning.py and the Python binding tests.
const INCIDENT_FEED = path.join(import.meta.dirname, "..", "..", "..", "examples", "incident_feed.json");

/**
 * Every lossy run persists dropped items, so each test gets its own throwaway retrieval store
 * and never touches the one a real run uses. Returns { configPath, storePath }.
 */
function throwawayStore() {
  const dir = mkdtempSync(path.join(tmpdir(), "tokenfold-lossy-"));
  const storePath = path.join(dir, "store");
  const configPath = path.join(dir, "tokenfold.toml");
  writeFileSync(
    configPath,
    `[retrieval]
backend = "filesystem"
store_path = "${storePath.replaceAll("\\", "/")}"
`,
  );
  return { configPath, storePath };
}

function markerHashes(payload) {
  const hashes = [];
  const walk = (node) => {
    if (Array.isArray(node)) node.forEach(walk);
    else if (node && typeof node === "object") {
      if (node.$tf_ref?.hash) hashes.push(node.$tf_ref.hash);
      else Object.values(node).forEach(walk);
    }
  };
  walk(JSON.parse(Buffer.from(payload).toString("utf8")));
  return hashes;
}

test("resolves an explicit binary and reports its version", async () => {
  assert.equal(binaryPath(), testBinary);
  const result = await run(["--version"]);
  assert.equal(result.exitCode, 0);
  assert.match(Buffer.from(result.stdout).toString(), /^tokenfold 0\.4\.1/);
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

test("lossy pruning beats lossless and the planted incident survives", async () => {
  const { configPath } = throwawayStore();
  const input = await readFile(INCIDENT_FEED);
  const lossless = await compress(input, { configPath, format: "json" });
  const lossy = await compress(input, {
    configPath,
    format: "json",
    lossy: "heuristic",
    lossyRatio: 0.35,
  });

  assert.ok(
    lossy.report.saved_tokens > lossless.report.saved_tokens,
    "lossy pruning must save strictly more than the lossless pipeline on this fixture -- if " +
      "this regresses, the fixture no longer exercises json_prune at all",
  );
  assert.equal(lossy.report.transforms.find(({ id }) => id === "json_prune")?.status, "applied");
  assert.ok((lossy.report.retrieval?.marker_count ?? 0) > 0);

  const events = JSON.parse(Buffer.from(lossy.payload).toString("utf8")).events;
  const kept = events.filter((event) => !event.$tf_ref);
  const dropped = events.filter((event) => event.$tf_ref);
  assert.ok(dropped.length > 0, "this fixture must exercise pruning");
  assert.ok(
    kept.some((event) => event.status_code === 503),
    "the planted incident must survive -- typed failure signal outranks position in the ranking",
  );
  // A marker missing either field is unrecoverable data loss, not "lossy but recoverable".
  for (const { $tf_ref } of dropped) {
    assert.equal(typeof $tf_ref.hash, "string");
    assert.equal(typeof $tf_ref.namespace, "string");
  }
});

test("retrieve round-trips a dropped item by every supported reference form", async () => {
  const { configPath } = throwawayStore();
  const input = await readFile(INCIDENT_FEED);
  const lossy = await compress(input, {
    configPath,
    format: "json",
    lossy: "heuristic",
    lossyRatio: 0.35,
  });
  const { $tf_ref: marker } = JSON.parse(Buffer.from(lossy.payload).toString("utf8")).events.find(
    (event) => event.$tf_ref,
  );

  const byHash = await retrieve(marker.hash, { configPath, namespace: marker.namespace });
  const restored = JSON.parse(Buffer.from(byHash).toString("utf8"));
  const originals = JSON.parse(Buffer.from(input).toString("utf8")).events;
  assert.deepEqual(
    originals.find((event) => event.seq === restored.seq),
    restored,
    "retrieved bytes must be one of the original, untouched events",
  );

  // The text-marker form carries its own namespace, so it needs no namespace option.
  const byMarker = await retrieve(
    `[tokenfold:retrieve hash=${marker.hash} namespace=${marker.namespace}]`,
    { configPath },
  );
  assert.deepEqual(byMarker, byHash);

  const byJsonMarker = await retrieve(JSON.stringify({ $tf_ref: marker }), { configPath });
  assert.deepEqual(byJsonMarker, byHash);
});

test("retrieve rejects an unknown hash, a malformed reference, and a report path", async () => {
  const { configPath } = throwawayStore();
  const failsWith = async (reference) => {
    await assert.rejects(
      () => retrieve(reference, { configPath }),
      (error) => {
        assert.ok(error instanceof TokenFoldProcessError);
        assert.equal(error.code, "tokenfold_exit");
        return true;
      },
    );
  };
  await failsWith("0".repeat(64));
  await failsWith("not-a-hash");
  // A CompressionReport carries no per-entry hash, so the CLI refuses it rather than guessing.
  // retrieve()'s doc comment promises exactly this, so pin it.
  const { report } = await compress('{"a":1}', { configPath, format: "json" });
  const reportPath = path.join(path.dirname(configPath), "report.json");
  writeFileSync(reportPath, JSON.stringify(report));
  await failsWith(reportPath);
});

test("lossyPreserve protects a named array, including a top-level one", async () => {
  const { configPath } = throwawayStore();
  const options = { configPath, format: "json", lossy: "heuristic", lossyRatio: 0.35 };
  const feed = await readFile(INCIDENT_FEED);
  assert.deepEqual(
    markerHashes((await compress(feed, { ...options, lossyPreserve: ["events"] })).payload),
    [],
  );

  // An eligible ROOT array has path "", which the nearest-eligible-ancestor rule once failed to
  // match at all (cli.rs::lossy_preserve_protects_a_top_level_array). Reuses the feed's own rows,
  // which are large enough to be worth replacing with a marker, as a bare top-level array; the
  // nested `users` array is what `lossyPreserve` names from *inside* the eligible root array.
  const rootArray = JSON.stringify(
    JSON.parse(Buffer.from(feed).toString("utf8")).events.map((event) => ({
      ...event,
      users: [1, 2, 3],
    })),
  );
  assert.ok(
    markerHashes((await compress(rootArray, options)).payload).length > 0,
    "control: a root array must be prunable without lossyPreserve",
  );
  assert.deepEqual(
    markerHashes((await compress(rootArray, { ...options, lossyPreserve: ["users"] })).payload),
    [],
    "naming a path inside the root array must protect its nearest enclosing eligible array",
  );
});

test("a lossy inspect previews json_prune and writes nothing", async () => {
  const { configPath, storePath } = throwawayStore();
  const options = { configPath, format: "json", lossy: "heuristic", lossyRatio: 0.35 };
  const feed = await readFile(INCIDENT_FEED);
  const preview = await inspect(feed, options);

  assert.deepEqual(
    preview.payload,
    Uint8Array.from(feed),
    "inspect hands the input back unchanged",
  );
  assert.ok(
    preview.report.transforms.some(({ id }) => id === "json_prune"),
    "a lossy preview must project the json_prune stage, not silently omit it",
  );
  assert.ok(preview.report.saved_tokens > 0);
  assert.throws(() => readdirSync(storePath), "a preview must not create the retrieval store");

  await compress(feed, options);
  assert.ok(readdirSync(storePath).length > 0, "the real run must still persist");
});

test("a lossy run that prunes nothing is never worse than the lossless run", async () => {
  // Identical short rows: nothing here is worth replacing with a marker, but json_field_fold and
  // json_value_dict have plenty to do -- and lossy defers those rather than disabling them.
  const { configPath } = throwawayStore();
  const uniform = JSON.stringify({
    events: Array.from({ length: 15 }, (_, seq) => ({
      seq,
      retries: 0,
      note: "queue drain cycle completed normally with no backpressure observed",
    })),
  });
  const lossless = await compress(uniform, { configPath, format: "json" });
  const lossy = await compress(uniform, {
    configPath,
    format: "json",
    lossy: "heuristic",
    lossyRatio: 0.25,
  });
  assert.deepEqual(lossy.payload, lossless.payload);
});

test("lossy on an unsupported format is a no-op that persists nothing", async () => {
  const { configPath, storePath } = throwawayStore();
  const payload = JSON.stringify({
    model: "gpt-4o",
    messages: [{ role: "user", content: "summarize the incident feed ".repeat(40) }],
  });
  const result = await compress(payload, {
    configPath,
    format: "openai",
    lossy: "heuristic",
    lossyRatio: 0.35,
  });
  // Reported as an explicit skip rather than silently omitted, so the receipt still shows that
  // lossy was asked for and why it did not run.
  const prune = result.report.transforms.find(({ id }) => id === "json_prune");
  assert.equal(prune?.status, "skipped");
  assert.equal(prune?.skipped_reason, "not_applicable_to_format");
  assert.ok(!Buffer.from(result.payload).includes("$tf_ref"));
  // lossy implies a durable receipt only where the lossy stage can actually run.
  assert.throws(
    () => readdirSync(storePath),
    "nothing may persist when the lossy stage cannot run",
  );
});

test("lossy options are validated by the CLI and never shell-interpolated", async () => {
  const { configPath } = throwawayStore();
  const feed = await readFile(INCIDENT_FEED);

  // lossyRatio without lossy used to be dropped on the floor. The CLI's own `requires = "lossy"`
  // must surface instead, for compress and for the inspect preview alike.
  for (const call of [compress, inspect]) {
    await assert.rejects(
      () => call(feed, { configPath, format: "json", lossyRatio: 0.35 }),
      (error) => {
        assert.equal(error.code, "tokenfold_exit");
        assert.match(Buffer.from(error.stderr).toString(), /--lossy/);
        return true;
      },
    );
  }

  // A preserve path is one argv element, never a shell word.
  const injected = await compress(feed, {
    configPath,
    format: "json",
    lossy: "heuristic",
    lossyRatio: 0.35,
    lossyPreserve: ['events" && echo pwned'],
  });
  assert.ok(!Buffer.from(injected.payload).includes("pwned"));
  assert.ok(
    markerHashes(injected.payload).length > 0,
    "a bogus preserve path must not protect anything",
  );
});

test("a lower lossyRatio keeps fewer items", async () => {
  const { configPath } = throwawayStore();
  const feed = await readFile(INCIDENT_FEED);
  const markerCount = async (lossyRatio) =>
    markerHashes(
      (await compress(feed, { configPath, format: "json", lossy: "heuristic", lossyRatio }))
        .payload,
    ).length;
  assert.ok((await markerCount(0.05)) > (await markerCount(0.35)));
});
