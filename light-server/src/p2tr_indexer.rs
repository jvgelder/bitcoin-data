//! P2TR/SP archive builder primitives.
//!
//! This module is source-agnostic. A Bitcoin Core REST/RPC adapter can decode
//! blocks and feed `BlockScanInput` values here. UID assignment is scoped by
//! `Profile.scope`:
//!
//! - `p2tr-sp`: every P2TR output receives a UID. Reused keys are included.
//!   NUMS is handled on input-side BIP352 spend eligibility, not output creation.
//! - `p2tr`: every P2TR output receives a UID, including reused keys.
//! - `all-outputs`: every output receives a UID; callers must provide canonical identity bytes.

use crate::index::{LightBlockInput, OutputRefInput};
use crate::output_id::{choose_output_id_bytes, truncate_into_packed};
use crate::profile::Profile;
use crate::types::{BlockHashBytes, OutputIdHash, TxTweak, TxidBytes};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};

pub const OUTPUT_ID_COLLISION_PROBABILITY_LOG2: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutPointKey {
    pub txid: TxidBytes,
    pub vout: u32,
}

#[derive(Debug, Clone)]
pub struct TxInputScan {
    pub previous_output: OutPointKey,
}

#[derive(Debug, Clone)]
pub struct TxOutputScan {
    pub vout: u32,
    pub is_p2tr: bool,
    /// Input-side NUMS accounting hook. For real Bitcoin output scanning this
    /// should normally be false: BIP352 NUMS handling applies to Taproot
    /// script-path spends, not to P2TR output creation.
    pub is_nums: bool,
    /// Canonical bytes committed into the served output identifier. For P2TR
    /// this must be the 32-byte x-only output key.
    pub identity_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TxScanInput {
    pub txid: TxidBytes,
    pub tx_index: u32,
    pub inputs: Vec<TxInputScan>,
    pub outputs: Vec<TxOutputScan>,
    /// BIP352/Blindbit per-transaction tweak: the 33-byte compressed public key input_hash*A.
    /// Do not fill this with fake data. Live indexing may set this only when it has
    /// complete prevout context and has applied all BIP352 input eligibility rules.
    pub silent_payment_tweak: Option<TxTweak>,
}

#[derive(Debug, Clone)]
pub struct BlockScanInput {
    pub height: u64,
    pub block_hash: BlockHashBytes,
    pub previous_block_hash: BlockHashBytes,
    pub txs: Vec<TxScanInput>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockScopeStats {
    pub tx_count: u32,
    pub output_count_total: u32,
    pub p2tr_output_count: u32,
    pub p2tr_sp_candidate_count: u32,
    pub p2tr_nums_count: u32,
    pub p2tr_reused_count: u32,
    pub p2tr_excluded_by_scope_count: u32,
    pub indexed_output_count: u32,
    pub indexed_spent_count: u32,
    pub tx_with_p2tr_output_count: u32,
    pub tx_with_indexed_output_count: u32,
    pub tweak_count: u32,
}

#[derive(Debug, Clone)]
pub struct AppliedBlock {
    pub light_block: LightBlockInput,
    pub stats: BlockScopeStats,
}

#[derive(Debug, Default)]
pub struct P2trIndexerState {
    next_uid: u64,
    outpoint_to_uid: HashMap<OutPointKey, u64>,
    live_uids: BTreeSet<u64>,
    seen_p2tr_keys: HashSet<Vec<u8>>,
}

impl P2trIndexerState {
    pub fn new() -> Self { Self::default() }

    pub fn last_uid(&self) -> u64 { self.next_uid }

    pub fn live_uids_sorted(&self) -> Vec<u64> {
        self.live_uids.iter().copied().collect()
    }

    /// Compatibility alias for earlier P2TR-only code.
    pub fn last_p2tr_uid(&self) -> u64 { self.last_uid() }

    /// Compatibility alias for earlier P2TR-only code.
    pub fn live_p2tr_uids_sorted(&self) -> Vec<u64> { self.live_uids_sorted() }

