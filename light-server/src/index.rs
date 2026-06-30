use crate::codec::{decode_elias_delta_values, encode_elias_delta_values};
use crate::helper::{read_32, read_u16_list};
use crate::light_capnp::{light_block, stored_light_block};
use crate::tagged_hash::{TaggedSha256, TRUNCATED_OUTPUT_HASH_TAG_HASH};
use crate::types::{BlockHashBytes, TxTweak};
use crate::WIRE_VERSION;
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use std::collections::BTreeSet;
use std::io::Cursor;
use std::sync::OnceLock;

pub const MAX_U16_SECTION_COUNT: usize = u16::MAX as usize;
pub const STORAGE_OUTPUT_FLAG_REUSED: u8 = 1 << 0;
pub const STORAGE_SPENT_HEIGHT_UNSPENT: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct TweakEntryInput {
    /// Number of dense outputs associated with this tweak entry.
    pub output_count: u16,
    /// Compressed 33-byte scan/tweak point. The served response stores the
    /// 32-byte x-coordinate in the flat `txTweaks` blob.
    pub tweak: TxTweak,
}

pub type StoredTweakEntryInput = TweakEntryInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpentIdCodec {
    EliasDeltaAscendingAbsolute,
}

#[derive(Debug, Clone)]
pub struct LightBlockInput {
    pub height: u64,
    pub block_hash: BlockHashBytes,
    pub previous_block_hash: BlockHashBytes,
    /// First global UID assigned to this block's native-P2TR output domain.
    pub first_uid: u64,
    /// Tx indexes without a corresponding dense tweak entry.
    pub skipped_txs_for_tweaks: Vec<u16>,
    /// One typed entry per dense tweak.
    pub tweaks: Vec<TweakEntryInput>,
    /// Number of bits in every packed truncated output hash.
    pub truncated_output_hash_bits: u8,
    /// Packed truncated output hashes in dense output order.
    pub truncated_output_hashes: Vec<u8>,
    /// Codec used by `spent_ids`.
    pub spent_id_codec: SpentIdCodec,
    /// Number of decoded spent UIDs in `spent_ids`.
    pub spent_count: u32,
    /// Packed spent UID stream. For v1 this is sorted ascending, Elias-delta encoded
    /// as first+1 followed by positive deltas.
    pub spent_ids: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct StoredOutputEntryInput {
    /// 32-byte P2TR x-only output key.
    pub key: [u8; 32],
    /// `STORAGE_SPENT_HEIGHT_UNSPENT` means unspent as of the archive tip.
    pub spent_height: u32,
    /// Storage-only output flags. Bit 0 means reused P2TR output key.
    pub flags: u8,
}

#[derive(Debug, Clone)]
pub struct StoredSpendEntryInput {
    pub spent_uid: u64,
    /// Height where the spent output was created. Used by server cut-through.
    pub creation_height: u32,
}

#[derive(Debug, Clone)]
pub struct StoredLightBlockInput {
    pub height: u64,
    pub block_hash: BlockHashBytes,
    pub previous_block_hash: BlockHashBytes,
    pub first_uid: u64,
    pub skipped_txs_for_tweaks: Vec<u16>,
    pub tweaks: Vec<StoredTweakEntryInput>,
    /// Storage-only static omitted P2TR slots for stats/debugging.
    pub skipped_outputs: Vec<u16>,
    pub outputs: Vec<StoredOutputEntryInput>,
    pub spends: Vec<StoredSpendEntryInput>,
    /// Raw serialized Bitcoin block size, excluding undo/spenttxouts data.
    pub raw_block_bytes: u32,
    /// Packed truncated output hash for clients with up to two labels.
    pub truncated_output_hash_for_two_labels: Vec<u8>,
    /// Packed truncated output hash for clients with more than two labels.
    pub truncated_output_hash_for_hundred_labels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StoredBlockResponseFilter {
    /// When set, produce a cut-through response for this starting height.
    /// Outputs created at or after this height and spent by cutthrough_tip are
    /// omitted, and spends for outputs created at or after this height are
    /// omitted.
    pub cutthrough_start: Option<u64>,
    /// Archive tip used for cut-through decisions. If omitted, every non-unspent
    /// spentHeight is considered spent.
    pub cutthrough_tip: Option<u64>,
    /// Omit outputs marked with STORAGE_OUTPUT_FLAG_REUSED.
    pub filter_reuse: bool,
    /// Optional requested wallet label budget. `<= 2` serves the two-label
    /// truncated output hash stream; larger or omitted serves the hundred-label stream.
    pub labels: Option<u16>,
}

impl StoredLightBlockInput {
    pub fn to_response_input(&self) -> LightBlockInput {
        self.to_response_input_for_labels(None)
            .expect("stored block should have valid default response truncated output hash")
    }

    pub fn to_response_input_for_labels(
        &self,
        labels: Option<u16>,
    ) -> anyhow::Result<LightBlockInput> {
        let label_budget = response_label_budget(labels);
        let output_count = self.outputs.len();
        let truncated_output_hash_bits =
            choose_truncated_output_hash_bits(output_count, self.raw_block_bytes, label_budget);
        let truncated_output_hashes = match label_budget {
            RESPONSE_LABEL_BUDGET_TWO => self.truncated_output_hash_for_two_labels.clone(),
            RESPONSE_LABEL_BUDGET_HUNDRED => self.truncated_output_hash_for_hundred_labels.clone(),
            _ => unreachable!("label budget is normalized"),
        };

        let (spent_count, spent_ids) = encode_spent_ids_elias_delta_ascending_absolute(
            self.spends.iter().map(|entry| entry.spent_uid),
        )?;

        let response = LightBlockInput {
            height: self.height,
            block_hash: self.block_hash,
            previous_block_hash: self.previous_block_hash,
            first_uid: self.first_uid,
            skipped_txs_for_tweaks: self.skipped_txs_for_tweaks.clone(),
            tweaks: self.tweaks.clone(),
            truncated_output_hash_bits,
            truncated_output_hashes,
            spent_id_codec: SpentIdCodec::EliasDeltaAscendingAbsolute,
            spent_count,
            spent_ids,
        };

        validate_light_block_input(&response)?;
        Ok(response)
    }

