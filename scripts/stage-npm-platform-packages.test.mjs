import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";

const root = resolve(import.meta.dirname, "..");
const script = join(root, "scripts", "stage-npm-platform-packages.mjs");
const assetNames = [
  "tokenfold-x86_64-apple-darwin",
  "tokenfold-aarch64-apple-darwin",
  "tokenfold-x86_64-unknown-linux-musl",
  "tokenfold-aarch64-unknown-linux-musl",
  "tokenfold-x86_64-pc-windows-msvc.exe",
];

async function fixture() {
  const directory = await mkdtemp(join(tmpdir(), "tokenfold-npm-stage-"));
  const assets = join(directory, "assets");
  const output = join(directory, "output");
  await mkdir(assets);
  for (const name of assetNames) {
    const bytes = Buffer.from(`fixture:${name}`);
    const checksum = createHash("sha256").update(bytes).digest("hex");
    await writeFile(join(assets, name), bytes);
    await writeFile(join(assets, `${name}.sha256`), `${checksum}  ${name}\n`);
  }
  return { assets, directory, output };
}

test("stages five exact-version platform packages with verified checksums", async () => {
  const { assets, directory, output } = await fixture();
  try {
    execFileSync(process.execPath, [script, "0.3.2", assets, output], { cwd: root });
    const packages = [
      "cli-darwin-x64",
      "cli-darwin-arm64",
      "cli-linux-x64",
      "cli-linux-arm64",
      "cli-win32-x64",
    ];
    for (const packageDirectory of packages) {
      const manifest = JSON.parse(await readFile(join(output, packageDirectory, "package.json")));
      assert.equal(manifest.version, "0.3.2");
      assert.equal(manifest.publishConfig.provenance, true);
    }
    if (process.platform !== "win32") {
      const mode = (await stat(join(output, "cli-linux-x64", "bin", "tokenfold"))).mode;
      assert.notEqual(mode & 0o111, 0);
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("rejects a mismatched platform checksum", async () => {
  const { assets, directory, output } = await fixture();
  try {
    await writeFile(join(assets, `${assetNames[0]}.sha256`), `${"0".repeat(64)}  bad\n`);
    assert.throws(
      () => execFileSync(process.execPath, [script, "0.3.2", assets, output], {
        cwd: root,
        stdio: "pipe",
      }),
      /Command failed/,
    );
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("rejects missing arguments and version drift", () => {
  assert.throws(() => execFileSync(process.execPath, [script], { cwd: root, stdio: "pipe" }));
  assert.throws(() => execFileSync(process.execPath, [script, "9.9.9", ".", "."], {
    cwd: root,
    stdio: "pipe",
  }));
});
