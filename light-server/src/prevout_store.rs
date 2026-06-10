use crate::p2tr_indexer::{BlockScanInput, OutPointKey};
use crate::script_classify::{classify_script, ScriptKind};
use crate::sp_tweak::{
    compute_tx_scan_point, PrevoutInfo, PrevoutScript, ScanPointStatus, TxInputContext,
};
use crate::types::BlockHashBytes;
use anyhow::Context;
use bitcoin::hashes::Hash as _;
use bitcoin::{key::XOnlyPublicKey, PubkeyHash, ScriptHash, WPubkeyHash};
use rocksdb::{Options as RocksOptions, WriteBatch, DB};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

const HASH160_LEN: usize = 20;
const XONLY_KEY_LEN: usize = 32;
const OUTPOINT_KEY_LEN: usize = 36;
const ROCKS_PREVOUT_KEY_LEN: usize = 2 + OUTPOINT_KEY_LEN;
const ENCODED_PREVOUT_HEADER_LEN: usize = 1;
const ENCODED_PREVOUT_MAX_LEN: usize = ENCODED_PREVOUT_HEADER_LEN + XONLY_KEY_LEN;
const LEGACY_ENCODED_PREVOUT_HEADER_LEN: usize = 8 + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum CompactPrevoutScriptKind {
    P2pkh = 0,
    P2sh = 1,
    P2wpkh = 2,
    P2tr = 3,
    // Spendable but not BIP352-eligible (P2WSH, bare, low-version unknown
    // witness). Stored so a spend resolves to "skip this input" instead of
    // "prevout unknown".
    NonEligible = 4,
    // Spends a segwit > v1 output: per BIP352 this makes the whole tx
    // ineligible. The version byte is retained for the eligibility check.
    WitnessGtV1 = 5,
}

impl CompactPrevoutScriptKind {
    fn from_byte(byte: u8) -> anyhow::Result<Self> {
        match byte {
            0 => Ok(Self::P2pkh),
            1 => Ok(Self::P2sh),
            2 => Ok(Self::P2wpkh),
            3 => Ok(Self::P2tr),
            4 => Ok(Self::NonEligible),
            5 => Ok(Self::WitnessGtV1),
            other => anyhow::bail!("unsupported prevout entry script kind {other}"),
        }
    }
}

fn hash160_from_payload(payload: &[u8], context: &str) -> anyhow::Result<[u8; HASH160_LEN]> {
    anyhow::ensure!(
        payload.len() == HASH160_LEN,
        "invalid {context} payload length"
    );
    let mut bytes = [0u8; HASH160_LEN];
    bytes.copy_from_slice(payload);
    Ok(bytes)
}

fn xonly_key_from_payload(payload: &[u8]) -> anyhow::Result<XOnlyPublicKey> {
    anyhow::ensure!(
        payload.len() == XONLY_KEY_LEN,
        "invalid p2tr prevout payload length"
    );
    XOnlyPublicKey::from_slice(payload).context("invalid p2tr x-only output key")
}

const META_TIP_HEIGHT_KEY: &[u8] = b"m:tip_height";
const META_TIP_HASH_KEY: &[u8] = b"m:tip_hash";

#[derive(Debug, Clone, Copy)]
enum CompactPrevoutScript {
    P2pkh { pubkey_hash: PubkeyHash },
    P2sh { script_hash: ScriptHash },
    P2wpkh { pubkey_hash: WPubkeyHash },
    P2tr { output_key: XOnlyPublicKey },
    /// Seen + spendable but not a BIP352-eligible input type.
    NonEligible,
    /// Spends a segwit version > 1 output (carries the version).
    WitnessGtV1 { version: u8 },
}

