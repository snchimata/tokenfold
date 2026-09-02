/** Minimal Node.js quickstart for Tokenfold's bytes-first v2 interface. */
import { readFile } from "node:fs/promises";

const { compress, decode, inspect } = await import("tokenfold").catch(() =>
  import(new URL("../packages/tokenfold/dist/index.js", import.meta.url).href),
);
const here = (name) => new URL(name, import.meta.url);
const show = (label, report) => console.log(
  `== ${label} ==\ntokens: ${report.original_tokens} -> ${report.compressed_tokens} ` +
  `(${report.saved_tokens} saved, ${report.savings_pct.toFixed(1)}%)`,
);

const body = await readFile(here("openai_payload.json"));
const request = await compress(body, { format: "openai", preset: "balanced" });
show("OpenAI request", request.report);
console.log(`payload: ${request.payload.length} bytes\n`);

const data = await readFile(here("api_response.json"));
show("generic JSON preview", await inspect(data, { format: "json" }));

const toonInput = Buffer.from(JSON.stringify({ users: [{ id: 1 }, { id: 2 }] }));
const encoded = await compress(toonInput, { format: "json", encoding: "toon" });
const restored = await decode(encoded.payload, { from: "toon" });
if (JSON.stringify(JSON.parse(Buffer.from(restored))) !== JSON.stringify(JSON.parse(toonInput))) {
  throw new Error("TOON round trip changed the JSON value");
}
show("explicit TOON", encoded.report);
