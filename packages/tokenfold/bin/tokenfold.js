#!/usr/bin/env node
import { spawn } from "node:child_process";
import { binaryPath } from "../dist/index.js";

let child;
try {
  child = spawn(binaryPath(), process.argv.slice(2), { shell: false, stdio: "inherit" });
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}

if (child) {
  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, () => child?.kill(signal));
  }
  child.once("error", (error) => {
    console.error(`Could not run tokenfold: ${error.message}`);
    process.exitCode = 1;
  });
  child.once("close", (code, signal) => {
    if (signal) {
      process.removeAllListeners(signal);
      process.kill(process.pid, signal);
    } else {
      process.exitCode = code ?? 1;
    }
  });
}