#[derive(Debug, Clone, Copy)]
struct CompactPrevoutContext {
    script: CompactPrevoutScript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct OutpointBytes([u8; OUTPOINT_KEY_LEN]);

impl OutpointBytes {
    fn from_outpoint(outpoint: &OutPointKey) -> Self {
        let mut bytes = [0u8; OUTPOINT_KEY_LEN];
        bytes[..32].copy_from_slice(outpoint.txid.as_bytes());
        bytes[32..].copy_from_slice(&outpoint.vout.to_le_bytes());
        Self(bytes)
    }
}

impl AsRef<[u8]> for OutpointBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RocksPrevoutKey([u8; ROCKS_PREVOUT_KEY_LEN]);

impl RocksPrevoutKey {
    fn from_outpoint(outpoint: &OutPointKey) -> Self {
        let mut bytes = [0u8; ROCKS_PREVOUT_KEY_LEN];
        bytes[..2].copy_from_slice(b"p:");
        bytes[2..].copy_from_slice(OutpointBytes::from_outpoint(outpoint).as_ref());
        Self(bytes)
    }
}

impl AsRef<[u8]> for RocksPrevoutKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy)]
struct EncodedPrevoutEntry {
    bytes: [u8; ENCODED_PREVOUT_MAX_LEN],
    len: usize,
}

impl EncodedPrevoutEntry {
    fn new(entry: CompactPrevoutContext) -> Self {
        let mut bytes = [0u8; ENCODED_PREVOUT_MAX_LEN];
        let len = match entry.script {
            CompactPrevoutScript::P2pkh { pubkey_hash } => {
                bytes[0] = CompactPrevoutScriptKind::P2pkh as u8;
                bytes[1..1 + HASH160_LEN].copy_from_slice(pubkey_hash.as_ref());
                ENCODED_PREVOUT_HEADER_LEN + HASH160_LEN
            }
            CompactPrevoutScript::P2sh { script_hash } => {
                bytes[0] = CompactPrevoutScriptKind::P2sh as u8;
                bytes[1..1 + HASH160_LEN].copy_from_slice(script_hash.as_ref());
                ENCODED_PREVOUT_HEADER_LEN + HASH160_LEN
            }
            CompactPrevoutScript::P2wpkh { pubkey_hash } => {
                bytes[0] = CompactPrevoutScriptKind::P2wpkh as u8;
                bytes[1..1 + HASH160_LEN].copy_from_slice(pubkey_hash.as_ref());
                ENCODED_PREVOUT_HEADER_LEN + HASH160_LEN
            }
            CompactPrevoutScript::P2tr { output_key } => {
                bytes[0] = CompactPrevoutScriptKind::P2tr as u8;
                bytes[1..1 + XONLY_KEY_LEN].copy_from_slice(&output_key.serialize());
                ENCODED_PREVOUT_HEADER_LEN + XONLY_KEY_LEN
            }
            CompactPrevoutScript::NonEligible => {
                bytes[0] = CompactPrevoutScriptKind::NonEligible as u8;
                ENCODED_PREVOUT_HEADER_LEN
            }
            CompactPrevoutScript::WitnessGtV1 { version } => {
                bytes[0] = CompactPrevoutScriptKind::WitnessGtV1 as u8;
                bytes[1] = version;
                ENCODED_PREVOUT_HEADER_LEN + 1
            }
        };
        Self { bytes, len }
    }
}

