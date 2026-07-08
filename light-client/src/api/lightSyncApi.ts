import { parseBinaryRangeFrame } from './rangeFrame';
import type {
  BinaryRangeResponse,
  ChainTip,
  HealthResponse,
  LightBlockJson,
  LightBlockRangeJson,
  LightClientQuery,
  ManifestResponse,
  RangeHeaders,
  RangeRequest,
} from './types';

export type ApiKeyPlacement = 'none' | 'header' | 'bearer' | 'query';

export interface ApiAuthOptions {
  apiKey?: string;
  apiKeyPlacement?: ApiKeyPlacement;
  apiKeyName?: string;
}

export class LightSyncApiError extends Error {
  constructor(
    message: string,
    readonly status?: number,
    readonly body?: unknown,
  ) {
    super(message);
    this.name = 'LightSyncApiError';
  }
}

export class LightSyncApi {
  readonly baseUrl: string;
  private readonly auth?: ApiAuthOptions;

  constructor(baseUrl: string, auth?: ApiAuthOptions) {
    this.baseUrl = normalizeBaseUrl(baseUrl);
    this.auth = auth;
  }

  async getHealth(signal?: AbortSignal): Promise<HealthResponse> {
    return this.getJson<HealthResponse>('/health', undefined, signal);
  }

  async getManifest(signal?: AbortSignal): Promise<ManifestResponse> {
    return this.getJson<ManifestResponse>('/manifest', undefined, signal);
  }

  async getTip(signal?: AbortSignal): Promise<ChainTip> {
    return this.getJson<ChainTip>('/tip', undefined, signal);
  }

  async getBlock(height: number, query: LightClientQuery, signal?: AbortSignal): Promise<LightBlockJson> {
    return this.getJson<LightBlockJson>(`/blocks/${height}/light`, queryToSearch(query), signal);
  }

  async getBlockRange(request: RangeRequest, signal?: AbortSignal): Promise<LightBlockRangeJson> {
    return this.getJson<LightBlockRangeJson>(
      '/blocks/light',
      queryToSearch({ ...request, start: request.start, count: request.count }),
      signal,
    );
  }

  async getBlockRangeBinary(request: RangeRequest, signal?: AbortSignal): Promise<BinaryRangeResponse> {
    const response = await fetch(this.url('/blocks/light', queryToSearch(request)), {
      method: 'GET',
      headers: this.headers('application/octet-stream'),
      signal,
    });
    if (!response.ok) {
      throw await toApiError(response);
    }
    const body = new Uint8Array(await response.arrayBuffer());
    return {
      frame: parseBinaryRangeFrame(body),
      headers: readRangeHeaders(response.headers),
    };
  }

  private async getJson<T>(path: string, search?: URLSearchParams, signal?: AbortSignal): Promise<T> {
    const response = await fetch(this.url(path, search), {
      method: 'GET',
      headers: this.headers('application/json'),
      signal,
    });
    if (!response.ok) {
      throw await toApiError(response);
    }
    return (await response.json()) as T;
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

  private url(path: string, search?: URLSearchParams): string {
    const normalizedPath = path.startsWith('/') ? path : `/${path}`;
    const query = search ? new URLSearchParams(search) : new URLSearchParams();
    if (this.auth?.apiKey && this.auth.apiKeyPlacement === 'query') {
      query.set(this.auth.apiKeyName || 'api_key', this.auth.apiKey);
    }
    const suffix = query.toString().length > 0 ? `?${query.toString()}` : '';
    return `${this.baseUrl}${normalizedPath}${suffix}`;
  }
}

function normalizeBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim();
  if (!trimmed) return 'http://127.0.0.1:3000';
  return trimmed.endsWith('/') ? trimmed.slice(0, -1) : trimmed;
}

function queryToSearch(query: object | undefined): URLSearchParams | undefined {
  if (!query) return undefined;

  const q = query as Record<string, unknown>;
  const search = new URLSearchParams();
  setNumber(search, 'start', q.start);
  setNumber(search, 'count', q.count);
  setNumber(search, 'labels', q.labels);
  setBoolean(search, 'filter_reuse', q.filterReuse);
  setBoolean(search, 'cutthrough', q.cutthrough);
  setNumber(search, 'cutthrough_start', q.cutthroughStart);
  setNumber(search, 'cutthrough_tip', q.cutthroughTip);
  setNumber(search, 'max_bytes', q.maxBytes);
  return search;
}

function setNumber(search: URLSearchParams, key: string, value: unknown): void {
  if (typeof value === 'number' && Number.isFinite(value)) {
    search.set(key, String(Math.trunc(value)));
  }
}

function setBoolean(search: URLSearchParams, key: string, value: unknown): void {
  if (typeof value === 'boolean' && value) {
    search.set(key, 'true');
  }
}

function readRangeHeaders(headers: Headers): RangeHeaders {
  return {
    start: readNumberHeader(headers, 'x-bitcoindata-range-start'),
    end: readNumberHeader(headers, 'x-bitcoindata-range-end'),
    requestedEnd: readNumberHeader(headers, 'x-bitcoindata-requested-range-end'),
    count: readNumberHeader(headers, 'x-bitcoindata-range-count'),
    complete: readBooleanHeader(headers, 'x-bitcoindata-range-complete'),
    nextStart: readNumberHeader(headers, 'x-bitcoindata-next-start'),
  };
}

function readNumberHeader(headers: Headers, name: string): number | undefined {
  const value = headers.get(name);
  if (value == null) return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function readBooleanHeader(headers: Headers, name: string): boolean | undefined {
  const value = headers.get(name);
  if (value == null) return undefined;
  return value === 'true';
}

async function toApiError(response: Response): Promise<LightSyncApiError> {
  const text = await response.text();
  let body: unknown = text;
  try {
    body = JSON.parse(text);
  } catch {
    // Keep raw text when the server did not return JSON.
  }

  const message =
    typeof body === 'object' && body !== null && 'error' in body
      ? String((body as { error: unknown }).error)
      : `${response.status} ${response.statusText}`;

  return new LightSyncApiError(message, response.status, body);
}
