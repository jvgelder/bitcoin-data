use crate::codec::elias_delta::encode_elias_delta_values;
use crate::light_capnp::{
    light_block, output_ref, snapshot_block, snapshot_output_ref, SpentIdCodec,
};
use crate::profile::Profile;
use crate::types::{BlockHashBytes, TxTweak};
use crate::WIRE_VERSION;
use capnp::message::{Builder, HeapAllocator};

/// Bound used by the tx-tweak index stream. The format is scope-based and uses
/// one fixed tx-index codec, so carrying a codec selector is unnecessary.
pub const MAX_TX_TWEAK_INDEX_DOMAIN: u32 = 25_000;
/// Conservative upper bound for scope outputs in one block.
pub const MAX_P2TR_OUTPUTS_PER_BLOCK: usize = 25_000;
/// Scope output IDs should not need more than seven bytes for P2TR-oriented scopes.
pub const MAX_P2TR_OUTPUT_ID_BYTES: u8 = 7;

#[derive(Debug, Clone)]
pub struct OutputRefInput {
    pub tx_index: u32,
    pub vout: u32,
    pub uid: u64,
}

#[derive(Debug, Clone)]
pub struct LightBlockInput {
    pub height: u64,
    pub block_hash: BlockHashBytes,
    pub previous_block_hash: BlockHashBytes,
    pub block_anchor_last_uid: u64,
    pub profile: Profile,
    pub output_id_bytes: u8,
    pub tx_tweak_indexes: Vec<u32>,
    pub tx_tweaks: Vec<TxTweak>,
    pub outputs: Vec<OutputRefInput>,
    pub output_ids: Vec<u8>,
    pub spent_uids_sorted: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct SnapshotOutputRefInput {
    pub vout: u32,
    pub uid: u64,
}

#[derive(Debug, Clone)]
pub struct SnapshotTxInput {
    pub tx_index: u32,
    pub tweak: TxTweak,
    pub outputs: Vec<SnapshotOutputRefInput>,
    pub output_ids: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct SnapshotBlockInput {
    pub height: u64,
    pub block_hash: BlockHashBytes,
    pub previous_block_hash: BlockHashBytes,
    pub block_anchor_last_uid: u64,
    pub output_id_bytes: u8,
    pub txs: Vec<SnapshotTxInput>,
}


pub fn encode_spent_uids(
    block_anchor_last_uid: u64,
    spent_sorted: &[u64],
) -> anyhow::Result<Vec<u8>> {
    if spent_sorted.is_empty() {
        return Ok(Vec::new());
    }
    validate_sorted_unique(spent_sorted)?;
    anyhow::ensure!(
        spent_sorted[0] <= block_anchor_last_uid,
        "spent uid exceeds block anchor"
    );
    let mut values = Vec::with_capacity(spent_sorted.len());
    values.push(block_anchor_last_uid - spent_sorted[0] + 1);
    for pair in spent_sorted.windows(2) {
        values.push(pair[1] - pair[0]);
    }
    encode_elias_delta_values(&values)
}

pub fn decode_spent_uids(
    block_anchor_last_uid: u64,
    encoded: &[u8],
    count: usize,
) -> anyhow::Result<Vec<u64>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let values = crate::codec::elias_delta::decode_elias_delta_values(encoded, count)?;
    let first = block_anchor_last_uid
        .checked_sub(values[0] - 1)
        .ok_or_else(|| anyhow::anyhow!("invalid first spent uid offset"))?;
    let mut out = Vec::with_capacity(count);
    out.push(first);
    for delta in &values[1..] {
        let next = out
            .last()
            .unwrap()
            .checked_add(*delta)
            .ok_or_else(|| anyhow::anyhow!("spent uid delta overflow"))?;
        out.push(next);
    }
    Ok(out)
}

pub fn encode_tx_tweak_indexes(indexes: &[u32]) -> anyhow::Result<Vec<u8>> {
    if indexes.is_empty() {
        return Ok(Vec::new());
    }
    validate_sorted_unique_u32(indexes)?;
    for &index in indexes {
        anyhow::ensure!(
            index < MAX_TX_TWEAK_INDEX_DOMAIN,
            "tx tweak index {index} exceeds tx-index domain bound {MAX_TX_TWEAK_INDEX_DOMAIN}"
        );
    }
    let mut values = Vec::with_capacity(indexes.len());
    values.push(indexes[0] as u64 + 1);
    for pair in indexes.windows(2) {
        values.push((pair[1] - pair[0]) as u64);
    }
    encode_elias_delta_values(&values)
}

pub fn decode_tx_tweak_indexes(encoded: &[u8], count: usize) -> anyhow::Result<Vec<u32>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let values = crate::codec::elias_delta::decode_elias_delta_values(encoded, count)?;
    let first = values[0] - 1;
    anyhow::ensure!(first <= u32::MAX as u64, "tx index overflow");
    let mut out = Vec::with_capacity(count);
    anyhow::ensure!(
        (first as u32) < MAX_TX_TWEAK_INDEX_DOMAIN,
        "tx index exceeds tx-index domain bound"
    );
    out.push(first as u32);
    for delta in &values[1..] {
        let next = u64::from(*out.last().unwrap()) + *delta;
        anyhow::ensure!(next <= u32::MAX as u64, "tx index overflow");
        anyhow::ensure!(
            (next as u32) < MAX_TX_TWEAK_INDEX_DOMAIN,
            "tx index exceeds tx-index domain bound"
        );
        out.push(next as u32);
    }
    Ok(out)
}