    pub fn to_filtered_response_input(
        &self,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<LightBlockInput> {
        if !filter.filter_reuse && filter.cutthrough_start.is_none() {
            return self.to_response_input_for_labels(filter.labels);
        }

        let mut skipped_txs_for_tweaks = self
            .skipped_txs_for_tweaks
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        let mut output_keys = Vec::<[u8; 32]>::new();
        let mut tweaks = Vec::<TweakEntryInput>::new();
        let mut output_cursor = 0usize;
        let tweak_tx_indexes =
            derive_stored_tweak_tx_indexes(&self.skipped_txs_for_tweaks, self.tweaks.len())?;

        for (tweak, tx_index) in self.tweaks.iter().zip(tweak_tx_indexes) {
            let group_count = usize::from(tweak.output_count);
            let group_end = output_cursor.checked_add(group_count).ok_or_else(|| {
                anyhow::anyhow!(
                    "stored tweak output cursor overflow at block {}",
                    self.height
                )
            })?;
            anyhow::ensure!(
                group_end <= self.outputs.len(),
                "stored tweak output count exceeds output list at block {} tx_index {}",
                self.height,
                tx_index
            );

            let mut kept_count = 0u16;
            for output in &self.outputs[output_cursor..group_end] {
                if should_omit_stored_output(self.height, output, filter) {
                    continue;
                }
                output_keys.push(output.key);
                kept_count = kept_count
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("filtered tweak output count overflow"))?;
            }
            output_cursor = group_end;

            if kept_count > 0 {
                tweaks.push(TweakEntryInput {
                    output_count: kept_count,
                    tweak: tweak.tweak,
                });
            } else {
                skipped_txs_for_tweaks.insert(tx_index);
            }
        }

        anyhow::ensure!(
            output_cursor == self.outputs.len(),
            "stored outputs not fully consumed by tweak output counts at block {}",
            self.height
        );

        let (spent_count, spent_ids) = encode_spent_ids_elias_delta_ascending_absolute(
            self.spends
                .iter()
                .filter(|spend| !should_omit_stored_spend(self.height, spend, filter))
                .map(|spend| spend.spent_uid),
        )?;

        let label_budget = response_label_budget(filter.labels);
        let truncated_output_hash_bits = choose_truncated_output_hash_bits(
            output_keys.len(),
            self.raw_block_bytes,
            label_budget,
        );
        let truncated_output_hashes =
            pack_truncated_output_hash_from_keys(output_keys.iter(), truncated_output_hash_bits)?;

        let response = LightBlockInput {
            height: self.height,
            block_hash: self.block_hash,
            previous_block_hash: self.previous_block_hash,
            first_uid: self.first_uid,
            skipped_txs_for_tweaks: skipped_txs_for_tweaks.into_iter().collect(),
            tweaks,
            truncated_output_hash_bits,
            truncated_output_hashes,
            spent_id_codec: SpentIdCodec::EliasDeltaAscendingAbsolute,
            spent_count,
            spent_ids,
        };
        validate_light_block_input(&response)?;
        Ok(response)
    }
}

fn derive_stored_tweak_tx_indexes(
    skipped_txs_for_tweaks: &[u16],
    tweak_count: usize,
) -> anyhow::Result<Vec<u16>> {
    let skipped = skipped_txs_for_tweaks
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut indexes = Vec::with_capacity(tweak_count);
    let mut tx_index = 0u32;

    while indexes.len() < tweak_count {
        if tx_index > u32::from(u16::MAX) {
            anyhow::bail!("stored tweak tx index overflow while deriving ordered tweak indexes");
        }

        let tx_index_u16 = tx_index as u16;
        if !skipped.contains(&tx_index_u16) {
            indexes.push(tx_index_u16);
        }
        tx_index = tx_index
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("stored tweak tx index overflow"))?;
    }

