import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("optional CLI dependencies have complete matching lock entries", async () => {
  const manifest = JSON.parse(await readFile(new URL("../package.json", import.meta.url)));
  const lock = JSON.parse(await readFile(new URL("../package-lock.json", import.meta.url)));
  assert.deepEqual(lock.packages[""].optionalDependencies, manifest.optionalDependencies);
  for (const [name, version] of Object.entries(manifest.optionalDependencies)) {
    const entry = lock.packages[`node_modules/${name}`];
    assert.equal(entry?.version, version, `${name}: unresolved or stale optional dependency`);
    assert.equal(entry.optional, true);
    assert.match(entry.resolved, /^https:\/\/registry\.npmjs\.org\//);
    assert.match(entry.integrity, /^sha512-[A-Za-z0-9+/]+=*$/);
  }
});
