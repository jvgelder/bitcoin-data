//! JSON representations for the Cap'n Proto light-sync wire messages.
//!
//! The binary Cap'n Proto payload remains canonical. These helpers decode the
//! cached Cap'n Proto bytes into stable, human-readable JSON for debugging,
//! interoperability, and simple clients.

use crate::index::{decode_spent_uids, decode_tx_tweak_indexes};
use crate::light_capnp::light_block;
use crate::storage::ServedProfile;
use capnp::message::ReaderOptions;
use serde::Serialize;
use std::io::Cursor;

#[derive(Debug, Serialize)]
pub struct JsonLightBlock {
    pub version: u16,
    pub height: u64,
    pub block_hash: String,
    pub previous_block_hash: String,
    pub block_anchor_last_uid: u64,
    pub profile: JsonProfile,
    pub output_id_bytes: u8,
    pub tweaks: Vec<JsonTxTweak>,
    pub outputs: Vec<JsonOutputRef>,
    pub spent_id_codec: &'static str,
    pub spent_uids: Vec<u64>,
}


#[derive(Debug, Serialize)]
pub struct JsonProfile {
    pub name: Option<String>,
    pub scope: String,
    pub cutthrough: bool,
    pub cutthrough_blocks: u32,
}

#[derive(Debug, Serialize)]
pub struct JsonTxTweak {
    pub tx_index: u32,
    pub tweak: String,
}

#[derive(Debug, Serialize)]
pub struct JsonOutputRef {
    pub tx_index: u32,
    pub vout: u32,
    pub uid: u64,
    pub output_id: String,
}

pub fn light_block_to_json(
    bytes: &[u8],
    profile: Option<&ServedProfile>,
) -> anyhow::Result<JsonLightBlock> {
    let mut cursor = Cursor::new(bytes);
    let message = capnp::serialize_packed::read_message(&mut cursor, ReaderOptions::new())?;
    let block = message.get_root::<light_block::Reader>()?;

    let output_id_bytes = block.get_output_id_bytes();
    let output_id_len = output_id_bytes as usize;
    let output_ids = block.get_output_ids()?;
    let outputs_reader = block.get_outputs()?;
    anyhow::ensure!(
        output_ids.len() == outputs_reader.len() as usize * output_id_len,
        "packed output ID length mismatch in cached block"
    );

    let mut outputs = Vec::with_capacity(outputs_reader.len() as usize);
    for i in 0..outputs_reader.len() {
        let output = outputs_reader.get(i);
        let offset = i as usize * output_id_len;
        outputs.push(JsonOutputRef {
            tx_index: output.get_tx_index(),
            vout: output.get_vout(),
            uid: output.get_uid(),
            output_id: hex::encode(&output_ids[offset..offset + output_id_len]),
        });
    }

    let tweak_indexes = decode_tx_tweak_indexes(
        block.get_tx_tweak_indexes()?,
        block.get_tweak_count() as usize,
    )?;
    let tweak_bytes = block.get_tx_tweaks()?;
    anyhow::ensure!(
        tweak_bytes.len() == tweak_indexes.len() * crate::types::TxTweak::LEN,
        "tx tweak byte length mismatch"
    );
    let tweaks = tweak_indexes
        .into_iter()
        .enumerate()
        .map(|(i, tx_index)| JsonTxTweak {
            tx_index,
            tweak: hex::encode(
                &tweak_bytes[i * crate::types::TxTweak::LEN..(i + 1) * crate::types::TxTweak::LEN],
            ),
        })
        .collect();

    let spent_uids = decode_spent_uids(
        block.get_block_anchor_last_uid(),
        block.get_spent_ids()?,
        block.get_spent_count() as usize,
    )?;

    Ok(JsonLightBlock {
        version: block.get_version(),
        height: block.get_height(),
        block_hash: hex::encode(block.get_block_hash()?),
        previous_block_hash: hex::encode(block.get_previous_block_hash()?),
        block_anchor_last_uid: block.get_block_anchor_last_uid(),
        profile: json_profile(profile),
        output_id_bytes,
        tweaks,
        outputs,
        spent_id_codec: "eliasDeltaSorted",
        spent_uids,
    })
}

fn json_profile(profile: Option<&ServedProfile>) -> JsonProfile {
    match profile {
        Some(profile) => JsonProfile {
            name: Some(profile.name.clone()),
            scope: profile.profile.scope.as_str().to_string(),
            cutthrough: profile.profile.cutthrough_blocks != 0,
            cutthrough_blocks: profile.profile.cutthrough_blocks,
        },
        None => JsonProfile {
            name: None,
            scope: "unknown".to_string(),
            cutthrough: false,
            cutthrough_blocks: 0,
        },
    }
}