    Ok(indexes)
}

fn should_omit_stored_output(
    block_height: u64,
    output: &StoredOutputEntryInput,
    filter: StoredBlockResponseFilter,
) -> bool {
    if filter.filter_reuse && (output.flags & STORAGE_OUTPUT_FLAG_REUSED) != 0 {
        return true;
    }

    if let Some(cutthrough_start) = filter.cutthrough_start {
        let cutthrough_tip = filter.cutthrough_tip.unwrap_or(u64::MAX);
        let spent_height = output.spent_height;
        if block_height >= cutthrough_start
            && spent_height != STORAGE_SPENT_HEIGHT_UNSPENT
            && u64::from(spent_height) <= cutthrough_tip
        {
            return true;
        }
    }

    false
}

fn should_omit_stored_spend(
    block_height: u64,
    spend: &StoredSpendEntryInput,
    filter: StoredBlockResponseFilter,
) -> bool {
    let Some(cutthrough_start) = filter.cutthrough_start else {
        return false;
    };
    let cutthrough_tip = filter.cutthrough_tip.unwrap_or(u64::MAX);

    block_height <= cutthrough_tip && u64::from(spend.creation_height) >= cutthrough_start
}

pub fn encode_light_block(input: &LightBlockInput) -> anyhow::Result<Builder<HeapAllocator>> {
    validate_light_block_input(input)?;

    let mut msg = Builder::new_default();
    {
        let mut b = msg.init_root::<light_block::Builder>();

        b.set_version(WIRE_VERSION);
        b.set_height(input.height);
        b.set_block_hash(input.block_hash.as_bytes());
        b.set_previous_block_hash(input.previous_block_hash.as_bytes());
        b.set_first_uid(input.first_uid);

        let mut skipped_txs = b
            .reborrow()
            .init_skipped_txs_for_tweaks(input.skipped_txs_for_tweaks.len() as u32);
        for (i, tx_index) in input.skipped_txs_for_tweaks.iter().copied().enumerate() {
            skipped_txs.set(i as u32, tx_index);
        }

        let (tweak_output_counts, tx_tweaks) = encode_flat_tweaks(&input.tweaks);
        b.set_tweak_count(input.tweaks.len() as u32);
        b.set_tweak_output_counts(&tweak_output_counts);
        b.set_tx_tweaks(&tx_tweaks);

        b.set_truncated_output_hash_bits(input.truncated_output_hash_bits);
        b.set_truncated_output_hashes(&input.truncated_output_hashes);
        b.set_spent_id_codec(spent_id_codec_to_capnp(input.spent_id_codec));
        b.set_spent_count(input.spent_count);
        b.set_spent_ids(&input.spent_ids);
    }
    Ok(msg)
}

pub fn encode_stored_light_block(
    input: &StoredLightBlockInput,
) -> anyhow::Result<Builder<HeapAllocator>> {
    validate_stored_light_block_input(input)?;

    let mut msg = Builder::new_default();
    {
        let mut b = msg.init_root::<stored_light_block::Builder>();

        b.set_version(WIRE_VERSION);
        b.set_height(u32::try_from(input.height)?);
        b.set_block_hash(input.block_hash.as_bytes());
        b.set_previous_block_hash(input.previous_block_hash.as_bytes());
        b.set_first_uid(input.first_uid);

        let mut skipped_txs = b
            .reborrow()
            .init_skipped_txs_for_tweaks(input.skipped_txs_for_tweaks.len() as u32);
        for (i, tx_index) in input.skipped_txs_for_tweaks.iter().copied().enumerate() {
            skipped_txs.set(i as u32, tx_index);
        }

        let mut tweaks = b.reborrow().init_tweaks(input.tweaks.len() as u32);
        for (i, entry) in input.tweaks.iter().enumerate() {
            let mut t = tweaks.reborrow().get(i as u32);
            t.set_output_count(entry.output_count);
            t.set_tweak(tx_tweak_payload_bytes(&entry.tweak));
        }

        let mut skipped_outputs = b
            .reborrow()
            .init_skipped_outputs(input.skipped_outputs.len() as u32);
        for (i, output_index) in input.skipped_outputs.iter().copied().enumerate() {
            skipped_outputs.set(i as u32, output_index);
        }

        let mut outputs = b.reborrow().init_outputs(input.outputs.len() as u32);
        for (i, entry) in input.outputs.iter().enumerate() {
            let mut o = outputs.reborrow().get(i as u32);
            o.set_key(&entry.key);
            o.set_spent_height(entry.spent_height);
            o.set_flags(entry.flags);
        }

        let mut spends = b.reborrow().init_spends(input.spends.len() as u32);
        for (i, entry) in input.spends.iter().enumerate() {
            let mut s = spends.reborrow().get(i as u32);
            s.set_spent_uid(entry.spent_uid);
            s.set_creation_height(entry.creation_height);
        }

        b.set_raw_block_bytes(input.raw_block_bytes);
        b.set_truncated_output_hash_for_two_labels(&input.truncated_output_hash_for_two_labels);
        b.set_truncated_output_hash_for_hundred_labels(
            &input.truncated_output_hash_for_hundred_labels,
        );
    }
    Ok(msg)
}

pub fn decode_light_block(bytes: &[u8]) -> anyhow::Result<LightBlockInput> {
    let mut cursor = Cursor::new(bytes);
    let message = capnp::serialize_packed::read_message(&mut cursor, ReaderOptions::new())?;
    let block = message.get_root::<light_block::Reader>()?;

    anyhow::ensure!(
        block.get_version() == WIRE_VERSION,
        "unsupported light block version {}",
        block.get_version()
    );

    let tweaks = decode_flat_tweaks(
        block.get_tweak_count(),
        block.get_tweak_output_counts()?,
        block.get_tx_tweaks()?,
    )?;

    let spent_id_codec = spent_id_codec_from_capnp(
        block
            .get_spent_id_codec()
            .map_err(|err| anyhow::anyhow!("unsupported spent id codec: {:?}", err))?,
    )?;

    let input = LightBlockInput {
        height: block.get_height(),
        block_hash: BlockHashBytes::from(read_32(block.get_block_hash()?, "block hash")?),
        previous_block_hash: BlockHashBytes::from(read_32(
            block.get_previous_block_hash()?,
            "previous block hash",
        )?),
        first_uid: block.get_first_uid(),
        skipped_txs_for_tweaks: read_u16_list(block.get_skipped_txs_for_tweaks()?),
        tweaks,
        truncated_output_hash_bits: block.get_truncated_output_hash_bits(),
        truncated_output_hashes: block.get_truncated_output_hashes()?.to_vec(),
        spent_id_codec,
        spent_count: block.get_spent_count(),
        spent_ids: block.get_spent_ids()?.to_vec(),
    };
    validate_light_block_input(&input)?;
    Ok(input)
}

pub fn decode_stored_light_block(bytes: &[u8]) -> anyhow::Result<StoredLightBlockInput> {
    let mut cursor = Cursor::new(bytes);
    let message = capnp::serialize_packed::read_message(&mut cursor, ReaderOptions::new())?;
    let block = message.get_root::<stored_light_block::Reader>()?;

    anyhow::ensure!(
        block.get_version() == WIRE_VERSION,
        "unsupported stored light block version {}",
        block.get_version()
    );

    let skipped_txs_for_tweaks = read_u16_list(block.get_skipped_txs_for_tweaks()?);
    let skipped_outputs = read_u16_list(block.get_skipped_outputs()?);
    let tweak_reader = block.get_tweaks()?;
    let mut tweaks = Vec::with_capacity(tweak_reader.len() as usize);
    for i in 0..tweak_reader.len() {
        let entry = tweak_reader.get(i);
        let tweak = entry.get_tweak()?;
        anyhow::ensure!(tweak.len() == 32, "stored tweak entry {i} is not 32 bytes");
        tweaks.push(TweakEntryInput {
            output_count: entry.get_output_count(),
            tweak: tx_tweak_from_payload_bytes(tweak)?,
        });
    }

    let output_reader = block.get_outputs()?;
    let mut outputs = Vec::with_capacity(output_reader.len() as usize);
    for i in 0..output_reader.len() {
        let entry = output_reader.get(i);
        let key = entry.get_key()?;
        anyhow::ensure!(
            key.len() == 32,
            "stored output entry {i} key is not 32 bytes"
        );
        outputs.push(StoredOutputEntryInput {
            key: read_32(key, "stored output key")?,
            spent_height: entry.get_spent_height(),
            flags: entry.get_flags(),
        });
    }

    let spend_reader = block.get_spends()?;
    let mut spends = Vec::with_capacity(spend_reader.len() as usize);
    for i in 0..spend_reader.len() {
        let entry = spend_reader.get(i);
        spends.push(StoredSpendEntryInput {
            spent_uid: entry.get_spent_uid(),
            creation_height: entry.get_creation_height(),
        });
    }

    let input = StoredLightBlockInput {
        height: u64::from(block.get_height()),
        block_hash: BlockHashBytes::from(read_32(block.get_block_hash()?, "stored block hash")?),
        previous_block_hash: BlockHashBytes::from(read_32(
            block.get_previous_block_hash()?,
            "stored previous block hash",
        )?),
        first_uid: block.get_first_uid(),
        skipped_txs_for_tweaks,
        tweaks,
        skipped_outputs,
        outputs,
        spends,
        raw_block_bytes: block.get_raw_block_bytes(),
        truncated_output_hash_for_two_labels: block
            .get_truncated_output_hash_for_two_labels()?
            .to_vec(),
        truncated_output_hash_for_hundred_labels: block
            .get_truncated_output_hash_for_hundred_labels()?
            .to_vec(),
    };
    validate_stored_light_block_input(&input)?;
    Ok(input)
}

pub fn stored_light_block_to_response_bytes(
    bytes: &[u8],
) -> anyhow::Result<(Vec<u8>, BlockHashBytes)> {
    stored_light_block_to_filtered_response_bytes(bytes, StoredBlockResponseFilter::default())
}

pub fn stored_light_block_to_filtered_response_bytes(
    bytes: &[u8],
    filter: StoredBlockResponseFilter,
) -> anyhow::Result<(Vec<u8>, BlockHashBytes)> {
    let stored = decode_stored_light_block(bytes)?;
    let block_hash = stored.block_hash;
    let response = stored.to_filtered_response_input(filter)?;
    let message = encode_light_block(&response)?;
    Ok((to_packed_bytes(&message)?, block_hash))
}

fn spent_id_codec_to_capnp(codec: SpentIdCodec) -> crate::light_capnp::SpentIdCodec {
    match codec {
        SpentIdCodec::EliasDeltaAscendingAbsolute => {
            crate::light_capnp::SpentIdCodec::EliasDeltaAscendingAbsolute
        }
    }
}

fn spent_id_codec_from_capnp(
    codec: crate::light_capnp::SpentIdCodec,
) -> anyhow::Result<SpentIdCodec> {
    match codec {
        crate::light_capnp::SpentIdCodec::EliasDeltaAscendingAbsolute => {
            Ok(SpentIdCodec::EliasDeltaAscendingAbsolute)
        }
    }
}

pub fn encode_spent_ids_elias_delta_ascending_absolute(
    spent_ids: impl IntoIterator<Item = u64>,
) -> anyhow::Result<(u32, Vec<u8>)> {
    let mut ids = spent_ids.into_iter().collect::<Vec<_>>();
    ids.sort_unstable();

    if ids.is_empty() {
        return Ok((0, Vec::new()));
    }

    let mut values = Vec::with_capacity(ids.len());
    let mut prev: Option<u64> = None;

    for uid in ids {
        let value = match prev {
            None => uid
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("spent uid overflow while encoding first value"))?,
            Some(prev_uid) => uid
                .checked_sub(prev_uid)
                .ok_or_else(|| anyhow::anyhow!("spent ids are not sorted ascending"))?,
        };
        anyhow::ensure!(
            value > 0,
            "duplicate spent uid {uid} cannot be Elias-delta encoded as a positive delta"
        );
        values.push(value);
        prev = Some(uid);
    }

