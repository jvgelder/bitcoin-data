//! JSON representation for the typed Cap'n Proto light block.
//!
//! The binary Cap'n Proto payload remains canonical. These helpers decode it
//! into stable human-readable JSON for debugging only.

use crate::index::{
    decode_light_block, decode_light_block_spent_ids, LightBlockInput, SpentIdCodec,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct JsonLightBlock {
    pub version: u16,
    pub height: u64,
    pub block_hash: String,
    pub previous_block_hash: String,
    pub first_uid: u64,
    pub tx_count: u16,
    pub skipped_txs_for_tweaks_bitmap: String,
    pub tweaks: Vec<JsonTweakEntry>,
    pub truncated_output_hash_bits: u8,
    pub truncated_output_hash_bytes: usize,
    pub truncated_output_hashes: String,
    pub spent_id_codec: &'static str,
    pub spent_count: u32,
    pub spent_id_bytes: usize,
    pub spent_ids: String,
    pub decoded_spent_ids: Vec<u64>,
}

#[derive(Debug, Serialize)]
pub struct JsonTweakEntry {
    pub output_count: u16,
    pub tweak: String,
}

pub fn light_block_to_json(bytes: &[u8]) -> anyhow::Result<JsonLightBlock> {
    light_block_input_to_json(decode_light_block(bytes)?)
}

fn light_block_input_to_json(input: LightBlockInput) -> anyhow::Result<JsonLightBlock> {
    let truncated_output_hash_bytes = input.truncated_output_hashes.len();
    let spent_id_bytes = input.spent_ids.len();
    let decoded_spends = decode_light_block_spent_ids(&input)?;
    Ok(JsonLightBlock {
        version: crate::WIRE_VERSION,
        height: input.height,
        block_hash: hex::encode(input.block_hash.as_bytes()),
        previous_block_hash: hex::encode(input.previous_block_hash.as_bytes()),
        first_uid: input.first_uid,
        tx_count: input.tx_count,
        skipped_txs_for_tweaks_bitmap: hex::encode(input.skipped_txs_for_tweaks),
        tweaks: input
            .tweaks
            .into_iter()
            .map(|entry| JsonTweakEntry {
                output_count: entry.output_count,
                tweak: hex::encode(&entry.tweak.as_bytes()[1..]),
            })
            .collect(),
        truncated_output_hash_bits: input.truncated_output_hash_bits,
        truncated_output_hash_bytes,
        truncated_output_hashes: hex::encode(input.truncated_output_hashes),
        spent_id_codec: spent_id_codec_name(input.spent_id_codec),
        spent_count: input.spent_count,
        spent_id_bytes,
        spent_ids: hex::encode(input.spent_ids),
        decoded_spent_ids: decoded_spends,
    })
}

fn spent_id_codec_name(codec: SpentIdCodec) -> &'static str {
    match codec {
        SpentIdCodec::EliasDeltaAscendingAbsolute => "eliasDeltaAscendingAbsolute",
    }
}
