use crate::light_capnp::{light_block, stored_light_block};
use crate::types::{BlockHashBytes, TxTweak};
use crate::WIRE_VERSION;
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use std::collections::BTreeSet;
use std::io::Cursor;

pub const MAX_U16_SECTION_COUNT: usize = u16::MAX as usize;
pub const STORAGE_OUTPUT_FLAG_REUSED: u8 = 1 << 0;
pub const STORAGE_SPENT_HEIGHT_UNSPENT: u32 = u32::MAX;

/// Compatibility alias for code that still uses the old name internally.
pub const OUTPUT_FLAG_REUSED: u8 = STORAGE_OUTPUT_FLAG_REUSED;

#[derive(Debug, Clone)]
pub struct TweakEntryInput {
    /// Number of dense outputs associated with this tweak entry.
    pub output_count: u16,
    /// Compressed 33-byte scan/tweak point. The served payload stores the
    /// 32-byte x-coordinate in `TweakEntry.tweak`.
    pub tweak: TxTweak,
}

#[derive(Debug, Clone)]
pub struct StoredTweakEntryInput {
    /// Number of stored dense outputs associated with this tweak entry.
    pub output_count: u16,
    /// Compressed 33-byte scan/tweak point. Storage stores the 32-byte
    /// x-coordinate on disk.
    pub tweak: TxTweak,
}

#[derive(Debug, Clone)]
pub struct OutputEntryInput {
    /// 32-byte P2TR x-only output key.
    pub key: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct SpendEntryInput {
    /// Global UID spent by this block. The spend height is this block's height.
    pub spent_uid: u64,
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
    /// One dense output key per indexed output.
    pub outputs: Vec<OutputEntryInput>,
    /// One spent UID per spent indexed output in this block.
    pub spends: Vec<SpendEntryInput>,
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
}

impl StoredLightBlockInput {
    pub fn to_response_input(&self) -> LightBlockInput {
        LightBlockInput {
            height: self.height,
            block_hash: self.block_hash,
            previous_block_hash: self.previous_block_hash,
            first_uid: self.first_uid,
            skipped_txs_for_tweaks: self.skipped_txs_for_tweaks.clone(),
            tweaks: self
                .tweaks
                .iter()
                .map(|entry| TweakEntryInput {
                    output_count: entry.output_count,
                    tweak: entry.tweak,
                })
                .collect(),
            outputs: self
                .outputs
                .iter()
                .map(|entry| OutputEntryInput { key: entry.key })
                .collect(),
            spends: self
                .spends
                .iter()
                .map(|entry| SpendEntryInput {
                    spent_uid: entry.spent_uid,
                })
                .collect(),
        }
    }

