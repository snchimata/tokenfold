export type TokenFoldErrorCode =
  | "binary_not_found"
  | "spawn_failed"
  | "tokenfold_exit"
  | "invalid_report";

export class TokenFoldProcessError extends Error {
  readonly code: TokenFoldErrorCode;
  readonly exitCode: number | null;
  readonly signal: NodeJS.Signals | null;
  readonly stderr: Uint8Array;

  constructor(
    message: string,
    options: {
      code: TokenFoldErrorCode;
      exitCode?: number | null;
      signal?: NodeJS.Signals | null;
      stderr?: Uint8Array;
      cause?: unknown;
    },
  ) {
    super(message, { cause: options.cause });
    this.name = "TokenFoldProcessError";
    this.code = options.code;
    this.exitCode = options.exitCode ?? null;
    this.signal = options.signal ?? null;
    this.stderr = options.stderr ?? new Uint8Array();
  }
}
