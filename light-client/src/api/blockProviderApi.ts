import type { ApiAuthOptions } from './lightSyncApi';

export interface BlockProviderConfig extends ApiAuthOptions {
  name: string;
  url: string;
}

export interface RawBlockResult {
  providerName: string;
  blockHash: string;
  rawBlock: Uint8Array;
}

export class BlockProviderApi {
  readonly name: string;
  readonly baseUrl: string;
  private readonly auth?: ApiAuthOptions;

  constructor(config: BlockProviderConfig) {
    this.name = config.name;
    this.baseUrl = normalizeBaseUrl(config.url);
    this.auth = config;
  }


  async getTipHeight(signal?: AbortSignal): Promise<number> {
    const response = await fetch(this.url('/blocks/tip/height'), {
      method: 'GET',
      headers: this.headers('text/plain'),
      signal,
    });
    if (!response.ok) throw new Error(`${this.name}: ${response.status} ${response.statusText}`);
    const height = Number((await response.text()).trim());
    if (!Number.isSafeInteger(height)) throw new Error(`${this.name}: invalid tip height response`);
    return height;
  }

  async getBlockTransactions(hash: string, signal?: AbortSignal): Promise<EsploraTransaction[]> {
    const response = await fetch(this.url(`/block/${hash}/txs/0`), {
      method: 'GET',
      headers: this.headers('application/json'),
      signal,
    });
    if (!response.ok) throw new Error(`${this.name}: ${response.status} ${response.statusText}`);
    return response.json() as Promise<EsploraTransaction[]>;
  }

  async getBlockHashByHeight(height: number, signal?: AbortSignal): Promise<string> {
    const response = await fetch(this.url(`/block-height/${height}`), {
      method: 'GET',
      headers: this.headers('text/plain'),
      signal,
    });
    if (!response.ok) throw new Error(`${this.name}: ${response.status} ${response.statusText}`);
    return (await response.text()).trim().replace(/^"|"$/g, '');
  }

  async getRawBlockByHash(hash: string, signal?: AbortSignal): Promise<Uint8Array> {
    const response = await fetch(this.url(`/block/${hash}/raw`), {
      method: 'GET',
      headers: this.headers('application/octet-stream'),
      signal,
    });
    if (!response.ok) throw new Error(`${this.name}: ${response.status} ${response.statusText}`);
    return new Uint8Array(await response.arrayBuffer());
  }

  async getRawBlockByHeight(height: number, signal?: AbortSignal): Promise<RawBlockResult> {
    const blockHash = await this.getBlockHashByHeight(height, signal);
    const rawBlock = await this.getRawBlockByHash(blockHash, signal);
    return { providerName: this.name, blockHash, rawBlock };
  }

  async broadcastTransaction(rawTxHex: string, signal?: AbortSignal): Promise<string> {
    const response = await fetch(this.url('/tx'), {
      method: 'POST',
      headers: { ...this.headers('text/plain'), 'Content-Type': 'text/plain' },
      body: rawTxHex,
      signal,
    });
    if (!response.ok) throw new Error(`${this.name}: ${response.status} ${response.statusText} ${await response.text()}`);
    return (await response.text()).trim().replace(/^\"|\"$/g, '');
  }

  async getFeeEstimates(signal?: AbortSignal): Promise<Record<string, number>> {
    const response = await fetch(this.url('/fee-estimates'), {
      method: 'GET',
      headers: this.headers('application/json'),
      signal,
    });
    if (!response.ok) throw new Error(`${this.name}: ${response.status} ${response.statusText}`);
    return response.json() as Promise<Record<string, number>>;
  }

  private headers(accept: string): HeadersInit {
    const headers: Record<string, string> = { Accept: accept };
    if (!this.auth?.apiKey) return headers;
    if (this.auth.apiKeyPlacement === 'header') {
      headers[this.auth.apiKeyName || 'x-api-key'] = this.auth.apiKey;
    }
    if (this.auth.apiKeyPlacement === 'bearer') {
      headers.Authorization = `Bearer ${this.auth.apiKey}`;
    }
    return headers;
  }

  private url(path: string): string {
    const query = new URLSearchParams();
    if (this.auth?.apiKey && this.auth.apiKeyPlacement === 'query') {
      query.set(this.auth.apiKeyName || 'api_key', this.auth.apiKey);
    }
    const suffix = query.toString() ? `?${query.toString()}` : '';
    return `${this.baseUrl}${path}${suffix}`;
  }
}

export async function fetchRawBlockWithFallback(
  providers: BlockProviderConfig[],
  height: number,
  signal?: AbortSignal,
): Promise<RawBlockResult> {
  const errors: string[] = [];
  for (const provider of providers) {
    try {
      return await new BlockProviderApi(provider).getRawBlockByHeight(height, signal);
    } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') throw error;
      errors.push(error instanceof Error ? error.message : String(error));
    }
  }
  throw new Error(`All block providers failed: ${errors.join('; ')}`);
}

function normalizeBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim();
  return trimmed.endsWith('/') ? trimmed.slice(0, -1) : trimmed;
}


export async function broadcastTransactionWithFallback(
  providers: BlockProviderConfig[],
  rawTxHex: string,
  signal?: AbortSignal,
): Promise<{ providerName: string; txid: string }> {
  const errors: string[] = [];
  for (const provider of providers) {
    try {
      const txid = await new BlockProviderApi(provider).broadcastTransaction(rawTxHex, signal);
      return { providerName: provider.name, txid };
    } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') throw error;
      errors.push(error instanceof Error ? error.message : String(error));
    }
  }
  throw new Error(`All block providers failed to broadcast: ${errors.join('; ')}`);
}


export interface EsploraTransaction {
  txid: string;
  fee?: number;
  status?: { confirmed?: boolean; block_height?: number; block_time?: number };
  vin?: Array<{ txid?: string; vout?: number; prevout?: { value?: number } }>;
  vout?: Array<{ value?: number; scriptpubkey?: string; scriptpubkey_address?: string; scriptpubkey_type?: string }>;
}

export async function fetchFeeEstimatesWithFallback(
  providers: BlockProviderConfig[],
  signal?: AbortSignal,
): Promise<{ providerName: string; fees: Record<string, number> }> {
  const errors: string[] = [];
  for (const provider of providers) {
    try {
      const fees = await new BlockProviderApi(provider).getFeeEstimates(signal);
      return { providerName: provider.name, fees };
    } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') throw error;
      errors.push(error instanceof Error ? error.message : String(error));
    }
  }
  throw new Error(`All block providers failed fee-estimate fetch: ${errors.join('; ')}`);
}
