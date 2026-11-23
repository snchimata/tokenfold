import { AccountingError, BudgetIndeterminateError, BudgetUnmetError, InvalidInputError, nativeError } from './errors.js';
import { native } from './native.js';

export type JsonValue = null | boolean | number | string | readonly JsonValue[] | JsonObject;
export interface JsonObject { readonly [key: string]: JsonValue }
export interface Data { readonly kind: 'json_data'; readonly value: JsonValue; readonly representation?: 'json' | 'folded' }
export interface Group { readonly id: string; readonly content: string | Data; readonly required?: boolean }
export interface Context { readonly kind: 'context'; readonly query: string; readonly groups: readonly Group[] }
export interface MessageGroup { readonly id: string; readonly indices: readonly number[]; readonly required?: boolean }
export interface Request { readonly kind: 'openai_chat' | 'anthropic_messages'; readonly payload: JsonObject; readonly query?: string | null; readonly groups?: readonly MessageGroup[] }
export type Input = string | Data | Context | Request;
export type BudgetMode = 'strict' | 'best_effort';
export type BudgetStatus = 'met' | 'unmet' | 'indeterminate';
export type CountBasis = 'exact' | 'estimated' | 'unavailable';
export type FitFormat = 'text' | Data['kind'] | Context['kind'] | Request['kind'];
export type AccountingScope = 'rendered_text' | 'provider_request';
export type FitReason = 'selection_disabled' | 'runtime_unavailable' | 'no_optional_groups' | 'required_over_budget' | 'unsupported_selection' | 'selection_limit' | 'scorer_failure' | 'scorer_timeout' | 'scorer_queue_timeout' | 'scorer_response_invalid' | 'no_valid_fit' | 'no_gain' | 'unknown_accounting' | 'estimated_fit';
export interface RenderedRequest { readonly format: FitFormat; readonly payload: string | JsonObject; readonly scope: AccountingScope }
export interface TokenCount { readonly tokens: number | null; readonly basis: CountBasis }
export interface TokenCounter { readonly id: string; count(request: RenderedRequest): TokenCount | Promise<TokenCount> }
export interface Accounting { readonly counterId: string; readonly scope: AccountingScope; readonly originalBasis: CountBasis; readonly baselineBasis: CountBasis; readonly finalBasis: CountBasis }
export interface ScorerInfo { readonly revision: string; readonly manifestHash: string; readonly runtimeId: string; readonly device: string; readonly precision: string; readonly qualification: 'unqualified' }
export interface FitWarning { readonly code: string; readonly message: string }
export interface TimingsMs { readonly total: number | null; readonly core: number | null; readonly accounting: number | null; readonly scoreQueue: number | null; readonly startup: number | null; readonly scoring: number | null }
export interface Receipt {
  readonly kind: 'tokenfold.fit'; readonly schemaVersion: '1.0'; readonly format: FitFormat;
  readonly budgetMode: BudgetMode; readonly budgetStatus: BudgetStatus; readonly reason: FitReason | null;
  readonly targetTokens: number; readonly originalTokens: number | null; readonly baselineTokens: number | null; readonly finalTokens: number | null;
  readonly accounting: Accounting; readonly coreTransforms: readonly string[]; readonly scorerAttempted: boolean; readonly scorerSucceeded: boolean;
  readonly selectionApplied: boolean; readonly scorer: ScorerInfo | null; readonly groupsKept: readonly string[]; readonly groupsDropped: readonly string[];
  readonly warnings: readonly FitWarning[]; readonly timingsMs: TimingsMs;
}
export class Receipt {
  readonly #wire: string;
  private constructor(wire: string) {
    this.#wire = wire;
    const camel = (v: unknown): unknown => Array.isArray(v) ? v.map(camel) : v && typeof v === 'object' ? Object.fromEntries(Object.entries(v).map(([k, value]) => [k.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase()), camel(value)])) : v;
    Object.assign(this, camel(JSON.parse(wire)));
    freeze(this);
  }
  static fromJSON(value: unknown): Receipt {
    try { return new Receipt(native().receiptJson(json(value))); }
    catch (error) { throw nativeError(error); }
  }
  toJSON(): JsonObject { return JSON.parse(this.#wire) as JsonObject; }
}
export class FitResult<P extends string | JsonObject> {
  readonly #payload: string;
  constructor(payload: P, readonly receipt: Receipt) { this.#payload = JSON.stringify(payload); Object.freeze(this); }
  get payload(): P { return JSON.parse(this.#payload) as P; }
  requireMet(): P {
    if (this.budgetStatus === 'unmet') throw new BudgetUnmetError(this.receipt);
    if (this.budgetStatus !== 'met') throw new BudgetIndeterminateError(this.receipt);
    return this.payload;
  }
  requireFit(): P { return this.budgetMode === 'best_effort' && this.estimatedFit ? this.payload : this.requireMet(); }
  get budgetMode(): BudgetMode { return this.receipt.budgetMode; }
  get budgetStatus(): BudgetStatus { return this.receipt.budgetStatus; }
  get reason(): FitReason | null { return this.receipt.reason; }
  get targetTokens(): number { return this.receipt.targetTokens; }
  get originalTokens(): number | null { return this.receipt.originalTokens; }
  get baselineTokens(): number | null { return this.receipt.baselineTokens; }
  get finalTokens(): number | null { return this.receipt.finalTokens; }
  get groupsKept(): readonly string[] { return this.receipt.groupsKept; }
  get groupsDropped(): readonly string[] { return this.receipt.groupsDropped; }
  get selectionApplied(): boolean { return this.receipt.selectionApplied; }
  get tokensSaved(): number | null { return this.delta(this.originalTokens, this.receipt.accounting.originalBasis); }
  get selectionTokensSaved(): number | null { return this.delta(this.baselineTokens, this.receipt.accounting.baselineBasis); }
  private delta(before: number | null, basis: CountBasis): number | null {
    return before !== null && this.finalTokens !== null && basis === this.receipt.accounting.finalBasis ? before - this.finalTokens : null;
  }
  get estimatedFit(): boolean | null { return this.receipt.accounting.finalBasis === 'estimated' && this.finalTokens !== null ? this.finalTokens <= this.targetTokens : null; }
}

export function freeze<T>(value: T): T {
  if (value && typeof value === 'object') { for (const child of Object.values(value)) freeze(child); Object.freeze(value); }
  return value;
}
/** Reject JSON coercions before serialization; native code owns source limits and policy. */
export function json(value: unknown): string {
  const active = new Set<object>();
  const walk = (v: unknown, depth: number): void => {
    if (depth > 70) throw new InvalidInputError('invalid JSON depth');
    if (v === null || typeof v === 'boolean') return;
    if (typeof v === 'string') { if (!v.isWellFormed()) throw new InvalidInputError('invalid Unicode'); return; }
    if (typeof v === 'number') {
      if (!Number.isFinite(v) || (Number.isInteger(v) && !Number.isSafeInteger(v))) throw new InvalidInputError('invalid JSON number');
      return;
    }
    if (!v || typeof v !== 'object' || active.has(v)) throw new InvalidInputError('invalid JSON value');
    const proto = Object.getPrototypeOf(v);
    if (!Array.isArray(v) && proto !== Object.prototype && proto !== null) throw new InvalidInputError('expected plain JSON');
    active.add(v);
    for (const key of Reflect.ownKeys(v)) {
      if (Array.isArray(v) && key === 'length') continue;
      const descriptor = Object.getOwnPropertyDescriptor(v, key)!;
      if (typeof key !== 'string' || !descriptor.enumerable || !('value' in descriptor)) throw new InvalidInputError('invalid JSON property');
      walk(key, depth); walk(descriptor.value, depth + 1);
    }
    if (Array.isArray(v) && (Object.keys(v).length !== v.length || !Object.keys(v).every((key,index) => key === String(index)))) throw new InvalidInputError('sparse JSON array');
    active.delete(v);
  };
  try { walk(value, 0); return JSON.stringify(value); }
  catch { throw new InvalidInputError('invalid JSON input'); }
}
export function tokenCount(value: unknown): TokenCount {
  if (!value || typeof value !== 'object') throw new AccountingError('invalid token count');
  const {tokens, basis} = value as TokenCount;
  if (!['exact','estimated','unavailable'].includes(basis) || (basis === 'unavailable' ? tokens !== null : !Number.isSafeInteger(tokens) || tokens === null || tokens < 0)) throw new AccountingError('invalid token count');
  return {tokens, basis};
}
