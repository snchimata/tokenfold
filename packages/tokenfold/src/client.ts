import { EncodingCounter, ProviderCounter } from './accounting.js';
import { AccountingError, CancelledError, ClosedError, ConfigError, InvalidInputError, nativeError } from './errors.js';
import { native, type NativeClient, type NativeFit } from './native.js';
import { FitResult, Receipt, freeze, json, tokenCount, type BudgetMode, type Context, type Data, type Input, type JsonObject, type RenderedRequest, type Request, type TokenCounter } from './types.js';

export interface SelectRuntime { readonly config: string; readonly experimental: true }
export interface ClientOptions { counter?: TokenCounter; select?: SelectRuntime; maxPendingScores?: number; scoreWaitTimeoutMs?: number }
export interface FitOptions { targetTokens: number; allowSelection?: boolean; budgetMode?: BudgetMode; signal?: AbortSignal }
export interface CoreFitOptions { targetTokens: number; counter?: TokenCounter; budgetMode?: BudgetMode; signal?: AbortSignal }
function integer(value: number, lo: number, hi: number, ErrorType: typeof ConfigError): void {
  if (!Number.isSafeInteger(value) || value < lo || value > hi) throw new ErrorType('invalid integer option');
}
function source(input: Input): string {
  // Snapshot first so caller mutation and getters cannot change an admitted fit.
  const snapshot = JSON.parse(json(input)) as Input;
  const content = (value: string | Data): object => {
    if (typeof value === 'string') return {kind:'text',text:value};
    if (!value || value.kind !== 'json_data') throw new InvalidInputError('invalid group content');
    return value;
  };
  if (typeof snapshot === 'string') return JSON.stringify(content(snapshot));
  if (!snapshot || typeof snapshot !== 'object') throw new InvalidInputError('expected fit input');
  if (!['json_data','context','openai_chat','anthropic_messages'].includes(snapshot.kind)) throw new InvalidInputError('invalid input kind');
  if (snapshot.kind === 'context') {
    if (!Array.isArray(snapshot.groups)) throw new InvalidInputError('invalid context groups');
    return JSON.stringify({...snapshot, groups:snapshot.groups.map(g => {
      if (!g || typeof g !== 'object' || Array.isArray(g)) throw new InvalidInputError('invalid context group');
      return {...g,content:content(g.content)};
    })});
  }
  return JSON.stringify(snapshot);
}
export class Tokenfold {
  readonly #client: NativeClient;
  readonly #counter: TokenCounter;
  readonly #pending = new Set<Promise<unknown>>();
  #closing: Promise<void> | undefined;
  constructor(options: ClientOptions = {}) {
    if (!options || typeof options !== 'object' || Array.isArray(options)) throw new ConfigError('invalid client options');
    const capacity = options.maxPendingScores === undefined ? 32 : options.maxPendingScores;
    const timeout = options.scoreWaitTimeoutMs === undefined ? 5000 : options.scoreWaitTimeoutMs;
    integer(capacity,0,128,ConfigError); integer(timeout,1,120000,ConfigError);
    if (options.select !== undefined && (!options.select || options.select.experimental !== true || typeof options.select.config !== 'string' || !options.select.config.isWellFormed())) throw new ConfigError('Select requires explicit experimental approval');
    if (options.counter !== undefined && (!options.counter || typeof options.counter !== 'object')) throw new ConfigError('invalid counter');
    this.#counter = options.counter === undefined ? new EncodingCounter() : options.counter;
    try { this.#client = new (native().NativeClient)(options.select?.config, capacity, timeout); }
    catch (error) { throw nativeError(error); }
  }
  fit(request: string | Data | Context, options: FitOptions): Promise<FitResult<string>>;
  fit(request: Request, options: FitOptions): Promise<FitResult<JsonObject>>;
  fit(request: Input, options: FitOptions): Promise<FitResult<string> | FitResult<JsonObject>>;
  fit(request: Input, options: FitOptions): Promise<FitResult<string> | FitResult<JsonObject>> {
    const running = this.run(request, options);
    this.#pending.add(running);
    void running.then(() => this.#pending.delete(running), () => this.#pending.delete(running));
    return running;
  }
  private counterId(): string {
    try { const id = this.#counter.id; if (typeof id !== 'string' || !id.isWellFormed()) throw Error(); return id; }
    catch { throw new AccountingError('invalid counter ID'); }
  }
  private async run(request: Input, options: FitOptions): Promise<FitResult<string> | FitResult<JsonObject>> {
    if (this.#closing) throw new ClosedError('client is closed');
    if (!options || typeof options !== 'object' || Array.isArray(options)) throw new ConfigError('invalid fit options');
    integer(options.targetTokens,0,Number.MAX_SAFE_INTEGER,InvalidInputError);
    const mode = options.budgetMode === undefined ? 'strict' : options.budgetMode;
    const selection = options.allowSelection === undefined ? false : options.allowSelection;
    if (!['strict','best_effort'].includes(mode) || typeof selection !== 'boolean') throw new ConfigError('invalid fit options');
    const signal = options.signal;
    if (signal !== undefined && !(signal instanceof AbortSignal)) throw new InvalidInputError('invalid signal');
    if (signal?.aborted) throw new CancelledError('fit cancelled');
    const cancel = new (native().NativeCancellation)();
    const abort = () => cancel.cancel();
    signal?.addEventListener('abort', abort, {once:true});
    let session: NativeFit | undefined;
    try {
      const counter = this.#counter;
      const kind = Object.getPrototypeOf(counter) === EncodingCounter.prototype ? 'encoding' : Object.getPrototypeOf(counter) === ProviderCounter.prototype ? 'openai' : 'custom';
      const value = kind === 'encoding' ? (counter as EncodingCounter).encoding : kind === 'openai' ? (counter as ProviderCounter).model : this.counterId();
      session = await this.#client.begin(source(request), JSON.stringify({target:options.targetTokens,mode,allow_selection:selection,counter_kind:kind,counter_value:value}),cancel);
      let action = JSON.parse(session.action);
      while (action.action !== 'done') {
        if (action.action === 'score') {
          await new Promise<void>((resolve,reject) => session!.onScore(error => error ? reject(error) : resolve()));
          action = JSON.parse(await session.step());
        } else {
          session.checkCancelled();
          const before = this.counterId();
          const rendered: RenderedRequest = freeze({format:action.format,payload:action.payload,scope:action.scope});
          session.checkCancelled();
          let callable: TokenCounter['count'];
          try {
            callable = counter.count;
            if (typeof callable !== 'function') throw Error();
          } catch { throw new AccountingError('custom counter failed'); }
          session.checkCancelled();
          let count, after;
          try {
            count = tokenCount(await Reflect.apply(callable, counter, [rendered]));
            after = this.counterId();
            if (before !== after) throw Error();
          } catch { throw new AccountingError('custom counter failed'); }
          action = JSON.parse(await session.step(JSON.stringify(count), after));
        }
      }
      const receipt = Receipt.fromJSON(JSON.parse(action.receipt));
      return typeof action.payload === 'string' ? new FitResult<string>(action.payload, receipt) : new FitResult<JsonObject>(action.payload, receipt);
    } catch (error) { throw nativeError(error); }
    finally { signal?.removeEventListener('abort',abort); session?.finish(); }
  }
  async warmup(): Promise<void> {
    if (this.#closing) throw new ClosedError('client is closed');
    try { await new Promise<void>((resolve,reject) => this.#client.warmup(error => error ? reject(error) : resolve())); }
    catch (error) { throw nativeError(error); }
  }
  close(): Promise<void> {
    if (!this.#closing) {
      // Native close stops admission/signals immediately, before awaiting any host callback.
      const closing = this.#client.close();
      this.#closing = Promise.all([closing, Promise.allSettled([...this.#pending])]).then(() => undefined).catch(error => { throw nativeError(error); });
    }
    return this.#closing;
  }
}
export function fit(request: string | Data | Context, options: CoreFitOptions): Promise<FitResult<string>>;
export function fit(request: Request, options: CoreFitOptions): Promise<FitResult<JsonObject>>;
export function fit(request: Input, options: CoreFitOptions): Promise<FitResult<string> | FitResult<JsonObject>>;
export async function fit(request: Input, options: CoreFitOptions): Promise<FitResult<string> | FitResult<JsonObject>> {
  if (!options || typeof options !== 'object' || Array.isArray(options)) throw new ConfigError('invalid fit options');
  const client = new Tokenfold(options.counter === undefined ? {} : {counter:options.counter});
  try { return await client.fit(request, {...options,allowSelection:false}); }
  finally { await client.close(); }
}