    let count = u32::try_from(values.len())?;
    Ok((count, encode_elias_delta_values(&values)?))
}

pub fn decode_spent_ids_elias_delta_ascending_absolute(
    spent_count: u32,
    spent_ids: &[u8],
) -> anyhow::Result<Vec<u64>> {
    if spent_count == 0 {
        anyhow::ensure!(
            spent_ids.is_empty(),
            "empty spent stream must have no bytes"
        );
        return Ok(Vec::new());
    }

    let values = decode_elias_delta_values(spent_ids, usize::try_from(spent_count)?)?;
    let first = values[0]
        .checked_sub(1)
        .ok_or_else(|| anyhow::anyhow!("invalid first spent id value"))?;

    let mut out = Vec::with_capacity(values.len());
    out.push(first);
    let mut uid = first;

    for delta in &values[1..] {
        uid = uid
            .checked_add(*delta)
            .ok_or_else(|| anyhow::anyhow!("spent uid delta overflow"))?;
        out.push(uid);
    }

    Ok(out)
}

pub fn decode_light_block_spent_ids(input: &LightBlockInput) -> anyhow::Result<Vec<u64>> {
    match input.spent_id_codec {
        SpentIdCodec::EliasDeltaAscendingAbsolute => {
            decode_spent_ids_elias_delta_ascending_absolute(input.spent_count, &input.spent_ids)
        }
    }
}

