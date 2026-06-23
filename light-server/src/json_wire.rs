//! JSON representation for the typed Cap'n Proto light block.
//!
//! The binary Cap'n Proto payload remains canonical. These helpers decode it
//! into stable human-readable JSON for debugging only.

use crate::light_capnp::light_block;
use capnp::message::ReaderOptions;
use serde::Serialize;
use std::io::Cursor;

#[derive(Debug, Serialize)]
pub struct JsonLightBlock {
    pub version: u16,
    pub height: u64,
    pub block_hash: String,
    pub previous_block_hash: String,
    pub first_uid: u64,
    pub p2tr_output_count: u32,
    pub skipped_txs_for_tweaks: Vec<u16>,
    pub tweaks: Vec<JsonTweakEntry>,
    pub skipped_outputs: Vec<u16>,
    pub outputs: Vec<JsonOutputEntry>,
    pub spends: Vec<JsonSpendEntry>,
}

#[derive(Debug, Serialize)]
pub struct JsonTweakEntry {
    pub output_count: u16,
    pub tweak: String,
}

#[derive(Debug, Serialize)]
pub struct JsonOutputEntry {
    pub uid: u64,
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct JsonSpendEntry {
    pub spent_uid: u64,
}

pub fn light_block_to_json(bytes: &[u8]) -> anyhow::Result<JsonLightBlock> {
    let mut cursor = Cursor::new(bytes);
    let message = capnp::serialize_packed::read_message(&mut cursor, ReaderOptions::new())?;
    let block = message.get_root::<light_block::Reader>()?;

    let skipped_txs_for_tweaks = read_u16_list(block.get_skipped_txs_for_tweaks()?);
    let skipped_outputs = read_u16_list(block.get_skipped_outputs()?);

    let tweak_reader = block.get_tweaks()?;
    let mut tweaks = Vec::with_capacity(tweak_reader.len() as usize);
    for i in 0..tweak_reader.len() {
        let entry = tweak_reader.get(i);
        let tweak = entry.get_tweak()?;
        anyhow::ensure!(tweak.len() == 32, "tweak entry {i} is not 32 bytes");
        tweaks.push(JsonTweakEntry {
            output_count: entry.get_output_count(),
            tweak: hex::encode(tweak),
        });
    }

    let output_reader = block.get_outputs()?;
    let mut outputs = Vec::with_capacity(output_reader.len() as usize);
    let mut skipped_iter = skipped_outputs.iter().copied().peekable();
    let mut p2tr_slot = 0u64;
    for i in 0..output_reader.len() {
        while skipped_iter
            .peek()
            .is_some_and(|skipped| u64::from(*skipped) == p2tr_slot)
        {
            skipped_iter.next();
            p2tr_slot += 1;
        }

        let entry = output_reader.get(i);
        let key = entry.get_key()?;
        anyhow::ensure!(key.len() == 32, "output entry {i} key is not 32 bytes");
        outputs.push(JsonOutputEntry {
            uid: block.get_first_uid() + p2tr_slot,
            key: hex::encode(key),
        });
        p2tr_slot += 1;
    }

    let spend_reader = block.get_spends()?;
    let mut spends = Vec::with_capacity(spend_reader.len() as usize);
    for i in 0..spend_reader.len() {
        spends.push(JsonSpendEntry {
            spent_uid: spend_reader.get(i).get_spent_uid(),
        });
    }

    Ok(JsonLightBlock {
        version: block.get_version(),
        height: block.get_height(),
        block_hash: hex::encode(block.get_block_hash()?),
        previous_block_hash: hex::encode(block.get_previous_block_hash()?),
        first_uid: block.get_first_uid(),
        p2tr_output_count: u32::try_from(outputs.len() + skipped_outputs.len())?,
        skipped_txs_for_tweaks,
        tweaks,
        skipped_outputs,
        outputs,
        spends,
    })
}

fn read_u16_list(list: capnp::primitive_list::Reader<'_, u16>) -> Vec<u16> {
    let mut out = Vec::with_capacity(list.len() as usize);
    for i in 0..list.len() {
        out.push(list.get(i));
    }
    out
}
