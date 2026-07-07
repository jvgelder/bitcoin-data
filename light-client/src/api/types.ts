export type SyncStatus = 'idle' | 'connecting' | 'syncing' | 'error' | 'stopped';

export interface HealthResponse {
  ok?: boolean;
  version?: number;
  [key: string]: unknown;
}

export interface ChainTip {
  height: number;
  block_hash: string;
}

export interface ManifestResponse {
  version: number;
  network: string;
  genesis_hash?: string | null;
  finality_depth?: number;
  suggested_reorg_cache_depth?: number;
  max_range_count?: number;
  tip?: ChainTip | null;
  [key: string]: unknown;
}

export interface TweakEntryJson {
  output_count: number;
  tweak: string;
}

export interface LightBlockJson {
  version: number;
  height: number;
  block_hash: string;
  previous_block_hash: string;
  first_uid: number;
  skipped_txs_for_tweaks: number[];
  tweaks: TweakEntryJson[];
  truncated_output_hash_bits: number;
  truncated_output_hash_bytes: number;
  truncated_output_hashes: string;
  spent_id_codec: string;
  spent_count: number;
  spent_id_bytes: number;
  spent_ids: string;
  decoded_spent_ids: number[];
}

export interface LightBlockRangeJson {
  version: number;
  format: 'light-block-range' | string;
  start: number;
  end: number;
  requested_end: number;
  count: number;
  complete: boolean;
  next_start?: number | null;
  blocks: LightBlockJson[];
}

export interface LightClientQuery {
  labels?: number;
  filterReuse?: boolean;
  cutthrough?: boolean;
  cutthroughStart?: number;
  cutthroughTip?: number;
  maxBytes?: number;
}

export interface RangeRequest extends LightClientQuery {
  start: number;
  count: number;
}

export interface BinaryRangeFrame {
  version: number;
  messages: Uint8Array[];
}

export interface RangeHeaders {
  start?: number;
  end?: number;
  requestedEnd?: number;
  count?: number;
  complete?: boolean;
  nextStart?: number;
}

export interface BinaryRangeResponse {
  frame: BinaryRangeFrame;
  headers: RangeHeaders;
}