pub fn validate_light_block_input(input: &LightBlockInput) -> anyhow::Result<()> {
    ensure_u16_len("skipped txs for tweaks", input.skipped_txs_for_tweaks.len())?;
    ensure_u16_len("tweaks", input.tweaks.len())?;
    validate_sorted_unique_u16(&input.skipped_txs_for_tweaks, "skipped txs for tweaks")?;

    let declared_outputs = light_block_output_count(input)?;
    let expected_truncated_output_hash_len =
        packed_truncated_output_hash_len(declared_outputs, input.truncated_output_hash_bits)?;
    anyhow::ensure!(
        expected_truncated_output_hash_len == input.truncated_output_hashes.len(),
        "truncated output hash byte length {} does not match expected {} for {} outputs at {} bits",
        input.truncated_output_hashes.len(),
        expected_truncated_output_hash_len,
        declared_outputs,
        input.truncated_output_hash_bits
    );

    if declared_outputs == 0 {
        anyhow::ensure!(
            input.truncated_output_hash_bits == 0,
            "empty truncated output hash stream must use 0 bits"
        );
    } else {
        anyhow::ensure!(
            input.truncated_output_hash_bits > 0,
            "non-empty truncated output hash stream must use a positive bit width"
        );
    }

    for entry in &input.tweaks {
        anyhow::ensure!(
            tx_tweak_payload_bytes(&entry.tweak).len() == 32,
            "tweak payload must be 32 bytes"
        );
    }

    let decoded_spends = decode_light_block_spent_ids(input)?;
    anyhow::ensure!(
        decoded_spends.len() == usize::try_from(input.spent_count)?,
        "spent count {} does not match decoded spent ID count {}",
        input.spent_count,
        decoded_spends.len()
    );

    Ok(())
}

pub fn validate_stored_light_block_input(input: &StoredLightBlockInput) -> anyhow::Result<()> {
    anyhow::ensure!(
        input.height <= u64::from(u32::MAX),
        "stored block height {} exceeds u32 range",
        input.height
    );
    ensure_u16_len(
        "stored skipped txs for tweaks",
        input.skipped_txs_for_tweaks.len(),
    )?;
    ensure_u16_len("stored tweaks", input.tweaks.len())?;
    ensure_u16_len("stored skipped outputs", input.skipped_outputs.len())?;
    ensure_u16_len("stored outputs", input.outputs.len())?;

    validate_sorted_unique_u16(
        &input.skipped_txs_for_tweaks,
        "stored skipped txs for tweaks",
    )?;
    validate_sorted_unique_u16(&input.skipped_outputs, "stored skipped outputs")?;
    validate_stored_p2tr_uid_domain(input.outputs.len(), &input.skipped_outputs)?;

    validate_stored_truncated_output_hashes(input)?;

    let response = input.to_response_input_for_labels(None)?;
    validate_light_block_input(&response)?;

    for spend in &input.spends {
        anyhow::ensure!(
            spend.creation_height <= u32::try_from(input.height)?,
            "stored spend creation height exceeds spend block height"
        );
    }
    Ok(())
}

fn ensure_u16_len(label: &str, len: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        len <= MAX_U16_SECTION_COUNT,
        "{label} count {len} exceeds u16 section bound"
    );
    Ok(())
}

fn validate_sorted_unique_u16(values: &[u16], label: &str) -> anyhow::Result<()> {
    for pair in values.windows(2) {
        anyhow::ensure!(pair[0] < pair[1], "{label} must be sorted and unique");
    }
    Ok(())
}

fn validate_stored_p2tr_uid_domain(
    output_count: usize,
    skipped_outputs: &[u16],
) -> anyhow::Result<()> {
    let p2tr_output_count = output_count
        .checked_add(skipped_outputs.len())
        .ok_or_else(|| anyhow::anyhow!("stored P2TR output domain count overflow"))?;

    anyhow::ensure!(
        p2tr_output_count <= usize::from(u16::MAX) + 1,
        "stored P2TR output count {p2tr_output_count} exceeds u16 slot domain"
    );

    for slot in skipped_outputs {
        anyhow::ensure!(
            usize::from(*slot) < p2tr_output_count,
            "stored skipped output slot {} is outside derived P2TR output count {}",
            slot,
            p2tr_output_count
        );
    }

    Ok(())
}

pub const RESPONSE_LABEL_BUDGET_TWO: u16 = 2;
pub const RESPONSE_LABEL_BUDGET_HUNDRED: u16 = 100;

pub fn response_label_budget(labels: Option<u16>) -> u16 {
    match labels {
        Some(labels) if labels <= RESPONSE_LABEL_BUDGET_TWO => RESPONSE_LABEL_BUDGET_TWO,
        _ => RESPONSE_LABEL_BUDGET_HUNDRED,
    }
}

/// Simple log-based policy:
///
/// bits = ceil(log2(4 * label_budget * raw_block_bytes))
///
/// The factor 4 is the one-more-bit marginal break-even approximation for
/// packed bits vs expected false full-block download bytes.
pub fn choose_truncated_output_hash_bits(
    output_count: usize,
    raw_block_bytes: u32,
    label_budget: u16,
) -> u8 {
    if output_count == 0 || raw_block_bytes == 0 {
        return 0;
    }

    let value = 4.0 * f64::from(raw_block_bytes) * f64::from(label_budget.max(1));
    value.log2().ceil().clamp(1.0, 255.0) as u8
}

pub fn light_block_output_count(input: &LightBlockInput) -> anyhow::Result<usize> {
    input.tweaks.iter().try_fold(0usize, |acc, entry| {
        acc.checked_add(entry.output_count as usize)
            .ok_or_else(|| anyhow::anyhow!("tweak output count sum overflow"))
    })
}

pub fn packed_truncated_output_hash_len(output_count: usize, bits: u8) -> anyhow::Result<usize> {
    if output_count == 0 || bits == 0 {
        return Ok(0);
    }
    let total_bits = output_count
        .checked_mul(usize::from(bits))
        .ok_or_else(|| anyhow::anyhow!("truncated output hash bit length overflow"))?;
    Ok(total_bits.div_ceil(8))
}

