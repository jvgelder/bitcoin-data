//! Minimal in-memory shapes for blocks at the source-pipeline boundary.
//!
//! These are intentionally tiny — just enough metadata to route a block
//! through the system. Decoded block contents are produced by the shared `core::parse` module.
//! Sources never decode blocks themselves; they only hand raw consensus bytes
//! to the core pipeline.

use bytes::Bytes;

/// A spent previous output decoded from Bitcoin Core's `/rest/spenttxouts` undo payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpentTxOut {
    pub value_sat: u64,
    pub script_pubkey: Vec<u8>,
}

/// Per-block undo data aligned with `block.txdata[1..]` and each transaction's inputs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockSpentTxOuts {
    pub txs: Vec<Vec<SpentTxOut>>,
}

/// A raw, consensus-encoded block plus the metadata needed to route it.
///
/// The block payload is stored as [`Bytes`] instead of `Vec<u8>` so sources
/// that already receive shared byte buffers, such as HTTP clients, can hand
/// them through the pipeline without copying. Sources that naturally produce
/// owned `Vec<u8>` can convert with `Bytes::from(vec)`.
#[derive(Clone, Debug)]
pub struct RawBlockFrame {
    pub height: u64,
    pub hash: [u8; 32],
    pub bytes: Bytes,
    pub spent_txouts: Option<BlockSpentTxOuts>,
}

/// Tip / chain-tip notification (no block body).
#[derive(Clone, Copy, Debug)]
pub struct BlockTipFrame {
    pub height: u64,
    pub hash: [u8; 32],
}

/// Block-hash-only event (e.g. ZMQ `hashblock`).
#[derive(Clone, Copy, Debug)]
pub struct BlockHashFrame {
    pub hash: [u8; 32],
}

/// Decode Bitcoin Core REST `/rest/spenttxouts/<blockhash>.bin` bytes.
///
/// The project endpoint returns a block-level undo payload as:
///
/// ```text
/// CompactSize tx_count
/// repeat tx_count times:
///   CompactSize input_count
///   repeat input_count times:
///     CTxOut(value, scriptPubKey)
/// ```
///
/// The outer vector is aligned with `block.txdata`: entry 0 is the
/// coinbase transaction and should have zero inputs. Each later entry is
/// aligned with that transaction's inputs. Each spent output itself is encoded like a normal
/// Bitcoin `CTxOut`: signed little-endian `nValue`, CompactSize script length,
/// then raw script bytes. This is not the on-disk `Coin` compression format.
pub fn decode_spent_txouts_payload(bytes: &[u8]) -> anyhow::Result<BlockSpentTxOuts> {
    let mut cursor = Cursor::new(bytes);
    let tx_count = cursor.read_compact_size()?;
    let mut txs = Vec::with_capacity(usize::try_from(tx_count)?);

    for _ in 0..tx_count {
        let input_count = cursor.read_compact_size()?;
        let mut inputs = Vec::with_capacity(usize::try_from(input_count)?);
        for _ in 0..input_count {
            let value_sat = cursor.read_txout_value()?;
            let script_pubkey = cursor.read_raw_script()?;
            inputs.push(SpentTxOut {
                value_sat,
                script_pubkey,
            });
        }
        txs.push(inputs);
    }

    anyhow::ensure!(
        cursor.is_empty(),
        "spenttxouts payload has {} trailing bytes",
        cursor.remaining()
    );
    Ok(BlockSpentTxOuts { txs })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn is_empty(&self) -> bool {
        self.pos == self.bytes.len()
    }

    fn read_u8(&mut self) -> anyhow::Result<u8> {
        let Some(byte) = self.bytes.get(self.pos).copied() else {
            anyhow::bail!("unexpected EOF while decoding spenttxouts");
        };
        self.pos += 1;
        Ok(byte)
    }

    fn read_exact(&mut self, len: usize) -> anyhow::Result<&'a [u8]> {
        anyhow::ensure!(
            self.remaining() >= len,
            "unexpected EOF while decoding spenttxouts: need {len}, have {}",
            self.remaining()
        );
        let start = self.pos;
        self.pos += len;
        Ok(&self.bytes[start..start + len])
    }

    fn read_compact_size(&mut self) -> anyhow::Result<u64> {
        let first = self.read_u8()?;
        match first {
            0x00..=0xfc => Ok(u64::from(first)),
            0xfd => {
                let b = self.read_exact(2)?;
                let value = u16::from_le_bytes([b[0], b[1]]) as u64;
                anyhow::ensure!(value >= 0xfd, "non-canonical CompactSize");
                Ok(value)
            }
            0xfe => {
                let b = self.read_exact(4)?;
                let value = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64;
                anyhow::ensure!(value > 0xffff, "non-canonical CompactSize");
                Ok(value)
            }
            0xff => {
                let b = self.read_exact(8)?;
                let value = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
                anyhow::ensure!(value > 0xffff_ffff, "non-canonical CompactSize");
                Ok(value)
            }
        }
    }

    fn read_txout_value(&mut self) -> anyhow::Result<u64> {
        let b = self.read_exact(8)?;
        let value = i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        anyhow::ensure!(value >= 0, "spenttxouts contains negative txout value");
        Ok(value as u64)
    }

    fn read_raw_script(&mut self) -> anyhow::Result<Vec<u8>> {
        let len = usize::try_from(self.read_compact_size()?)?;
        Ok(self.read_exact(len)?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_empty_block_undo() {
        assert_eq!(decode_spent_txouts_payload(&[0]).unwrap(), BlockSpentTxOuts::default());
    }

    #[test]
    fn decodes_raw_txout_payload() {
        let mut payload = Vec::new();
        payload.push(1); // one non-coinbase tx undo
        payload.push(1); // one input
        payload.extend_from_slice(&1_309_258i64.to_le_bytes());
        payload.push(34);
        payload.extend([0x51, 0x20]);
        payload.extend([0x42; 32]);

        let decoded = decode_spent_txouts_payload(&payload).unwrap();
        assert_eq!(decoded.txs.len(), 1);
        assert_eq!(decoded.txs[0].len(), 1);
        assert_eq!(decoded.txs[0][0].value_sat, 1_309_258);
        assert_eq!(decoded.txs[0][0].script_pubkey.len(), 34);
        assert_eq!(decoded.txs[0][0].script_pubkey[0..2], [0x51, 0x20]);
    }
}
