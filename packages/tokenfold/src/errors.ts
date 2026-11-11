import type { Receipt } from './types.js';

export class TokenfoldError extends Error {
  constructor(message: string) { super(message); this.name = new.target.name; }
}
export class InstallationError extends TokenfoldError {}
export class InvalidInputError extends TokenfoldError {}
export class ConfigError extends TokenfoldError {}
export class AccountingError extends TokenfoldError {}
export class SafetyError extends TokenfoldError {}
export class BusyError extends TokenfoldError {}
export class ClosedError extends TokenfoldError {}
export class ScorerError extends TokenfoldError {}
export class CancelledError extends TokenfoldError {}
export class InternalError extends TokenfoldError {}
export class BudgetUnmetError extends TokenfoldError {
  constructor(readonly receipt: Receipt) { super('token budget unmet'); }
}
export class BudgetIndeterminateError extends TokenfoldError {
  constructor(readonly receipt: Receipt) { super('token budget indeterminate'); }
}
export function nativeError(cause: unknown): Error {
  if (cause instanceof TokenfoldError) return cause;
  const errors = { InvalidInputError, ConfigError, AccountingError, SafetyError, BusyError, ClosedError, ScorerError, CancelledError, InternalError };
  const name = cause instanceof Error ? cause.message : '';
  const ErrorType = Object.hasOwn(errors, name) ? errors[name as keyof typeof errors] : InternalError;
  return new ErrorType(name in errors ? name : 'native operation failed');
}

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