    /// Apply one block to the scoped UID state and return the corresponding
    /// `LightBlockInput` plus debug/statistical counters.
    ///
    /// Inputs are processed before outputs within each tx, matching Bitcoin
    /// spend semantics while still using an end-of-block UID anchor for
    /// same-block spends.
    pub fn apply_block(&mut self, block: BlockScanInput, profile: Profile) -> anyhow::Result<LightBlockInput> {
        Ok(self.apply_block_with_stats(block, profile)?.light_block)
    }

    pub fn apply_block_with_stats(&mut self, block: BlockScanInput, profile: Profile) -> anyhow::Result<AppliedBlock> {
        let mut outputs = Vec::<OutputRefInput>::new();
        let mut full_output_hashes = Vec::<OutputIdHash>::new();
        let mut spent_uids = Vec::<u64>::new();
        let mut tx_tweak_indexes = Vec::<u32>::new();
        let mut tx_tweaks = Vec::<TxTweak>::new();
        let mut stats = BlockScopeStats { tx_count: block.txs.len() as u32, ..Default::default() };

        for tx in &block.txs {
            let mut tx_has_p2tr_output = false;
            let mut tx_has_indexed_output = false;

            for input in &tx.inputs {
                if let Some(uid) = self.outpoint_to_uid.remove(&input.previous_output) {
                    self.live_uids.remove(&uid);
                    spent_uids.push(uid);
                    stats.indexed_spent_count += 1;
                }
            }

            for output in &tx.outputs {
                stats.output_count_total += 1;
                if output.is_p2tr {
                    tx_has_p2tr_output = true;
                    stats.p2tr_output_count += 1;
                    if output.is_nums {
                        // Kept for compatibility with the current DB column, but
                        // live block ingestion should not set this from outputs.
                        stats.p2tr_nums_count += 1;
                    }
                    // In output scope, every P2TR output is an SP scan candidate;
                    // input-side eligibility determines whether a tx scan point
                    // can be produced.
                    stats.p2tr_sp_candidate_count += 1;
                    if !self.seen_p2tr_keys.insert(output.identity_bytes.clone()) {
                        stats.p2tr_reused_count += 1;
                    }
                }
                let include = profile.scope.include_output(output.is_p2tr, output.is_nums);
                if !include {
                    if output.is_p2tr {
                        stats.p2tr_excluded_by_scope_count += 1;
                    }
                    continue;
                }

                tx_has_indexed_output = true;
                stats.indexed_output_count += 1;
                self.next_uid = self.next_uid.checked_add(1).ok_or_else(|| anyhow::anyhow!("scoped UID overflow"))?;
                let uid = self.next_uid;
                let outpoint = OutPointKey { txid: tx.txid, vout: output.vout };
                self.outpoint_to_uid.insert(outpoint, uid);
                self.live_uids.insert(uid);
                outputs.push(OutputRefInput { tx_index: tx.tx_index, vout: output.vout, uid });
                full_output_hashes.push(output_identifier_hash(&output.identity_bytes));
            }

            if tx_has_p2tr_output {
                stats.tx_with_p2tr_output_count += 1;
            }
            if tx_has_indexed_output {
                stats.tx_with_indexed_output_count += 1;
                if tx_has_p2tr_output {
                    // Do not fabricate scan data. If the caller cannot compute
                    // a correct BIP352 value from eligible inputs/prevouts, omit
                    // it. The real schema migration should replace this legacy
                    // 33-byte tx tweaks.
                    if let Some(tweak) = tx.silent_payment_tweak {
                        tx_tweak_indexes.push(tx.tx_index);
                        tx_tweaks.push(tweak);
                        stats.tweak_count += 1;
                    }
                }
            }
        }

        spent_uids.sort_unstable();
        let output_id_bytes = choose_output_id_bytes(outputs.len() as u64, OUTPUT_ID_COLLISION_PROBABILITY_LOG2).clamp(1, crate::index::MAX_P2TR_OUTPUT_ID_BYTES);
        let output_ids = truncate_into_packed(&full_output_hashes, output_id_bytes)?;

        Ok(AppliedBlock {
            light_block: LightBlockInput {
                height: block.height,
                block_hash: block.block_hash,
                previous_block_hash: block.previous_block_hash,
                block_anchor_last_uid: self.last_uid(),
                profile,
                output_id_bytes,
                tx_tweak_indexes,
                tx_tweaks,
                outputs,
                output_ids,
                spent_uids_sorted: spent_uids,
            },
            stats,
        })
    }
}

pub fn output_identifier_hash(identity_bytes: &[u8]) -> OutputIdHash {
    let mut h = Sha256::new();
    h.update(b"bitcoindata:light:p2tr-output-key");
    h.update(identity_bytes);
    let bytes: [u8; 32] = h.finalize().into();
    OutputIdHash::from(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txid(n: u8) -> TxidBytes { [n; 32].into() }

    #[test]
    fn p2tr_sp_includes_reuse_and_does_not_filter_output_nums() {
        let profile = Profile::default();
        let mut state = P2trIndexerState::new();
        let block = BlockScanInput {
            height: 1,
            block_hash: [1; 32].into(),
            previous_block_hash: [0; 32].into(),
            txs: vec![TxScanInput {
                txid: txid(10),
                tx_index: 0,
                inputs: vec![],
                silent_payment_tweak: Some([9; 33].into()),
                outputs: vec![
                    TxOutputScan { vout: 0, is_p2tr: true, is_nums: false, identity_bytes: vec![1; 32] },
                    TxOutputScan { vout: 1, is_p2tr: true, is_nums: false, identity_bytes: vec![1; 32] },
                    TxOutputScan { vout: 2, is_p2tr: true, is_nums: true, identity_bytes: vec![2; 32] },
                ],
            }],
        };
        let applied = state.apply_block_with_stats(block, profile).unwrap();
        assert_eq!(applied.light_block.outputs.len(), 3);
        assert_eq!(applied.light_block.outputs[0].uid, 1);
        assert_eq!(applied.light_block.outputs[1].uid, 2);
        assert_eq!(applied.light_block.outputs[2].uid, 3);
        assert_eq!(applied.stats.p2tr_nums_count, 1);
        assert_eq!(applied.stats.p2tr_reused_count, 1);
        assert_eq!(applied.stats.indexed_output_count, 3);
    }

    #[test]
    fn tracks_spends_in_scope() {
        let profile = Profile::default();
        let mut state = P2trIndexerState::new();
        let block1 = BlockScanInput {
            height: 1,
            block_hash: [1; 32].into(),
            previous_block_hash: [0; 32].into(),
            txs: vec![TxScanInput {
                txid: txid(10),
                tx_index: 0,
                inputs: vec![],
                silent_payment_tweak: Some([9; 33].into()),
                outputs: vec![
                    TxOutputScan { vout: 0, is_p2tr: false, is_nums: false, identity_bytes: vec![0] },
                    TxOutputScan { vout: 1, is_p2tr: true, is_nums: false, identity_bytes: vec![1; 32] },
                ],
            }],
        };
        let light1 = state.apply_block(block1, profile).unwrap();
        assert_eq!(light1.outputs.len(), 1);
        assert_eq!(light1.outputs[0].uid, 1);
        assert_eq!(light1.block_anchor_last_uid, 1);

        let block2 = BlockScanInput {
            height: 2,
            block_hash: [2; 32].into(),
            previous_block_hash: [1; 32].into(),
            txs: vec![TxScanInput {
                txid: txid(11),
                tx_index: 0,
                inputs: vec![TxInputScan { previous_output: OutPointKey { txid: txid(10), vout: 1 } }],
                silent_payment_tweak: Some([8; 33].into()),
                outputs: vec![TxOutputScan { vout: 0, is_p2tr: true, is_nums: false, identity_bytes: vec![2; 32] }],
            }],
        };
        let light2 = state.apply_block(block2, profile).unwrap();
        assert_eq!(light2.spent_uids_sorted, vec![1]);
        assert_eq!(light2.outputs[0].uid, 2);
        assert_eq!(state.live_uids_sorted(), vec![2]);
    }
}
