import { ConfigError, nativeError } from './errors.js';
import { native } from './native.js';
import { json, tokenCount, type RenderedRequest, type TokenCount, type TokenCounter } from './types.js';

export class EncodingCounter implements TokenCounter {
  constructor(readonly encoding: 'o200k_base' | 'cl100k_base' = 'o200k_base') {
    if (!['o200k_base','cl100k_base'].includes(encoding)) throw new ConfigError('invalid encoding');
  }
  get id(): string { return `encoding:${this.encoding}:v1`; }
  async count(request: RenderedRequest): Promise<TokenCount> {
    try { return tokenCount(JSON.parse(await native().count('encoding', this.encoding, json(request)))); }
    catch (error) { throw nativeError(error); }
  }
}
export class ProviderCounter implements TokenCounter {
  private constructor(readonly model: string) {
    if (model !== 'gpt-4o-2024-08-06') throw new ConfigError('unsupported provider profile');
  }
  static openai(options: {model: string}): ProviderCounter { return new ProviderCounter(options.model); }
  get id(): string { return `openai_chat:${this.model}:local-text-v1`; }
  async count(request: RenderedRequest): Promise<TokenCount> {
    try { return tokenCount(JSON.parse(await native().count('openai', this.model, json(request)))); }
    catch (error) { throw nativeError(error); }
  }
}