pub fn encode_light_block(input: &LightBlockInput) -> anyhow::Result<Builder<HeapAllocator>> {
    anyhow::ensure!(
        input.tx_tweak_indexes.len() == input.tx_tweaks.len(),
        "tx tweak indexes/tweaks length mismatch"
    );
    anyhow::ensure!(
        input.tx_tweaks.len() <= u32::MAX as usize,
        "too many tweaks"
    );
    // Each served Silent Payments tweak is a 33-byte compressed public key.
    // The name intentionally follows Blindbit/light-client terminology: it is
    // public point input_hash*A, not a 32-byte scalar.
    anyhow::ensure!(
        input.outputs.len() <= MAX_P2TR_OUTPUTS_PER_BLOCK,
        "too many scope outputs in one block"
    );
    anyhow::ensure!(
        input.output_id_bytes <= MAX_P2TR_OUTPUT_ID_BYTES,
        "scope output_id_bytes exceeds configured bound"
    );
    let output_id_bytes = input.output_id_bytes as usize;
    anyhow::ensure!(output_id_bytes > 0, "output_id_bytes must be non-zero");
    anyhow::ensure!(
        input.output_ids.len() == input.outputs.len() * output_id_bytes,
        "packed output id length mismatch"
    );
    for output in &input.outputs {
        anyhow::ensure!(
            output.uid <= input.block_anchor_last_uid,
            "output UID exceeds block anchor"
        );
        anyhow::ensure!(
            output.tx_index < MAX_TX_TWEAK_INDEX_DOMAIN,
            "output tx index exceeds tx-index domain bound"
        );
    }
    for &uid in &input.spent_uids_sorted {
        anyhow::ensure!(
            uid <= input.block_anchor_last_uid,
            "spent UID exceeds block anchor"
        );
    }

    let tx_tweak_indexes = encode_tx_tweak_indexes(&input.tx_tweak_indexes)?;
    let mut tx_tweaks = Vec::with_capacity(input.tx_tweaks.len() * TxTweak::LEN);
    for tweak in &input.tx_tweaks {
        tx_tweaks.extend_from_slice(tweak.as_bytes());
    }
    let spent = encode_spent_uids(input.block_anchor_last_uid, &input.spent_uids_sorted)?;

    let mut msg = Builder::new_default();
    {
        let mut b = msg.init_root::<light_block::Builder>();
        b.set_version(WIRE_VERSION);
        b.set_height(input.height);
        b.set_block_hash(input.block_hash.as_bytes());
        b.set_previous_block_hash(input.previous_block_hash.as_bytes());
        b.set_block_anchor_last_uid(input.block_anchor_last_uid);
        input.profile.fill_capnp(b.reborrow().init_profile());
        b.set_output_id_bytes(input.output_id_bytes);
        b.set_tweak_count(input.tx_tweaks.len() as u32);
        b.set_tx_tweak_indexes(&tx_tweak_indexes);
        b.set_tx_tweaks(&tx_tweaks);
        {
            let mut outs = b.reborrow().init_outputs(input.outputs.len() as u32);
            for (i, src) in input.outputs.iter().enumerate() {
                fill_output_ref(outs.reborrow().get(i as u32), src);
            }
        }
        b.set_output_ids(&input.output_ids);
        b.set_spent_id_codec(SpentIdCodec::EliasDeltaSorted);
        b.set_spent_count(input.spent_uids_sorted.len() as u32);
        b.set_spent_ids(&spent);
    }
    Ok(msg)
}

