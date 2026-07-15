import { spawn } from "node:child_process";
import { binaryPath } from "./binary.js";
import { TokenFoldProcessError } from "./errors.js";

export type Input = string | Uint8Array;

export interface ProcessResult {
  stdout: Uint8Array;
  stderr: Uint8Array;
  exitCode: number | null;
  signal: NodeJS.Signals | null;
}

export interface RunOptions {
  stdin?: Input;
  cwd?: string;
  env?: Readonly<Record<string, string | undefined>>;
  signal?: AbortSignal;
}

function environment(overrides?: Readonly<Record<string, string | undefined>>): NodeJS.ProcessEnv {
  const env = { ...process.env };
  for (const [key, value] of Object.entries(overrides ?? {})) {
    if (value === undefined) delete env[key];
    else env[key] = value;
  }
  return env;
}

export function run(args: readonly string[], options: RunOptions = {}): Promise<ProcessResult> {
  return new Promise((resolve, reject) => {
    let child;
    try {
      child = spawn(binaryPath(), [...args], {
        cwd: options.cwd,
        env: environment(options.env),
        signal: options.signal,
        shell: false,
        stdio: ["pipe", "pipe", "pipe"],
      });
    } catch (cause) {
      reject(new TokenFoldProcessError("Could not start tokenfold", { code: "spawn_failed", cause }));
      return;
    }

    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    let settled = false;

    child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
    child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
    // A process may exit before consuming all input. Its close/error event is
    // authoritative; avoid turning the resulting broken pipe into an unhandled event.
    child.stdin.on("error", () => {});
    child.once("error", (cause) => {
      if (settled) return;
      settled = true;
      reject(new TokenFoldProcessError("Could not run tokenfold", {
        code: "spawn_failed",
        stderr: Buffer.concat(stderr),
        cause,
      }));
    });
    child.once("close", (exitCode, signal) => {
      if (settled) return;
      settled = true;
      resolve({
        stdout: Buffer.concat(stdout),
        stderr: Buffer.concat(stderr),
        exitCode,
        signal,
      });
    });

    child.stdin.end(options.stdin === undefined ? undefined : Buffer.from(options.stdin));
  });
}