    pub fn to_filtered_response_input(
        &self,
        filter: StoredBlockResponseFilter,
    ) -> anyhow::Result<LightBlockInput> {
        if !filter.filter_reuse && filter.cutthrough_start.is_none() {
            return Ok(self.to_response_input());
        }

        let mut skipped_txs_for_tweaks = self
            .skipped_txs_for_tweaks
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        let mut outputs = Vec::<OutputEntryInput>::new();
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
                outputs.push(OutputEntryInput { key: output.key });
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

        let spends = self
            .spends
            .iter()
            .filter(|spend| !should_omit_stored_spend(spend, filter))
            .map(|spend| SpendEntryInput {
                spent_uid: spend.spent_uid,
            })
            .collect();

        let response = LightBlockInput {
            height: self.height,
            block_hash: self.block_hash,
            previous_block_hash: self.previous_block_hash,
            first_uid: self.first_uid,
            skipped_txs_for_tweaks: skipped_txs_for_tweaks.into_iter().collect(),
            tweaks,
            outputs,
            spends,
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
    spend: &StoredSpendEntryInput,
    filter: StoredBlockResponseFilter,
) -> bool {
    filter
        .cutthrough_start
        .is_some_and(|start| u64::from(spend.creation_height) >= start)
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

        let mut tweaks = b.reborrow().init_tweaks(input.tweaks.len() as u32);
        for (i, entry) in input.tweaks.iter().enumerate() {
            let mut t = tweaks.reborrow().get(i as u32);
            t.set_output_count(entry.output_count);
            t.set_tweak(tx_tweak_payload_bytes(&entry.tweak)?);
        }

        let mut outputs = b.reborrow().init_outputs(input.outputs.len() as u32);
        for (i, entry) in input.outputs.iter().enumerate() {
            let mut o = outputs.reborrow().get(i as u32);
            o.set_key(&entry.key);
        }

        let mut spends = b.reborrow().init_spends(input.spends.len() as u32);
        for (i, entry) in input.spends.iter().enumerate() {
            let mut s = spends.reborrow().get(i as u32);
            s.set_spent_uid(entry.spent_uid);
        }
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
            t.set_tweak(tx_tweak_payload_bytes(&entry.tweak)?);
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
    }
    Ok(msg)
}

pub fn decode_stored_light_block(bytes: &[u8]) -> anyhow::Result<StoredLightBlockInput> {
    let mut cursor = Cursor::new(bytes);
    let message = capnp::serialize_packed::read_message(&mut cursor, ReaderOptions::new())?;
    let block = message.get_root::<stored_light_block::Reader>()?;

    let skipped_txs_for_tweaks = read_u16_list(block.get_skipped_txs_for_tweaks()?);
    let skipped_outputs = read_u16_list(block.get_skipped_outputs()?);
    let tweak_reader = block.get_tweaks()?;
    let mut tweaks = Vec::with_capacity(tweak_reader.len() as usize);
    for i in 0..tweak_reader.len() {
        let entry = tweak_reader.get(i);
        let tweak = entry.get_tweak()?;
        anyhow::ensure!(tweak.len() == 32, "stored tweak entry {i} is not 32 bytes");
        tweaks.push(StoredTweakEntryInput {
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

pub fn validate_light_block_input(input: &LightBlockInput) -> anyhow::Result<()> {
    ensure_u16_len("skipped txs for tweaks", input.skipped_txs_for_tweaks.len())?;
    ensure_u16_len("tweaks", input.tweaks.len())?;
    ensure_u16_len("outputs", input.outputs.len())?;

    validate_sorted_unique_u16(&input.skipped_txs_for_tweaks, "skipped txs for tweaks")?;

    let declared_outputs = input.tweaks.iter().try_fold(0usize, |acc, entry| {
        acc.checked_add(entry.output_count as usize)
            .ok_or_else(|| anyhow::anyhow!("tweak output count sum overflow"))
    })?;
    anyhow::ensure!(
        declared_outputs == input.outputs.len(),
        "sum(tweak.output_count) {} does not match output count {}",
        declared_outputs,
        input.outputs.len()
    );

    for entry in &input.tweaks {
        anyhow::ensure!(
            tx_tweak_payload_bytes(&entry.tweak)?.len() == 32,
            "tweak payload must be 32 bytes"
        );
    }

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

    let response = input.to_response_input();
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

/// The indexer stores a compressed 33-byte tweak point internally. The served
/// light block carries only the 32-byte x-coordinate.
fn tx_tweak_payload_bytes(tweak: &TxTweak) -> anyhow::Result<&[u8]> {
    let bytes = tweak.as_ref();
    match bytes.len() {
        32 => Ok(bytes),
        33 => Ok(&bytes[1..33]),
        len => anyhow::bail!("unexpected tweak length {len}"),
    }
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

fn read_u16_list(list: capnp::primitive_list::Reader<'_, u16>) -> Vec<u16> {
    let mut out = Vec::with_capacity(list.len() as usize);
    for i in 0..list.len() {
        out.push(list.get(i));
    }
    out
}

fn read_32(bytes: &[u8], label: &str) -> anyhow::Result<[u8; 32]> {
    anyhow::ensure!(bytes.len() == 32, "invalid {label} length {}", bytes.len());
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Ok(out)
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
    fn encodes_typed_light_block() {
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
            outputs: vec![OutputEntryInput { key: [3u8; 32] }],
            spends: vec![SpendEntryInput { spent_uid: 41 }],
        };
        let msg = encode_light_block(&input).unwrap();
        let bytes = to_packed_bytes(&msg).unwrap();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn encodes_and_decodes_stored_light_block_to_response() {
        let stored = StoredLightBlockInput {
            height: 100,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 42,
            skipped_txs_for_tweaks: vec![0, 3],
            tweaks: vec![StoredTweakEntryInput {
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
        };
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

        let (response_bytes, block_hash) = stored_light_block_to_response_bytes(&bytes).unwrap();
        assert_eq!(block_hash.as_bytes(), &[1u8; 32]);
        assert!(!response_bytes.is_empty());
    }

    #[test]
    fn validates_tweak_output_counts_match_outputs() {
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
            outputs: vec![OutputEntryInput { key: [3u8; 32] }],
            spends: vec![],
        };
        assert!(validate_light_block_input(&input).is_err());
    }

    #[test]
    fn filters_reused_outputs_and_recomputes_dense_tweak_counts() {
        let stored = StoredLightBlockInput {
            height: 100,
            block_hash: BlockHashBytes::from([1u8; 32]),
            previous_block_hash: BlockHashBytes::from([2u8; 32]),
            first_uid: 10,
            skipped_txs_for_tweaks: vec![0],
            tweaks: vec![StoredTweakEntryInput {
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
        };
        let response = stored
            .to_filtered_response_input(StoredBlockResponseFilter {
                filter_reuse: true,
                ..StoredBlockResponseFilter::default()
            })
            .unwrap();
        assert_eq!(response.outputs.len(), 1);
        assert_eq!(response.outputs[0].key, [3u8; 32]);
        assert_eq!(response.tweaks[0].output_count, 1);
    }
}