impl AsRef<[u8]> for EncodedPrevoutEntry {
    fn as_ref(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub struct PrevoutStore {
    db: DB,
    tip_height: Option<u64>,
    tip_hash: Option<BlockHashBytes>,
}

#[derive(Debug, Default)]
pub struct PendingPrevoutWrites {
    puts: HashMap<OutPointKey, CompactPrevoutContext>,
    deletes: HashSet<OutPointKey>,
}

#[derive(Debug, Clone, Copy)]
enum PendingPrevoutLookup {
    Present(CompactPrevoutContext),
    Tombstoned,
    Absent,
}

impl PendingPrevoutWrites {
    pub fn is_empty(&self) -> bool {
        self.puts.is_empty() && self.deletes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.puts.len() + self.deletes.len()
    }

    fn lookup(&self, outpoint: &OutPointKey) -> PendingPrevoutLookup {
        if let Some(entry) = self.puts.get(outpoint).copied() {
            return PendingPrevoutLookup::Present(entry);
        }
        if self.deletes.contains(outpoint) {
            return PendingPrevoutLookup::Tombstoned;
        }
        PendingPrevoutLookup::Absent
    }

    fn put(&mut self, outpoint: OutPointKey, entry: CompactPrevoutContext) {
        self.deletes.remove(&outpoint);
        self.puts.insert(outpoint, entry);
    }

    fn delete(&mut self, outpoint: OutPointKey) {
        if self.puts.remove(&outpoint).is_none() {
            self.deletes.insert(outpoint);
        }
    }
}

fn compact_prevout_script(script_pubkey: &[u8]) -> Option<CompactPrevoutScript> {
    match classify_script(script_pubkey) {
        ScriptKind::P2pkh { hash160 } => Some(CompactPrevoutScript::P2pkh {
            pubkey_hash: PubkeyHash::from_byte_array(hash160),
        }),
        ScriptKind::P2sh { hash160 } => Some(CompactPrevoutScript::P2sh {
            script_hash: ScriptHash::from_byte_array(hash160),
        }),
        ScriptKind::P2wpkh { hash160 } => Some(CompactPrevoutScript::P2wpkh {
            pubkey_hash: WPubkeyHash::from_byte_array(hash160),
        }),
        ScriptKind::P2tr { xonly_key } => Some(CompactPrevoutScript::P2tr {
            output_key: XOnlyPublicKey::from_slice(&xonly_key).ok()?,
        }),
        // Segwit > v1: spending such an output makes the tx ineligible, so the
        // version must survive to the eligibility check rather than vanish.
        ScriptKind::WitnessUnknown { version, .. } if version >= 2 => {
            Some(CompactPrevoutScript::WitnessGtV1 { version })
        }
        // Spendable but never a BIP352 input (P2WSH, bare/non-standard, and
        // low-version unknown witness programs). Stored so a later spend is
        // "skip this input", not "prevout unknown".
        ScriptKind::P2wsh { .. } | ScriptKind::WitnessUnknown { .. } | ScriptKind::Other => {
            Some(CompactPrevoutScript::NonEligible)
        }
        // Provably unspendable: it can never appear as a spent prevout, so
        // storing it would be pure waste.
        ScriptKind::OpReturn => None,
    }
}

fn prevout_info_from_compact(script: CompactPrevoutScript) -> PrevoutInfo {
    let script = match script {
        CompactPrevoutScript::P2pkh { pubkey_hash } => PrevoutScript::P2pkh {
            hash160: pubkey_hash.to_byte_array(),
        },
        CompactPrevoutScript::P2sh { script_hash } => PrevoutScript::P2sh {
            hash160: script_hash.to_byte_array(),
        },
        CompactPrevoutScript::P2wpkh { pubkey_hash } => PrevoutScript::P2wpkh {
            hash160: pubkey_hash.to_byte_array(),
        },
        CompactPrevoutScript::P2tr { output_key } => PrevoutScript::P2tr {
            xonly_key: output_key,
        },
        CompactPrevoutScript::NonEligible => PrevoutScript::Other,
        CompactPrevoutScript::WitnessGtV1 { version } => {
            PrevoutScript::WitnessUnknown { version }
        }
    };
    PrevoutInfo { script }
}

fn decode_prevout_entry(bytes: &[u8]) -> anyhow::Result<CompactPrevoutContext> {
    anyhow::ensure!(
        bytes.len() >= ENCODED_PREVOUT_HEADER_LEN,
        "prevout entry is too short"
    );

    // v1 RocksDB entries stored an unused u64 value before the script kind.
    // The scan-point code only needs script context, so new entries omit it.
    // Decode both shapes so existing stores continue to read correctly; old
    // entries naturally disappear as their outpoints are spent or the store is
    // rebuilt.
    let (kind, payload) = match bytes.len() {
        // New compact entries: kind byte + optional payload. NonEligible has no
        // payload (len 1); WitnessGtV1 carries a 1-byte version (len 2).
        len if len == ENCODED_PREVOUT_HEADER_LEN
            || len == ENCODED_PREVOUT_HEADER_LEN + 1
            || len == ENCODED_PREVOUT_HEADER_LEN + HASH160_LEN
            || len == ENCODED_PREVOUT_HEADER_LEN + XONLY_KEY_LEN =>
        {
            (bytes[0], &bytes[1..])
        }
        len if len == LEGACY_ENCODED_PREVOUT_HEADER_LEN + HASH160_LEN
            || len == LEGACY_ENCODED_PREVOUT_HEADER_LEN + XONLY_KEY_LEN =>
        {
            (bytes[8], &bytes[9..])
        }
        len => anyhow::bail!("invalid prevout entry length {len}"),
    };

    let script = match CompactPrevoutScriptKind::from_byte(kind)? {
        CompactPrevoutScriptKind::P2pkh => CompactPrevoutScript::P2pkh {
            pubkey_hash: PubkeyHash::from_byte_array(hash160_from_payload(
                payload,
                "p2pkh prevout",
            )?),
        },
        CompactPrevoutScriptKind::P2sh => CompactPrevoutScript::P2sh {
            script_hash: ScriptHash::from_byte_array(hash160_from_payload(
                payload,
                "p2sh prevout",
            )?),
        },
        CompactPrevoutScriptKind::P2wpkh => CompactPrevoutScript::P2wpkh {
            pubkey_hash: WPubkeyHash::from_byte_array(hash160_from_payload(
                payload,
                "p2wpkh prevout",
            )?),
        },
        CompactPrevoutScriptKind::P2tr => CompactPrevoutScript::P2tr {
            output_key: xonly_key_from_payload(payload)?,
        },
        CompactPrevoutScriptKind::NonEligible => CompactPrevoutScript::NonEligible,
        CompactPrevoutScriptKind::WitnessGtV1 => {
            anyhow::ensure!(
                payload.len() == 1,
                "invalid witness>v1 prevout payload length"
            );
            CompactPrevoutScript::WitnessGtV1 { version: payload[0] }
        }
    };
    Ok(CompactPrevoutContext { script })
}

impl PrevoutStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(path)?;
        let mut opts = RocksOptions::default();
        opts.create_if_missing(true);
        opts.set_max_open_files(1024);
        opts.set_use_fsync(false);
        opts.set_keep_log_file_num(8);
        let db = DB::open(&opts, path).with_context(|| {
            format!("failed to open RocksDB prevout store at {}", path.display())
        })?;

        let tip_height = db
            .get(META_TIP_HEIGHT_KEY)?
            .map(|bytes| {
                anyhow::ensure!(
                    bytes.len() == 8,
                    "invalid prevout store tip height metadata"
                );
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&bytes);
                Ok::<_, anyhow::Error>(u64::from_le_bytes(buf))
            })
            .transpose()?;
        let tip_hash = db
            .get(META_TIP_HASH_KEY)?
            .map(|bytes| {
                anyhow::ensure!(bytes.len() == 32, "invalid prevout store tip hash metadata");
                let mut buf = [0u8; 32];
                buf.copy_from_slice(&bytes);
                Ok::<_, anyhow::Error>(BlockHashBytes::from(buf))
            })
            .transpose()?;