pub fn pack_truncated_output_hash_from_keys<'a>(
    keys: impl ExactSizeIterator<Item = &'a [u8; 32]>,
    bits: u8,
) -> anyhow::Result<Vec<u8>> {
    if bits == 0 {
        return Ok(Vec::new());
    }

    let mut out = vec![0u8; packed_truncated_output_hash_len(keys.len(), bits)?];
    let mut out_bit = 0usize;
    let tagged_hasher = truncated_output_hash_hasher();

    for key in keys {
        let digest = tagged_hasher.digest_32(key);

        for bit in 0..usize::from(bits) {
            let src = (digest[bit / 8] >> (7 - (bit % 8))) & 1;
            if src != 0 {
                let byte = out_bit / 8;
                let shift = 7 - (out_bit % 8);
                out[byte] |= 1 << shift;
            }
            out_bit += 1;
        }
    }

    Ok(out)
}

fn truncated_output_hash_hasher() -> &'static TaggedSha256 {
    static HASHER: OnceLock<TaggedSha256> = OnceLock::new();
    HASHER.get_or_init(|| TaggedSha256::from_tag_hash(TRUNCATED_OUTPUT_HASH_TAG_HASH))
}

pub fn build_stored_truncated_output_hashes(
    raw_block_bytes: u32,
    outputs: &[StoredOutputEntryInput],
    label_budget: u16,
) -> anyhow::Result<Vec<u8>> {
    let bits = choose_truncated_output_hash_bits(outputs.len(), raw_block_bytes, label_budget);
    pack_truncated_output_hash_from_keys(outputs.iter().map(|entry| &entry.key), bits)
}

fn validate_stored_truncated_output_hashes(input: &StoredLightBlockInput) -> anyhow::Result<()> {
    for (label, label_budget, truncated_output_hashes) in [
        (
            "two-label",
            RESPONSE_LABEL_BUDGET_TWO,
            &input.truncated_output_hash_for_two_labels,
        ),
        (
            "hundred-label",
            RESPONSE_LABEL_BUDGET_HUNDRED,
            &input.truncated_output_hash_for_hundred_labels,
        ),
    ] {
        let bits = choose_truncated_output_hash_bits(
            input.outputs.len(),
            input.raw_block_bytes,
            label_budget,
        );
        let expected = packed_truncated_output_hash_len(input.outputs.len(), bits)?;
        anyhow::ensure!(
            truncated_output_hashes.len() == expected,
            "stored {label} truncated output hash byte length {} does not match expected {}",
            truncated_output_hashes.len(),
            expected
        );
    }

    Ok(())
}

fn encode_flat_tweaks(tweaks: &[TweakEntryInput]) -> (Vec<u8>, Vec<u8>) {
    let mut output_counts = Vec::with_capacity(tweaks.len() * 2);
    let mut tx_tweaks = Vec::with_capacity(tweaks.len() * 32);

    for entry in tweaks {
        output_counts.extend_from_slice(&entry.output_count.to_le_bytes());
        tx_tweaks.extend_from_slice(tx_tweak_payload_bytes(&entry.tweak));
    }

    (output_counts, tx_tweaks)
}

fn decode_flat_tweaks(
    tweak_count: u32,
    output_counts: &[u8],
    tx_tweaks: &[u8],
) -> anyhow::Result<Vec<TweakEntryInput>> {
    let tweak_count = usize::try_from(tweak_count)?;
    let expected_output_count_bytes = tweak_count
        .checked_mul(2)
        .ok_or_else(|| anyhow::anyhow!("flat tweak output-count byte length overflow"))?;
    let expected_tweak_bytes = tweak_count
        .checked_mul(32)
        .ok_or_else(|| anyhow::anyhow!("flat tweak payload byte length overflow"))?;

    anyhow::ensure!(
        output_counts.len() == expected_output_count_bytes,
        "flat tweak output-count bytes length {} does not match expected {} for {} tweaks",
        output_counts.len(),
        expected_output_count_bytes,
        tweak_count
    );
    anyhow::ensure!(
        tx_tweaks.len() == expected_tweak_bytes,
        "flat tweak payload bytes length {} does not match expected {} for {} tweaks",
        tx_tweaks.len(),
        expected_tweak_bytes,
        tweak_count
    );

    let mut tweaks = Vec::with_capacity(tweak_count);
    for i in 0..tweak_count {
        let count_offset = i * 2;
        let output_count =
            u16::from_le_bytes([output_counts[count_offset], output_counts[count_offset + 1]]);
        let tweak_offset = i * 32;
        let tweak = tx_tweak_from_payload_bytes(&tx_tweaks[tweak_offset..tweak_offset + 32])?;
        tweaks.push(TweakEntryInput {
            output_count,
            tweak,
        });
    }

    Ok(tweaks)
}

/// The indexer stores a compressed 33-byte tweak point internally. The served
/// light block carries only the 32-byte x-coordinate.
fn tx_tweak_payload_bytes(tweak: &TxTweak) -> &[u8] {
    &tweak.as_bytes()[1..]
}

