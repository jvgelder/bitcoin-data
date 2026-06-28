//! JSON representation for the typed Cap'n Proto light block.
//!
//! The binary Cap'n Proto payload remains canonical. These helpers decode it
//! into stable human-readable JSON for debugging only.

use crate::index::{decode_light_block, LightBlockInput};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct JsonLightBlock {
    pub version: u16,
    pub height: u64,
    pub block_hash: String,
    pub previous_block_hash: String,
    pub first_uid: u64,
    pub skipped_txs_for_tweaks: Vec<u16>,
    pub tweaks: Vec<JsonTweakEntry>,
    pub output_fingerprint_bits: u8,
    pub output_fingerprint_bytes: usize,
    pub output_fingerprints: String,
    pub spends: Vec<JsonSpendEntry>,
}

#[derive(Debug, Serialize)]
pub struct JsonTweakEntry {
    pub output_count: u16,
    pub tweak: String,
}

#[derive(Debug, Serialize)]
pub struct JsonSpendEntry {
    pub spent_uid: u64,
}

pub fn light_block_to_json(bytes: &[u8]) -> anyhow::Result<JsonLightBlock> {
    light_block_input_to_json(decode_light_block(bytes)?)
}

fn light_block_input_to_json(input: LightBlockInput) -> anyhow::Result<JsonLightBlock> {
    let output_fingerprint_bytes = input.output_fingerprints.len();
    Ok(JsonLightBlock {
        version: crate::WIRE_VERSION,
        height: input.height,
        block_hash: hex::encode(input.block_hash.as_bytes()),
        previous_block_hash: hex::encode(input.previous_block_hash.as_bytes()),
        first_uid: input.first_uid,
        skipped_txs_for_tweaks: input.skipped_txs_for_tweaks,
        tweaks: input
            .tweaks
            .into_iter()
            .map(|entry| JsonTweakEntry {
                output_count: entry.output_count,
                tweak: hex::encode(&entry.tweak.as_bytes()[1..]),
            })
            .collect(),
        output_fingerprint_bits: input.output_fingerprint_bits,
        output_fingerprint_bytes,
        output_fingerprints: hex::encode(input.output_fingerprints),
        spends: input
            .spends
            .into_iter()
            .map(|entry| JsonSpendEntry {
                spent_uid: entry.spent_uid,
            })
            .collect(),
    })
}
