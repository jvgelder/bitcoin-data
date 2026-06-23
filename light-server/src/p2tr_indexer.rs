//! P2TR/SP archive builder primitives.
//!
//! This module is source-agnostic. A Bitcoin Core REST/RPC adapter can decode
//! blocks and feed `BlockScanInput` values here. The archive currently has one
//! fixed scope: every P2TR output receives a UID. Reused keys are included.
//! NUMS is handled on input-side BIP352 spend eligibility, not output creation.

use crate::index::{
    LightBlockInput, OutputEntryInput, SpendEntryInput, StoredLightBlockInput,
    StoredOutputEntryInput, StoredSpendEntryInput, StoredTweakEntryInput, TweakEntryInput,
    STORAGE_OUTPUT_FLAG_REUSED, STORAGE_SPENT_HEIGHT_UNSPENT,
};
use crate::types::{BlockHashBytes, TxTweak, TxidBytes};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutPointKey {
    pub txid: TxidBytes,
    pub vout: u32,
}

impl OutPointKey {
    pub fn is_coinbase(&self) -> bool {
        self.vout == u32::MAX && self.txid.as_bytes() == &[0u8; 32]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpendLookup {
    pub uid: u64,
    pub creation_height: u64,
    pub output_index: u32,
    pub flags: u8,
}

#[derive(Debug, Clone)]
pub struct ScopedUtxoEntry {
    /// Canonical-chain outpoint key for spend lookup. A tx can appear in
    /// competing forks, so this row is only valid for the currently indexed
    /// best-work chain. Reorg rollback must disconnect affected block effects
    /// before re-indexing the replacement chain.
    pub outpoint: OutPointKey,
    pub uid: u64,
    pub created_height: u64,
    pub created_block_hash: BlockHashBytes,
    pub tx_index: u32,
    /// Dense index in the stored output list for this creation block.
    pub output_index: u32,
    pub value_sat: u64,
    pub script_pubkey: Vec<u8>,
    /// Required for the p2tr-sp scope: this is the previous-output key needed
    /// later for BIP352 input scan-point construction. Stored as the raw 32-byte
    /// x-only program: a Taproot output key is only an identifier here and is
    /// not required to be a valid curve point (consensus permits non-point
    /// outputs), so it must never be eagerly parsed into an `XOnlyPublicKey`.
    pub p2tr_xonly_key: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct CreatedScopedUtxo {
    pub entry: ScopedUtxoEntry,
    pub is_nums: bool,
    pub is_reused: bool,
    pub reuse_count_at_creation: u64,
}

#[derive(Debug, Clone)]
pub struct SpentScopedUtxo {
    pub outpoint: OutPointKey,
    pub uid: u64,
    pub creation_height: u64,
    pub output_index: u32,
    pub flags: u8,
    pub spent_height: u64,
    pub spent_block_hash: BlockHashBytes,
    pub spend_tx_index: u32,
}

#[derive(Debug, Clone)]
pub struct TxInputScan {
    pub previous_output: OutPointKey,
    /// Raw scriptSig bytes from the spending input. Needed by BIP352 input
    /// eligibility for legacy/P2SH-wrapped input forms.
    pub script_sig: Vec<u8>,
    /// Raw witness stack items from the spending input. Needed to extract
    /// input public keys for SegWit/Taproot forms.
    pub witness: Vec<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct TxOutputScan {
    pub vout: u32,
    pub value_sat: u64,
    pub script_pubkey: Vec<u8>,
    pub p2tr_xonly_key: Option<[u8; 32]>,
    pub is_p2tr: bool,
    /// Input-side NUMS accounting hook. For real Bitcoin output scanning this
    /// should normally be false: BIP352 NUMS handling applies to Taproot
    /// script-path spends, not to P2TR output creation.
    pub is_nums: bool,
}

#[derive(Debug, Clone)]
pub struct TxScanInput {
    pub txid: TxidBytes,
    pub tx_index: u32,
    pub inputs: Vec<TxInputScan>,
    pub outputs: Vec<TxOutputScan>,
    /// BIP352/Blindbit per-transaction tweak. The served block stores its 32-byte x-coordinate.
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
    /// Compact client/server response shape.
    pub light_block: LightBlockInput,
    /// Rich file-archive storage shape. The server decodes this and serializes
    /// `light_block` responses from it.
    pub storage_block: StoredLightBlockInput,
    pub stats: BlockScopeStats,
    pub created_utxos: Vec<CreatedScopedUtxo>,
    pub spent_utxos: Vec<SpentScopedUtxo>,
}

#[derive(Debug, Default)]
pub struct P2trIndexerState {
    next_uid: u64,
    /// Minimal spend index. A Bitcoin input only gives us `(txid, vout)`, and
    /// the light payload only needs the corresponding UID for spent-ID output.
    /// Full output metadata is carried by `CreatedScopedUtxo` until it is
    /// flushed to the archive/index store; it is not retained in the long-lived state.
    outpoint_to_spend: HashMap<OutPointKey, SpendLookup>,
    /// Counts live indexed outputs, including historical DB outputs restored by
    /// count only. Historical outputs are not materialized in `outpoint_to_uid`
    /// unless a chunk may spend them.
    live_uid_count: usize,
    seen_p2tr_keys: Option<HashSet<[u8; 32]>>,
}

impl P2trIndexerState {
    pub fn new() -> Self {
        Self {
            next_uid: 0,
            outpoint_to_spend: HashMap::new(),
            live_uid_count: 0,
            seen_p2tr_keys: Some(HashSet::new()),
        }
    }

    pub fn restore(
        last_uid: u64,
        live_entries: impl IntoIterator<Item = ScopedUtxoEntry>,
        seen_keys: impl IntoIterator<Item = [u8; 32]>,
    ) -> Self {
        let mut state = Self {
            next_uid: last_uid,
            outpoint_to_spend: HashMap::new(),
            live_uid_count: 0,
            seen_p2tr_keys: Some(seen_keys.into_iter().collect()),
        };
        for entry in live_entries {
            state.live_uid_count += 1;
            state.outpoint_to_spend.insert(
                entry.outpoint,
                SpendLookup {
                    uid: entry.uid,
                    creation_height: entry.created_height,
                    output_index: entry.output_index,
                    flags: 0,
                },
            );
        }
        state
    }

    pub fn restore_without_reuse_tracking(
        last_uid: u64,
        live_entries: impl IntoIterator<Item = ScopedUtxoEntry>,
    ) -> Self {
        let mut state = Self {
            next_uid: last_uid,
            outpoint_to_spend: HashMap::new(),
            live_uid_count: 0,
            seen_p2tr_keys: None,
        };
        for entry in live_entries {
            state.live_uid_count += 1;
            state.outpoint_to_spend.insert(
                entry.outpoint,
                SpendLookup {
                    uid: entry.uid,
                    creation_height: entry.created_height,
                    output_index: entry.output_index,
                    flags: 0,
                },
            );
        }
        state
    }

    pub fn restore_counts_only(last_uid: u64, live_uid_count: usize) -> Self {
        Self {
            next_uid: last_uid,
            outpoint_to_spend: HashMap::new(),
            live_uid_count,
            seen_p2tr_keys: None,
        }
    }

    pub fn restore_at_uid(last_uid: u64) -> Self {
        Self {
            next_uid: last_uid,
            outpoint_to_spend: HashMap::new(),
            live_uid_count: 0,
            seen_p2tr_keys: None,
        }
    }

    pub fn contains_outpoint(&self, outpoint: &OutPointKey) -> bool {
        self.outpoint_to_spend.contains_key(outpoint)
    }

    pub fn cached_outpoint_count(&self) -> usize {
        self.outpoint_to_spend.len()
    }

    /// Cache historical spend candidates loaded from RocksDB for the current
    /// decoded chunk. This intentionally stores only `OutPointKey -> uid`; value,
    /// script, creation height/hash and output key are not needed for spend UID
    /// accounting. Returned outpoints can be evicted after the chunk is applied.
    pub fn cache_spend_uid_candidates(
        &mut self,
        candidates: impl IntoIterator<Item = (OutPointKey, SpendLookup)>,
    ) -> Vec<OutPointKey> {
        let mut cached = Vec::new();
        for (outpoint, spend) in candidates {
            if let std::collections::hash_map::Entry::Vacant(slot) =
                self.outpoint_to_spend.entry(outpoint)
            {
                slot.insert(spend);
                cached.push(outpoint);
            }
        }
        cached
    }

    /// Remove unspent historical candidates after a chunk. If a candidate was
    /// actually spent, `apply_block_with_stats` has already removed it and
    /// decremented `live_uid_count`; this method only drops lookup-only cache
    /// entries and must not adjust the live count.
    pub fn evict_spend_uid_candidates(
        &mut self,
        cached_outpoints: impl IntoIterator<Item = OutPointKey>,
    ) -> usize {
        let mut evicted = 0usize;
        for outpoint in cached_outpoints {
            if self.outpoint_to_spend.remove(&outpoint).is_some() {
                evicted += 1;
            }
        }
        evicted
    }

    pub fn live_uid_count(&self) -> usize {
        self.live_uid_count
    }

    pub fn last_uid(&self) -> u64 {
        self.next_uid
    }

    pub fn live_uids_sorted(&self) -> Vec<u64> {
        let mut uids = self
            .outpoint_to_spend
            .values()
            .map(|spend| spend.uid)
            .collect::<Vec<_>>();
        uids.sort_unstable();
        uids
    }

    /// Compatibility alias for earlier P2TR-only code.
    pub fn last_p2tr_uid(&self) -> u64 {
        self.last_uid()
    }

    /// Compatibility alias for earlier P2TR-only code.
    pub fn live_p2tr_uids_sorted(&self) -> Vec<u64> {
        self.live_uids_sorted()
    }

    /// Apply one block to the scoped UID state and return the corresponding
    /// `LightBlockInput` plus debug/statistical counters.
    ///
    /// Inputs are processed before outputs within each tx, matching Bitcoin
    /// spend semantics while still using an end-of-block UID anchor for
    /// same-block spends.
    pub fn apply_block(&mut self, block: BlockScanInput) -> anyhow::Result<LightBlockInput> {
        Ok(self.apply_block_with_stats(block)?.light_block)
    }

    pub fn apply_block_with_stats(
        &mut self,
        block: BlockScanInput,
    ) -> anyhow::Result<AppliedBlock> {
        let block_first_uid = self.next_uid.saturating_add(1);
        let mut output_entries = Vec::<OutputEntryInput>::new();
        let mut storage_output_entries = Vec::<StoredOutputEntryInput>::new();
        let mut spends = Vec::<SpendEntryInput>::new();
        let mut storage_spends = Vec::<StoredSpendEntryInput>::new();
        let mut skipped_txs_for_tweaks = Vec::<u16>::new();
        let mut tx_tweaks = Vec::<TweakEntryInput>::new();
        let mut storage_tx_tweaks = Vec::<StoredTweakEntryInput>::new();
        let mut storage_skipped_outputs = Vec::<u16>::new();
        let mut stats = BlockScopeStats {
            tx_count: block.txs.len() as u32,
            ..Default::default()
        };
        let mut created_utxos = Vec::<CreatedScopedUtxo>::new();
        let mut spent_utxos = Vec::<SpentScopedUtxo>::new();

        for tx in &block.txs {
            let mut tx_has_p2tr_output = false;
            let mut tx_indexed_output_count: u16 = 0;

            for input in &tx.inputs {
                if let Some(spend_ref) = self.outpoint_to_spend.remove(&input.previous_output) {
                    self.live_uid_count = self.live_uid_count.saturating_sub(1);
                    spends.push(SpendEntryInput {
                        spent_uid: spend_ref.uid,
                    });
                    storage_spends.push(StoredSpendEntryInput {
                        spent_uid: spend_ref.uid,
                        creation_height: u32::try_from(spend_ref.creation_height)?,
                    });
                    spent_utxos.push(SpentScopedUtxo {
                        outpoint: input.previous_output,
                        uid: spend_ref.uid,
                        creation_height: spend_ref.creation_height,
                        output_index: spend_ref.output_index,
                        flags: spend_ref.flags,
                        spent_height: block.height,
                        spent_block_hash: block.block_hash,
                        spend_tx_index: tx.tx_index,
                    });
                    stats.indexed_spent_count += 1;
                }
            }

            for output in &tx.outputs {
                stats.output_count_total += 1;

                if !output.is_p2tr {
                    continue;
                }

                let current_p2tr_slot = stats.p2tr_output_count;
                let current_p2tr_slot_u16 = u16::try_from(current_p2tr_slot)?;
                stats.p2tr_output_count = stats
                    .p2tr_output_count
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("P2TR output count overflow"))?;

                let p2tr_xonly_key = p2tr_output_identity(output, block.height, tx.tx_index)?;
                tx_has_p2tr_output = true;
                if output.is_nums {
                    stats.p2tr_nums_count += 1;
                }
                stats.p2tr_sp_candidate_count += 1;

                let mut is_reused_output = false;
                if let Some(seen_p2tr_keys) = self.seen_p2tr_keys.as_mut() {
                    is_reused_output = !seen_p2tr_keys.insert(p2tr_xonly_key);
                    if is_reused_output {
                        stats.p2tr_reused_count += 1;
                    }
                }

                // Every native P2TR output consumes a UID slot. Statically
                // omitted outputs stay out of the dense output list; the client
                // can recover the UID after a match by counting P2TR outputs in
                // the full block up to the matched outpoint.
                if output.is_nums || tx.silent_payment_tweak.is_none() {
                    storage_skipped_outputs.push(current_p2tr_slot_u16);
                    stats.p2tr_excluded_by_scope_count += 1;
                    continue;
                }

                stats.indexed_output_count += 1;
                tx_indexed_output_count = tx_indexed_output_count
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("tx indexed output count exceeds u16"))?;
                let uid = block_first_uid
                    .checked_add(u64::from(current_p2tr_slot))
                    .ok_or_else(|| anyhow::anyhow!("scoped UID overflow"))?;
                let outpoint = OutPointKey {
                    txid: tx.txid,
                    vout: output.vout,
                };
                let flags = if is_reused_output {
                    STORAGE_OUTPUT_FLAG_REUSED
                } else {
                    0
                };
                let output_index = u32::try_from(storage_output_entries.len())?;
                let entry = ScopedUtxoEntry {
                    outpoint,
                    uid,
                    created_height: block.height,
                    created_block_hash: block.block_hash,
                    tx_index: tx.tx_index,
                    output_index,
                    value_sat: output.value_sat,
                    script_pubkey: output.script_pubkey.clone(),
                    p2tr_xonly_key,
                };
                self.outpoint_to_spend.insert(
                    outpoint,
                    SpendLookup {
                        uid,
                        creation_height: block.height,
                        output_index,
                        flags,
                    },
                );
                self.live_uid_count += 1;
                created_utxos.push(CreatedScopedUtxo {
                    entry,
                    is_nums: output.is_nums,
                    is_reused: is_reused_output,
                    reuse_count_at_creation: 1,
                });
                output_entries.push(OutputEntryInput {
                    key: p2tr_xonly_key,
                });
                storage_output_entries.push(StoredOutputEntryInput {
                    key: p2tr_xonly_key,
                    spent_height: STORAGE_SPENT_HEIGHT_UNSPENT,
                    flags,
                });
            }

            if tx_has_p2tr_output {
                stats.tx_with_p2tr_output_count += 1;
            }
            if tx_indexed_output_count > 0 {
                stats.tx_with_indexed_output_count += 1;
                if let Some(tweak) = tx.silent_payment_tweak {
                    tx_tweaks.push(TweakEntryInput {
                        output_count: tx_indexed_output_count,
                        tweak,
                    });
                    storage_tx_tweaks.push(StoredTweakEntryInput {
                        output_count: tx_indexed_output_count,
                        tweak,
                    });
                    stats.tweak_count += 1;
                }
            } else if tx_has_p2tr_output {
                skipped_txs_for_tweaks.push(u16::try_from(tx.tx_index)?);
            }
        }

        if stats.p2tr_output_count > 0 {
            self.next_uid = block_first_uid
                .checked_add(u64::from(stats.p2tr_output_count))
                .and_then(|next_after_block| next_after_block.checked_sub(1))
                .ok_or_else(|| anyhow::anyhow!("scoped UID overflow"))?;
        }

        Ok(AppliedBlock {
            light_block: LightBlockInput {
                height: block.height,
                block_hash: block.block_hash,
                previous_block_hash: block.previous_block_hash,
                first_uid: block_first_uid,
                skipped_txs_for_tweaks: skipped_txs_for_tweaks.clone(),
                tweaks: tx_tweaks.clone(),
                outputs: output_entries,
                spends,
            },
            storage_block: StoredLightBlockInput {
                height: block.height,
                block_hash: block.block_hash,
                previous_block_hash: block.previous_block_hash,
                first_uid: block_first_uid,
                skipped_txs_for_tweaks,
                tweaks: storage_tx_tweaks,
                skipped_outputs: storage_skipped_outputs,
                outputs: storage_output_entries,
                spends: storage_spends,
            },
            stats,
            created_utxos,
            spent_utxos,
        })
    }
}

fn p2tr_output_identity(
    output: &TxOutputScan,
    height: u64,
    tx_index: u32,
) -> anyhow::Result<[u8; 32]> {
    output.p2tr_xonly_key.ok_or_else(|| {
        anyhow::anyhow!(
            "P2TR output at height {height} tx_index {tx_index} vout {} is missing x-only key",
            output.vout
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txid(n: u8) -> TxidBytes {
        [n; 32].into()
    }

    fn xonly_key(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn p2tr_sp_counts_nums_slots_but_skips_nums_outputs() {
        let mut state = P2trIndexerState::new();
        let block = BlockScanInput {
            height: 1,
            block_hash: [1; 32].into(),
            previous_block_hash: [0; 32].into(),
            txs: vec![TxScanInput {
                txid: txid(10),
                tx_index: 0,
                inputs: vec![],
                silent_payment_tweak: Some([9u8; 33].into()),
                outputs: vec![
                    TxOutputScan {
                        vout: 0,
                        value_sat: 0,
                        script_pubkey: Vec::new(),
                        p2tr_xonly_key: Some(xonly_key(1)),
                        is_p2tr: true,
                        is_nums: false,
                    },
                    TxOutputScan {
                        vout: 1,
                        value_sat: 0,
                        script_pubkey: Vec::new(),
                        p2tr_xonly_key: Some(xonly_key(1)),
                        is_p2tr: true,
                        is_nums: false,
                    },
                    TxOutputScan {
                        vout: 2,
                        value_sat: 0,
                        script_pubkey: Vec::new(),
                        p2tr_xonly_key: Some(xonly_key(2)),
                        is_p2tr: true,
                        is_nums: true,
                    },
                ],
            }],
        };
        let applied = state.apply_block_with_stats(block).unwrap();
        assert_eq!(applied.light_block.outputs.len(), 2);
        assert_eq!(applied.light_block.first_uid, 1);
        assert_eq!(applied.light_block.outputs[0].key, xonly_key(1));
        assert_eq!(applied.created_utxos[1].is_reused, true);
        assert_eq!(
            applied.storage_block.outputs[1].flags & STORAGE_OUTPUT_FLAG_REUSED,
            STORAGE_OUTPUT_FLAG_REUSED
        );
        assert_eq!(applied.storage_block.skipped_outputs, vec![2]);
        assert_eq!(applied.stats.p2tr_nums_count, 1);
        assert_eq!(applied.stats.p2tr_reused_count, 1);
        assert_eq!(applied.stats.indexed_output_count, 2);
    }

    #[test]
    fn tracks_spends_in_scope() {
        let mut state = P2trIndexerState::new();
        let block1 = BlockScanInput {
            height: 1,
            block_hash: [1; 32].into(),
            previous_block_hash: [0; 32].into(),
            txs: vec![TxScanInput {
                txid: txid(10),
                tx_index: 0,
                inputs: vec![],
                silent_payment_tweak: Some([9u8; 33].into()),
                outputs: vec![
                    TxOutputScan {
                        vout: 0,
                        value_sat: 0,
                        script_pubkey: Vec::new(),
                        p2tr_xonly_key: None,
                        is_p2tr: false,
                        is_nums: false,
                    },
                    TxOutputScan {
                        vout: 1,
                        value_sat: 0,
                        script_pubkey: Vec::new(),
                        p2tr_xonly_key: Some(xonly_key(1)),
                        is_p2tr: true,
                        is_nums: false,
                    },
                ],
            }],
        };
        let light1 = state.apply_block(block1).unwrap();
        assert_eq!(light1.outputs.len(), 1);
        assert_eq!(light1.first_uid, 1);

        let block2 = BlockScanInput {
            height: 2,
            block_hash: [2; 32].into(),
            previous_block_hash: [1; 32].into(),
            txs: vec![TxScanInput {
                txid: txid(11),
                tx_index: 0,
                inputs: vec![TxInputScan {
                    previous_output: OutPointKey {
                        txid: txid(10),
                        vout: 1,
                    },
                    script_sig: Vec::new(),
                    witness: Vec::new(),
                }],
                silent_payment_tweak: Some([8u8; 33].into()),
                outputs: vec![TxOutputScan {
                    vout: 0,
                    value_sat: 0,
                    script_pubkey: Vec::new(),
                    p2tr_xonly_key: Some(xonly_key(2)),
                    is_p2tr: true,
                    is_nums: false,
                }],
            }],
        };
        let light2 = state.apply_block(block2).unwrap();
        assert_eq!(
            light2
                .spends
                .iter()
                .map(|s| s.spent_uid)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(light2.first_uid, 2);
        assert_eq!(state.live_uids_sorted(), vec![2]);
    }
}