fn tx_tweak_from_payload_bytes(bytes: &[u8]) -> anyhow::Result<TxTweak> {
    anyhow::ensure!(bytes.len() == 32, "tweak payload must be 32 bytes");
    let mut tweak = [0u8; 33];
    // The storage/response format stores the x-coordinate only. Use an even-y
    // compressed prefix when a TxTweak value is needed for re-encoding.
    tweak[0] = 0x02;
    tweak[1..].copy_from_slice(bytes);
    Ok(TxTweak::from(tweak))
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
    use crate::tagged_hash::TRUNCATED_OUTPUT_HASH_TAG_HASH;
    use sha2::{Digest, Sha256};

    fn stored_block_for_test() -> StoredLightBlockInput {
        let mut stored = StoredLightBlockInput {
            height: 100,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 42,
            skipped_txs_for_tweaks: vec![0, 3],
            tweaks: vec![TweakEntryInput {
                output_count: 1,
                tweak: TxTweak::from([9u8; 33]),
            }],
            skipped_outputs: vec![1],
            outputs: vec![StoredOutputEntryInput {
                key: [3u8; 32],
                spent_height: STORAGE_SPENT_HEIGHT_UNSPENT,
                flags: STORAGE_OUTPUT_FLAG_REUSED,
            }],
            spends: vec![StoredSpendEntryInput {
                spent_uid: 41,
                creation_height: 99,
            }],
            raw_block_bytes: 2_500_000,
            truncated_output_hash_for_two_labels: Vec::new(),
            truncated_output_hash_for_hundred_labels: Vec::new(),
        };
        stored.truncated_output_hash_for_two_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_TWO,
        )
        .unwrap();
        stored.truncated_output_hash_for_hundred_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_HUNDRED,
        )
        .unwrap();
        stored
    }

    #[test]
    fn truncated_output_hash_uses_bip340_tagged_hash() {
        let key = [3u8; 32];
        const TRUNCATED_OUTPUT_HASH_DOMAIN: &str = "bitcoindata/light-truncated-output-hash/v1";

        let tag_hash: [u8; 32] = Sha256::digest(TRUNCATED_OUTPUT_HASH_DOMAIN.as_bytes()).into();
        assert_eq!(TRUNCATED_OUTPUT_HASH_TAG_HASH, tag_hash);

        let mut reference = Sha256::new();
        reference.update(tag_hash);
        reference.update(tag_hash);
        reference.update(key);
        let expected: [u8; 32] = reference.finalize().into();

        assert_eq!(truncated_output_hash_hasher().digest_32(key), expected);
    }

    #[test]
    fn spent_ids_elias_delta_ascending_absolute_roundtrip() {
        let ids = [0u64, 41, 41_000, 1_000_000];
        let (count, bytes) = encode_spent_ids_elias_delta_ascending_absolute(ids).unwrap();
        assert_eq!(count, ids.len() as u32);
        let decoded = decode_spent_ids_elias_delta_ascending_absolute(count, &bytes).unwrap();
        assert_eq!(decoded, ids);
    }

    #[test]
    fn encodes_typed_light_block_with_flat_tweaks() {
        let input = LightBlockInput {
            height: 100,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 42,
            skipped_txs_for_tweaks: vec![0, 3],
            tweaks: vec![TweakEntryInput {
                output_count: 1,
                tweak: TxTweak::from([9u8; 33]),
            }],
            truncated_output_hash_bits: 8,
            truncated_output_hashes: vec![3],
            spent_id_codec: SpentIdCodec::EliasDeltaAscendingAbsolute,
            spent_count: 1,
            spent_ids: encode_spent_ids_elias_delta_ascending_absolute([41u64])
                .unwrap()
                .1,
        };
        let msg = encode_light_block(&input).unwrap();
        let bytes = to_packed_bytes(&msg).unwrap();
        assert!(!bytes.is_empty());

        let mut cursor = Cursor::new(&bytes);
        let message =
            capnp::serialize_packed::read_message(&mut cursor, ReaderOptions::new()).unwrap();
        let block = message.get_root::<light_block::Reader>().unwrap();
        assert_eq!(block.get_tweak_count(), 1);
        assert_eq!(block.get_tweak_output_counts().unwrap(), &[1, 0]);
        assert_eq!(block.get_tx_tweaks().unwrap(), &[9u8; 32]);

        let decoded = decode_light_block(&bytes).unwrap();
        assert_eq!(decoded.tweaks.len(), 1);
        assert_eq!(decoded.tweaks[0].output_count, 1);
        let mut expected_tweak = [9u8; 33];
        expected_tweak[0] = 0x02;
        assert_eq!(decoded.tweaks[0].tweak.as_bytes(), &expected_tweak);
    }

    #[test]
    fn rejects_malformed_flat_tweak_lengths() {
        assert!(decode_flat_tweaks(1, &[1], &[9u8; 32]).is_err());
        assert!(decode_flat_tweaks(1, &[1, 0], &[9u8; 31]).is_err());
    }

    #[test]
    fn encodes_and_decodes_stored_light_block_to_response() {
        let stored = stored_block_for_test();
        let bytes = to_packed_bytes(&encode_stored_light_block(&stored).unwrap()).unwrap();
        let decoded = decode_stored_light_block(&bytes).unwrap();
        assert_eq!(
            decoded.outputs[0].spent_height,
            STORAGE_SPENT_HEIGHT_UNSPENT
        );
        assert_eq!(
            decoded.outputs[0].flags & STORAGE_OUTPUT_FLAG_REUSED,
            STORAGE_OUTPUT_FLAG_REUSED
        );
        assert_eq!(decoded.spends[0].creation_height, 99);
        assert_eq!(decoded.skipped_outputs, vec![1]);
        assert_eq!(decoded.raw_block_bytes, 2_500_000);

        let (response_bytes, block_hash) = stored_light_block_to_response_bytes(&bytes).unwrap();
        assert_eq!(block_hash.as_bytes(), &[1u8; 32]);
        assert!(!response_bytes.is_empty());
    }

    #[test]
    fn validates_tweak_output_counts_match_truncated_output_hash_stream() {
        let input = LightBlockInput {
            height: 100,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 42,
            skipped_txs_for_tweaks: vec![],
            tweaks: vec![TweakEntryInput {
                output_count: 2,
                tweak: TxTweak::from([9u8; 33]),
            }],
            truncated_output_hash_bits: 8,
            truncated_output_hashes: vec![3],
            spent_id_codec: SpentIdCodec::EliasDeltaAscendingAbsolute,
            spent_count: 0,
            spent_ids: Vec::new(),
        };
        assert!(validate_light_block_input(&input).is_err());
    }

    #[test]
    fn filters_reused_outputs_and_recomputes_dense_tweak_counts() {
        let mut stored = StoredLightBlockInput {
            height: 100,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 10,
            skipped_txs_for_tweaks: vec![0],
            tweaks: vec![TweakEntryInput {
                output_count: 2,
                tweak: TxTweak::from([9u8; 33]),
            }],
            skipped_outputs: vec![],
            outputs: vec![
                StoredOutputEntryInput {
                    key: [3u8; 32],
                    spent_height: STORAGE_SPENT_HEIGHT_UNSPENT,
                    flags: 0,
                },
                StoredOutputEntryInput {
                    key: [4u8; 32],
                    spent_height: STORAGE_SPENT_HEIGHT_UNSPENT,
                    flags: STORAGE_OUTPUT_FLAG_REUSED,
                },
            ],
            spends: vec![],
            raw_block_bytes: 2_500_000,
            truncated_output_hash_for_two_labels: Vec::new(),
            truncated_output_hash_for_hundred_labels: Vec::new(),
        };
        stored.truncated_output_hash_for_two_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_TWO,
        )
        .unwrap();
        stored.truncated_output_hash_for_hundred_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_HUNDRED,
        )
        .unwrap();

        let response = stored
            .to_filtered_response_input(StoredBlockResponseFilter {
                filter_reuse: true,
                ..StoredBlockResponseFilter::default()
            })
            .unwrap();
        assert_eq!(light_block_output_count(&response).unwrap(), 1);
        assert_eq!(response.tweaks[0].output_count, 1);
    }

    #[test]
    fn cutthrough_filters_outputs_tweaks_skipped_txs_and_spent_ids() {
        let mut stored = StoredLightBlockInput {
            height: 200,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 1000,
            skipped_txs_for_tweaks: vec![0],
            tweaks: vec![
                TweakEntryInput {
                    output_count: 2,
                    tweak: TxTweak::from([9u8; 33]),
                },
                TweakEntryInput {
                    output_count: 1,
                    tweak: TxTweak::from([8u8; 33]),
                },
            ],
            skipped_outputs: vec![],
            outputs: vec![
                StoredOutputEntryInput {
                    key: [3u8; 32],
                    spent_height: STORAGE_SPENT_HEIGHT_UNSPENT,
                    flags: 0,
                },
                StoredOutputEntryInput {
                    key: [4u8; 32],
                    spent_height: 250,
                    flags: 0,
                },
                StoredOutputEntryInput {
                    key: [5u8; 32],
                    spent_height: 210,
                    flags: 0,
                },
            ],
            spends: vec![
                StoredSpendEntryInput {
                    spent_uid: 100,
                    creation_height: 149,
                },
                StoredSpendEntryInput {
                    spent_uid: 101,
                    creation_height: 150,
                },
            ],
            raw_block_bytes: 2_500_000,
            truncated_output_hash_for_two_labels: Vec::new(),
            truncated_output_hash_for_hundred_labels: Vec::new(),
        };
        stored.truncated_output_hash_for_two_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_TWO,
        )
        .unwrap();
        stored.truncated_output_hash_for_hundred_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_HUNDRED,
        )
        .unwrap();

        let response = stored
            .to_filtered_response_input(StoredBlockResponseFilter {
                cutthrough_start: Some(150),
                cutthrough_tip: Some(300),
                labels: Some(RESPONSE_LABEL_BUDGET_TWO),
                ..StoredBlockResponseFilter::default()
            })
            .unwrap();

        assert_eq!(light_block_output_count(&response).unwrap(), 1);
        assert_eq!(response.tweaks.len(), 1);
        assert_eq!(response.tweaks[0].output_count, 1);
        assert_eq!(response.tweaks[0].tweak.as_bytes(), &[9u8; 33]);
        assert_eq!(response.skipped_txs_for_tweaks, vec![0, 2]);
        assert_eq!(decode_light_block_spent_ids(&response).unwrap(), vec![100]);

        let expected_bits =
            choose_truncated_output_hash_bits(1, stored.raw_block_bytes, RESPONSE_LABEL_BUDGET_TWO);
        let expected_hashes = pack_truncated_output_hash_from_keys(
            [&stored.outputs[0].key].into_iter(),
            expected_bits,
        )
        .unwrap();
        assert_eq!(response.truncated_output_hash_bits, expected_bits);
        assert_eq!(response.truncated_output_hashes, expected_hashes);
    }

    #[test]
    fn filtered_response_omits_zero_output_reused_tweak_and_adds_skipped_tx() {
        let mut stored = StoredLightBlockInput {
            height: 100,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 10,
            skipped_txs_for_tweaks: vec![],
            tweaks: vec![TweakEntryInput {
                output_count: 1,
                tweak: TxTweak::from([9u8; 33]),
            }],
            skipped_outputs: vec![],
            outputs: vec![StoredOutputEntryInput {
                key: [4u8; 32],
                spent_height: STORAGE_SPENT_HEIGHT_UNSPENT,
                flags: STORAGE_OUTPUT_FLAG_REUSED,
            }],
            spends: vec![],
            raw_block_bytes: 2_500_000,
            truncated_output_hash_for_two_labels: Vec::new(),
            truncated_output_hash_for_hundred_labels: Vec::new(),
        };
        stored.truncated_output_hash_for_two_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_TWO,
        )
        .unwrap();
        stored.truncated_output_hash_for_hundred_labels = build_stored_truncated_output_hashes(
            stored.raw_block_bytes,
            &stored.outputs,
            RESPONSE_LABEL_BUDGET_HUNDRED,
        )
        .unwrap();

        let response = stored
            .to_filtered_response_input(StoredBlockResponseFilter {
                filter_reuse: true,
                ..StoredBlockResponseFilter::default()
            })
            .unwrap();

        assert_eq!(light_block_output_count(&response).unwrap(), 0);
        assert!(response.tweaks.is_empty());
        assert_eq!(response.skipped_txs_for_tweaks, vec![0]);
        assert_eq!(response.truncated_output_hash_bits, 0);
        assert!(response.truncated_output_hashes.is_empty());
    }

    #[test]
    fn unfiltered_response_uses_precomputed_hashes_and_encoded_spent_ids() {
        let stored = stored_block_for_test();
        let response = stored
            .to_response_input_for_labels(Some(RESPONSE_LABEL_BUDGET_TWO))
            .unwrap();

        assert_eq!(
            response.truncated_output_hashes,
            stored.truncated_output_hash_for_two_labels
        );
        assert_eq!(response.spent_count, 1);
        assert_eq!(decode_light_block_spent_ids(&response).unwrap(), vec![41]);
    }
}
