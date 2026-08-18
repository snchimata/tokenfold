/**
 * Real-world quickstart for the `tokenfold` npm package.
 *
 * Install the package, then run this file:
 *
 *     npm install tokenfold
 *     node examples/quickstart.mjs
 *
 * From a clone, build the local package and point it at a local CLI build:
 *
 *     cargo build --release -p tokenfold-cli
 *     npm --prefix packages/tokenfold ci && npm --prefix packages/tokenfold run build
 *     TOKENFOLD_BINARY_PATH=target/release/tokenfold node examples/quickstart.mjs
 *
 * PowerShell:
 *
 *     $env:TOKENFOLD_BINARY_PATH = "target/release/tokenfold.exe"
 *     node examples/quickstart.mjs
 *
 * It shows the three things you'll actually do in an app:
 *
 *   1. compress(body, { format: "openai" }) -- compress a chat-completion request
 *      body before you send it, and read the exact token accounting off the receipt.
 *   2. compress(data, { format: "json" }) -- compress generic JSON *data* (API
 *      responses, records, logs). Losslessly folds repeated keys and values.
 *   3. inspect(...) -- dry run. Same receipt, your bytes handed back untouched, so
 *      you can decide whether compressing is worth it before you commit to it.
 *
 * Every call returns `{ payload, report }`: `payload` is bytes ready to send, and
 * `report` carries before/after tokens, which transforms ran, and any safety
 * warnings. tokenfold never silently drops content -- you get receipts.
 *
 * Everything here is lossless: the TypeScript package only ever restructures data,
 * never discards it. Opt-in lossy array pruning is CLI-only today -- see
 * `examples/lossy_pruning.py` for that. Node's standard library only, no extra deps.
 */

import { readFile } from "node:fs/promises";

// Prefer the published package; fall back to this repo's own build so a clone can run
// the example without installing anything from npm.
const { compress, inspect } = await import("tokenfold").catch(() =>
  import(new URL("../packages/tokenfold/dist/index.js", import.meta.url).href).catch(() => {
    console.error(
      "tokenfold not found. Install it (`npm install tokenfold`), or build the local\n" +
        "package first:\n" +
        "    npm --prefix packages/tokenfold ci && npm --prefix packages/tokenfold run build",
    );
    process.exit(1);
  }),
);

const here = (name) => new URL(name, import.meta.url);

const applied = (report) =>
  report.transforms.filter((t) => t.status === "applied").map((t) => t.id);

function printTokens(report) {
  console.log(
    `tokens:     ${report.original_tokens} -> ${report.compressed_tokens} ` +
      `(${report.saved_tokens} saved, ${report.savings_pct.toFixed(1)}%)`,
  );
}

// --- 1. A provider request body (chat messages + a verbose tool schema) -------------
async function compressAnOpenAiBody() {
  // The same payload the Rust and Python examples use (examples/openai_payload.json).
  const body = await readFile(here("openai_payload.json"));
  const { payload, report } = await compress(body, { format: "openai", mode: "balanced" });

  console.log("== compress (OpenAI request body) ==");
  // `best_effort` here means "no --target-tokens to confirm against", not a failure:
  // `compressed` is reserved for runs that provably met a target you asked for.
  console.log(`status:     ${report.status}`);
  printTokens(report);
  console.log(`estimator:  ${report.estimator.backend} (exact: ${report.estimator.is_exact})`);
  for (const w of report.warnings) console.log(`  warning: ${w.message}`);
  // `payload` is the compressed body -- POST it to the provider as-is:
  //   await fetch(url, { method: "POST", body: payload });
  console.log(`payload:    ${payload.length} bytes ready to send\n`);
}

// --- 2. Generic JSON data (API response, records, logs) ------------------------------
async function compressGenericJsonData() {
  const data = await readFile(here("api_response.json"));
  const { payload, report } = await compress(data, { format: "json", mode: "balanced" });

  console.log("== compress (generic JSON data) ==");
  printTokens(report);
  console.log(`transforms: ${applied(report).join(", ")}`);
  // Columnar folding plus a value dictionary, losslessly reversible -- every stage is
  // round-trip gated before it's applied.
  console.log(`payload:    ${payload.length} bytes\n`);
}

// --- 3. Dry run: what would compressing buy me? --------------------------------------
async function inspectBeforeCommitting() {
  const data = await readFile(here("api_response.json"));
  const { payload, report } = await inspect(data, { format: "json" });

  console.log("== inspect (dry run) ==");
  printTokens(report);
  console.log(`would apply: ${applied(report).join(", ")}`);
  console.log(`payload:    ${payload.length} bytes -- your input, handed back unchanged\n`);
}

await compressAnOpenAiBody();
await compressGenericJsonData();
await inspectBeforeCommitting();
