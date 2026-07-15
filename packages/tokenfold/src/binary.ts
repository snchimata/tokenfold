import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { TokenFoldProcessError } from "./errors.js";

const require = createRequire(import.meta.url);

const PLATFORM_PACKAGES: Readonly<Record<string, readonly [string, string]>> = {
  "darwin-x64": ["@tokenfold/cli-darwin-x64", "tokenfold"],
  "darwin-arm64": ["@tokenfold/cli-darwin-arm64", "tokenfold"],
  "linux-x64": ["@tokenfold/cli-linux-x64", "tokenfold"],
  "linux-arm64": ["@tokenfold/cli-linux-arm64", "tokenfold"],
  "win32-x64": ["@tokenfold/cli-win32-x64", "tokenfold.exe"],
};

export function binaryPath(): string {
  const override = process.env.TOKENFOLD_BINARY_PATH;
  if (override) return resolve(override);

  const key = `${process.platform}-${process.arch}`;
  const platformPackage = PLATFORM_PACKAGES[key];
  if (!platformPackage) {
    throw new TokenFoldProcessError(`Unsupported tokenfold platform: ${key}`, {
      code: "binary_not_found",
    });
  }

  const [packageName, binaryName] = platformPackage;
  try {
    return resolve(dirname(require.resolve(`${packageName}/package.json`)), "bin", binaryName);
  } catch (cause) {
    throw new TokenFoldProcessError(
      `The optional package ${packageName} is not installed for ${key}`,
      { code: "binary_not_found", cause },
    );
  }
}
