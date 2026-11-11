import { createRequire } from 'node:module';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { InstallationError } from './errors.js';

export interface NativeCancellation { cancel(): void; readonly started: boolean }
export interface NativeFit {
  readonly action: string;
  checkCancelled(): void;
  step(count?: string, counterId?: string): Promise<string>;
  onScore(callback: (error: Error | null) => void): void;
  finish(): void;
}
export interface NativeClient {
  begin(source: string, options: string, cancel: NativeCancellation): Promise<NativeFit>;
  close(): Promise<void>;
  warmup(callback: (error: Error | null) => void): void;
}
interface Addon {
  version(): string;
  NativeCancellation: new () => NativeCancellation;
  NativeClient: new (config: string | undefined, capacity: number, timeout: number) => NativeClient;
  receiptJson(source: string): string;
  count(kind: string, value: string, source: string): Promise<string>;
}
let addon: Addon | undefined;
export function native(): Addon {
  if (addon) return addon;
  const require = createRequire(import.meta.url);
  const platform = `${process.platform}-${process.arch}`;
  const supported = ['linux-x64', 'linux-arm64', 'darwin-x64', 'darwin-arm64', 'win32-x64'];
  try {
    if (!supported.includes(platform)) throw Error();
    if (process.platform === 'linux' && !(process.report.getReport() as {header?: {glibcVersionRuntime?: string}}).header?.glibcVersionRuntime) throw Error();
    const local = fileURLToPath(new URL('../native/tokenfold.node', import.meta.url));
    const loaded: Addon = require(existsSync(local) ? local : `@tokenfold/native-${platform}`);
    if (loaded.version() !== require('../package.json').version) throw Error();
    addon = loaded;
    return loaded;
  } catch {
    throw new InstallationError(`Install tokenfold with its matching @tokenfold/native-${platform} optional package on a supported Node >=22 platform; no CLI fallback is used.`);
  }
}
