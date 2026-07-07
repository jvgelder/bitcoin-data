import type { LightBlockJson } from '../api/types';

export interface BlockSummary {
  height: number;
  blockHash: string;
  previousBlockHash: string;
  firstUid: number;
  outputCount: number;
  tweakCount: number;
  skippedTweakTxs: number;
  truncatedHashBits: number;
  truncatedHashBytes: number;
  spentCount: number;
  spentIdBytes: number;
}

export interface ContinuityResult {
  ok: true;
}

export function summarizeBlock(block: LightBlockJson): BlockSummary {
  return {
    height: block.height,
    blockHash: block.block_hash,
    previousBlockHash: block.previous_block_hash,
    firstUid: block.first_uid,
    outputCount: block.tweaks.reduce((sum, tweak) => sum + tweak.output_count, 0),
    tweakCount: block.tweaks.length,
    skippedTweakTxs: block.skipped_txs_for_tweaks.length,
    truncatedHashBits: block.truncated_output_hash_bits,
    truncatedHashBytes: block.truncated_output_hash_bytes,
    spentCount: block.spent_count,
    spentIdBytes: block.spent_id_bytes,
  };
}

export function assertRangeContinuity(
  blocks: LightBlockJson[],
  expectedStart: number,
  previousLocalHash?: string,
): ContinuityResult {
  if (blocks.length === 0) {
    throw new Error('range response contained no blocks');
  }

  if (blocks[0].height !== expectedStart) {
    throw new Error(`range starts at ${blocks[0].height}, expected ${expectedStart}`);
  }

  if (previousLocalHash && blocks[0].previous_block_hash !== previousLocalHash) {
    throw new Error(
      `chain continuity failed at ${blocks[0].height}: previous hash ${shortHash(blocks[0].previous_block_hash)} does not match local ${shortHash(previousLocalHash)}`,
    );
  }

  for (let i = 1; i < blocks.length; i += 1) {
    const prev = blocks[i - 1];
    const current = blocks[i];
    if (current.height !== prev.height + 1) {
      throw new Error(`height gap between ${prev.height} and ${current.height}`);
    }
    if (current.previous_block_hash !== prev.block_hash) {
      throw new Error(
        `chain continuity failed between ${prev.height} and ${current.height}: ${shortHash(current.previous_block_hash)} != ${shortHash(prev.block_hash)}`,
      );
    }
  }

  return { ok: true };
}

function shortHash(hash: string): string {
  if (hash.length <= 16) return hash;
  return `${hash.slice(0, 8)}…${hash.slice(-8)}`;
}