pub fn encode_snapshot_block(input: &SnapshotBlockInput) -> anyhow::Result<Builder<HeapAllocator>> {
    anyhow::ensure!(
        input.output_id_bytes <= MAX_P2TR_OUTPUT_ID_BYTES,
        "snapshot output_id_bytes exceeds configured bound"
    );
    let output_id_bytes = input.output_id_bytes as usize;
    anyhow::ensure!(output_id_bytes > 0, "output_id_bytes must be non-zero");

    let mut total_outputs = 0usize;
    let mut last_tx_index = None;
    for tx in &input.txs {
        anyhow::ensure!(
            tx.tx_index < MAX_TX_TWEAK_INDEX_DOMAIN,
            "snapshot tx index exceeds tx-index domain bound"
        );
        if let Some(prev) = last_tx_index {
            anyhow::ensure!(prev < tx.tx_index, "snapshot txs must be sorted and unique");
        }
        last_tx_index = Some(tx.tx_index);
        anyhow::ensure!(
            tx.output_ids.len() == tx.outputs.len() * output_id_bytes,
            "snapshot tx packed output id length mismatch"
        );
        let mut last_vout = None;
        for output in &tx.outputs {
            if let Some(prev) = last_vout {
                anyhow::ensure!(
                    prev < output.vout,
                    "snapshot outputs must be sorted by vout"
                );
            }
            last_vout = Some(output.vout);
            anyhow::ensure!(
                output.uid <= input.block_anchor_last_uid,
                "snapshot output UID exceeds block anchor"
            );
        }
        total_outputs += tx.outputs.len();
    }
    anyhow::ensure!(
        total_outputs <= MAX_P2TR_OUTPUTS_PER_BLOCK,
        "too many snapshot outputs in one block"
    );

    let mut msg = Builder::new_default();
    {
        let mut b = msg.init_root::<snapshot_block::Builder>();
        b.set_version(WIRE_VERSION);
        b.set_height(input.height);
        b.set_block_hash(input.block_hash.as_bytes());
        b.set_previous_block_hash(input.previous_block_hash.as_bytes());
        b.set_block_anchor_last_uid(input.block_anchor_last_uid);
        b.set_output_id_bytes(input.output_id_bytes);
        let mut txs = b.reborrow().init_txs(input.txs.len() as u32);
        for (tx_idx, src_tx) in input.txs.iter().enumerate() {
            let mut dst_tx = txs.reborrow().get(tx_idx as u32);
            dst_tx.set_tx_index(src_tx.tx_index);
            dst_tx.set_tweak(src_tx.tweak.as_bytes());
            {
                let mut outs = dst_tx.reborrow().init_outputs(src_tx.outputs.len() as u32);
                for (i, src) in src_tx.outputs.iter().enumerate() {
                    fill_snapshot_output_ref(outs.reborrow().get(i as u32), src);
                }
            }
            dst_tx.set_output_ids(&src_tx.output_ids);
        }
    }
    Ok(msg)
}

fn fill_snapshot_output_ref(mut b: snapshot_output_ref::Builder<'_>, src: &SnapshotOutputRefInput) {
    b.set_vout(src.vout);
    b.set_uid(src.uid);
}

fn fill_output_ref(mut b: output_ref::Builder<'_>, src: &OutputRefInput) {
    b.set_tx_index(src.tx_index);
    b.set_vout(src.vout);
    b.set_uid(src.uid);
}



fn validate_sorted_unique(values: &[u64]) -> anyhow::Result<()> {
    for pair in values.windows(2) {
        anyhow::ensure!(
            pair[0] < pair[1],
            "values must be sorted ascending and unique"
        );
    }
    Ok(())
}

fn validate_sorted_unique_u32(values: &[u32]) -> anyhow::Result<()> {
    for pair in values.windows(2) {
        anyhow::ensure!(
            pair[0] < pair[1],
            "values must be sorted ascending and unique"
        );
    }
    Ok(())
}

pub fn to_packed_bytes(msg: &Builder<HeapAllocator>) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    capnp::serialize_packed::write_message(&mut out, msg)?;
    Ok(out)
}

pub fn to_bytes(msg: &Builder<HeapAllocator>) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    capnp::serialize::write_message(&mut out, msg)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spent_uid_roundtrip_uses_end_of_block_anchor() {
        let anchor = 100;
        let spent = vec![3, 10, 98, 100];
        let encoded = encode_spent_uids(anchor, &spent).unwrap();
        let decoded = decode_spent_uids(anchor, &encoded, spent.len()).unwrap();
        assert_eq!(decoded, spent);
    }

    #[test]
    fn tx_tweak_index_roundtrip_fixed_elias_delta() {
        let indexes = vec![2, 23, 2000];
        let encoded = encode_tx_tweak_indexes(&indexes).unwrap();
        let decoded = decode_tx_tweak_indexes(&encoded, indexes.len()).unwrap();
        assert_eq!(decoded, indexes);
    }
}

#[cfg(test)]
mod tx_index_bound_tests {
    use super::*;

    #[test]
    fn tx_tweak_index_rejects_out_of_domain_value() {
        let err = encode_tx_tweak_indexes(&[MAX_TX_TWEAK_INDEX_DOMAIN]).unwrap_err();
        assert!(err.to_string().contains("exceeds tx-index domain bound"));
    }
}
