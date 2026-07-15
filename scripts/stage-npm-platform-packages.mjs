import { createHash } from "node:crypto";
import { chmod, cp, mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import process from "node:process";

const [version, assetsArgument, outputArgument] = process.argv.slice(2);
if (!version || !assetsArgument || !outputArgument) {
  throw new Error("usage: node scripts/stage-npm-platform-packages.mjs VERSION ASSETS_DIR OUTPUT_DIR");
}

const cargoVersion = /version\s*=\s*"([^"]+)"/.exec(await readFile("Cargo.toml", "utf8"))?.[1];
if (cargoVersion !== version) throw new Error(`Cargo version ${cargoVersion} does not match ${version}`);

const assets = resolve(assetsArgument);
const output = resolve(outputArgument);
const license = resolve("LICENSE");
const packages = [
  ["@tokenfold/cli-darwin-x64", "darwin", "x64", "tokenfold-x86_64-apple-darwin", "tokenfold"],
  ["@tokenfold/cli-darwin-arm64", "darwin", "arm64", "tokenfold-aarch64-apple-darwin", "tokenfold"],
  ["@tokenfold/cli-linux-x64", "linux", "x64", "tokenfold-x86_64-unknown-linux-musl", "tokenfold"],
  ["@tokenfold/cli-linux-arm64", "linux", "arm64", "tokenfold-aarch64-unknown-linux-musl", "tokenfold"],
  ["@tokenfold/cli-win32-x64", "win32", "x64", "tokenfold-x86_64-pc-windows-msvc.exe", "tokenfold.exe"],
];

for (const [name, os, cpu, sourceName, binaryName] of packages) {
  const directory = join(output, name.replace("@tokenfold/", ""));
  const source = join(assets, sourceName);
  const checksumSource = join(assets, `${sourceName}.sha256`);
  const expected = (await readFile(checksumSource, "utf8")).trim().split(/\s+/)[0]?.toLowerCase();
  const actual = createHash("sha256").update(await readFile(source)).digest("hex");
  if (expected !== actual) throw new Error(`SHA-256 mismatch for ${sourceName}`);

  await mkdir(join(directory, "bin"), { recursive: true });
  const binary = join(directory, "bin", binaryName);
  await cp(source, binary);
  if (os !== "win32") await chmod(binary, 0o755);
  await writeFile(join(directory, "bin", `${binaryName}.sha256`), `${actual}  ${binaryName}\n`);
  await cp(license, join(directory, "LICENSE"));
  await writeFile(join(directory, "package.json"), `${JSON.stringify({
    name,
    version,
    description: `tokenfold native CLI for ${os} ${cpu}`,
    license: "Apache-2.0",
    repository: "github:snchimata/tokenfold",
    engines: { node: ">=22" },
    os: [os],
    cpu: [cpu],
    files: ["bin", "LICENSE"],
    publishConfig: { access: "public", provenance: true },
  }, null, 2)}\n`);
}