        Ok(Self {
            db,
            tip_height,
            tip_hash,
        })
    }

    pub fn tip(&self) -> (Option<u64>, Option<BlockHashBytes>) {
        (self.tip_height, self.tip_hash)
    }

    pub fn len_estimate(&self) -> usize {
        self.db
            .property_int_value("rocksdb.estimate-num-keys")
            .ok()
            .flatten()
            .unwrap_or(0) as usize
    }

    fn lookup_prevout(
        &self,
        pending: &PendingPrevoutWrites,
        outpoint: &OutPointKey,
    ) -> anyhow::Result<Option<CompactPrevoutContext>> {
        match pending.lookup(outpoint) {
            PendingPrevoutLookup::Present(entry) => Ok(Some(entry)),
            PendingPrevoutLookup::Tombstoned => Ok(None),
            PendingPrevoutLookup::Absent => self
                .db
                .get(RocksPrevoutKey::from_outpoint(outpoint))?
                .map(|bytes| decode_prevout_entry(&bytes))
                .transpose(),
        }
    }

    pub fn apply_pending(
        &mut self,
        pending: PendingPrevoutWrites,
        height: u64,
        block_hash: BlockHashBytes,
    ) -> anyhow::Result<()> {
        let mut batch = WriteBatch::default();
        for outpoint in pending.deletes {
            batch.delete(RocksPrevoutKey::from_outpoint(&outpoint));
        }
        for (outpoint, entry) in pending.puts {
            batch.put(
                RocksPrevoutKey::from_outpoint(&outpoint),
                EncodedPrevoutEntry::new(entry),
            );
        }
        batch.put(META_TIP_HEIGHT_KEY, height.to_le_bytes());
        batch.put(META_TIP_HASH_KEY, block_hash.as_bytes());
        self.db.write(batch)?;
        self.db.flush()?;
        self.tip_height = Some(height);
        self.tip_hash = Some(block_hash);
        Ok(())
    }

    pub fn enrich_block_scan_points(
        &self,
        pending: &mut PendingPrevoutWrites,
        block: &mut BlockScanInput,
    ) -> anyhow::Result<PrevoutMutationStats> {
        let mut stats = PrevoutMutationStats::default();

        for tx in &mut block.txs {
            self.enrich_tx_scan_point(pending, tx, &mut stats)?;
            Self::apply_tx_spends(pending, tx, &mut stats);
            Self::apply_tx_outputs(pending, tx, &mut stats);
        }

        Ok(stats)
    }

    fn enrich_tx_scan_point(
        &self,
        pending: &PendingPrevoutWrites,
        tx: &mut crate::p2tr_indexer::TxScanInput,
        stats: &mut PrevoutMutationStats,
    ) -> anyhow::Result<()> {
        let needs_scan_point = tx.outputs.iter().any(|output| output.is_p2tr);
        if !needs_scan_point {
            tx.silent_payment_tweak = None;
            return Ok(());
        }

        let mut input_context = Vec::with_capacity(tx.inputs.len());
        for input in &tx.inputs {
            if !input.previous_output.is_coinbase() {
                stats.scan_point_prevout_lookups += 1;
            }
            let entry = self.lookup_prevout(pending, &input.previous_output)?;
            let prevout = entry.map(|entry| prevout_info_from_compact(entry.script));
            input_context.push(TxInputContext {
                previous_output: input.previous_output,
                script_sig: input.script_sig.clone(),
                witness: input.witness.clone(),
                prevout,
            });
        }

        match compute_tx_scan_point(&input_context)? {
            ScanPointStatus::Computed(tweak) => tx.silent_payment_tweak = Some(tweak),
            ScanPointStatus::Ineligible | ScanPointStatus::MissingPrevout { .. } => {
                tx.silent_payment_tweak = None
            }
        }

        Ok(())
    }

    fn apply_tx_spends(
        pending: &mut PendingPrevoutWrites,
        tx: &crate::p2tr_indexer::TxScanInput,
        stats: &mut PrevoutMutationStats,
    ) {
        for input in &tx.inputs {
            if !input.previous_output.is_coinbase() {
                pending.delete(input.previous_output);
                stats.delete_attempts += 1;
            }
        }
    }

    fn apply_tx_outputs(
        pending: &mut PendingPrevoutWrites,
        tx: &crate::p2tr_indexer::TxScanInput,
        stats: &mut PrevoutMutationStats,
    ) {
        for output in &tx.outputs {
            let Some(script) = compact_prevout_script(&output.script_pubkey) else {
                continue;
            };
            let outpoint = OutPointKey {
                txid: tx.txid,
                vout: output.vout,
            };
            pending.put(outpoint, CompactPrevoutContext { script });
            stats.contexts_created += 1;
        }
    }
}

#[derive(Debug, Default)]
pub struct PrevoutMutationStats {
    pub contexts_created: usize,
    pub delete_attempts: usize,
    pub scan_point_prevout_lookups: usize,
}
