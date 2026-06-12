//! BIP352 Silent Payments transaction scan-point computation.
//!
//! The light archive serves one 33-byte compressed public scan point per
//! eligible transaction. For BIP352 v0 this point is `input_hash * A`, where
//! `A` is the sum of eligible input public keys and `input_hash` commits to the
//! lexicographically-smallest input outpoint plus `A`. A scanning wallet can then
//! compute `b_scan * (input_hash * A)` without downloading full transactions.

use crate::p2tr_indexer::OutPointKey;
use crate::script_classify::{classify_script, ScriptKind};
use crate::types::TxTweak;
use bitcoin::hashes::{hash160, Hash};
use bitcoin::secp256k1::{Parity, PublicKey, Scalar, Secp256k1, XOnlyPublicKey};
use sha2::{Digest, Sha256};

const BIP352_INPUTS_TAG: &str = "BIP0352/Inputs";
const TAPROOT_NUMS_H_XONLY: [u8; 32] = [
    0x50, 0x92, 0x9b, 0x74, 0xc1, 0xa0, 0x49, 0x54, 0xb7, 0x8b, 0x4b, 0x60, 0x35, 0xe9, 0x7a, 0x5e,
    0x07, 0x8a, 0x5a, 0x0f, 0x28, 0xec, 0x96, 0xd5, 0x47, 0xbf, 0xee, 0x9a, 0xce, 0x80, 0x3a, 0xc0,
];

#[derive(Debug, thiserror::Error)]
pub enum SpTweakError {
    #[error("eligible input public-key sum is infinity")]
    PublicKeySumInfinity,
    #[error("non-empty non-coinbase input set expected")]
    EmptyNonCoinbaseInputSet,
    #[error("BIP352 input hash is not a valid secp256k1 scalar")]
    InvalidInputHashScalar,
    #[error("BIP352 scan point multiplication failed")]
    ScanPointMultiplication,
    #[error("invalid compressed input public key")]
    InvalidCompressedInputPublicKey {
        #[source]
        source: bitcoin::secp256k1::Error,
    },
}

pub type SpTweakResult<T> = Result<T, SpTweakError>;


#[derive(Debug, Clone, Copy)]
pub struct PrevoutInfo {
    pub script: PrevoutScript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrevoutScript {
    P2pkh { hash160: [u8; 20] },
    P2sh { hash160: [u8; 20] },
    P2wpkh { hash160: [u8; 20] },
    P2tr { xonly_key: XOnlyPublicKey },
    WitnessUnknown { version: u8 },
    Other,
}

#[derive(Debug, Clone)]
pub struct TxInputContext {
    pub previous_output: OutPointKey,
    pub script_sig: Vec<u8>,
    pub witness: Vec<Vec<u8>>,
    pub prevout: Option<PrevoutInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPointStatus {
    /// The transaction cannot create BIP352 outputs because no eligible input
    /// material was found or a BIP352 v0 exclusion applies.
    Ineligible,
    /// At least one non-coinbase input is missing prevout context. Computing a
    /// partial scan point would be incorrect, so the indexer must omit it.
    MissingPrevout { missing_count: usize },
    /// A real 33-byte compressed public scan point was computed.
    Computed(TxTweak),
}

pub fn compute_tx_scan_point(inputs: &[TxInputContext]) -> SpTweakResult<ScanPointStatus> {
    if inputs.is_empty()
        || inputs
            .iter()
            .all(|input| input.previous_output.is_coinbase())
    {
        return Ok(ScanPointStatus::Ineligible);
    }

    let missing_count = inputs
        .iter()
        .filter(|input| !input.previous_output.is_coinbase() && input.prevout.is_none())
        .count();
    if missing_count > 0 {
        return Ok(ScanPointStatus::MissingPrevout { missing_count });
    }

    if spends_witness_version_greater_than_one(inputs) {
        return Ok(ScanPointStatus::Ineligible);
    }

    let secp = Secp256k1::verification_only();
    let mut eligible_pubkeys = Vec::<PublicKey>::new();

    for input in inputs {
        if input.previous_output.is_coinbase() {
            continue;
        }
        let prevout = input
            .prevout
            .as_ref()
            .expect("missing prevout handled before scan-point computation");

        if let Some(pubkey) = extract_bip352_input_pubkey(input, prevout)? {
            eligible_pubkeys.push(pubkey);
        }
    }

    if eligible_pubkeys.is_empty() {
        return Ok(ScanPointStatus::Ineligible);
    }

    let pubkey_refs = eligible_pubkeys.iter().collect::<Vec<_>>();
    let sum = PublicKey::combine_keys(&pubkey_refs)
        .map_err(|_| SpTweakError::PublicKeySumInfinity)?;

    let outpoint_l = smallest_non_coinbase_outpoint(inputs)
        .ok_or(SpTweakError::EmptyNonCoinbaseInputSet)?;
    let mut input_hash_preimage = Vec::with_capacity(36 + 33);
    input_hash_preimage.extend_from_slice(&outpoint_l);
    input_hash_preimage.extend_from_slice(&sum.serialize());

    let input_hash = tagged_sha256(BIP352_INPUTS_TAG, &input_hash_preimage);
    let scalar = Scalar::from_be_bytes(input_hash)
        .map_err(|_| SpTweakError::InvalidInputHashScalar)?;

    let scan_point = sum
        .mul_tweak(&secp, &scalar)
        .map_err(|_| SpTweakError::ScanPointMultiplication)?;

    Ok(ScanPointStatus::Computed(TxTweak::from(
        scan_point.serialize(),
    )))
}

fn spends_witness_version_greater_than_one(inputs: &[TxInputContext]) -> bool {
    inputs.iter().any(|input| {
        input.prevout.as_ref().is_some_and(|prevout| {
            matches!(
                prevout.script,
                PrevoutScript::WitnessUnknown { version: 2..=16 }
            )
        })
    })
}

fn extract_bip352_input_pubkey(
    input: &TxInputContext,
    prevout: &PrevoutInfo,
) -> SpTweakResult<Option<PublicKey>> {
    match prevout.script {
        PrevoutScript::P2tr { xonly_key } => extract_p2tr_input_pubkey(input, xonly_key),
        PrevoutScript::P2wpkh { hash160 } => extract_p2wpkh_input_pubkey(input, hash160),
        PrevoutScript::P2sh { hash160 } => extract_p2sh_p2wpkh_input_pubkey(input, hash160),
        PrevoutScript::P2pkh { hash160 } => extract_p2pkh_input_pubkey(input, hash160),
        PrevoutScript::WitnessUnknown { .. } | PrevoutScript::Other => Ok(None),
    }
}

fn extract_p2tr_input_pubkey(
    input: &TxInputContext,
    xonly_key: XOnlyPublicKey,
) -> SpTweakResult<Option<PublicKey>> {
    if let Some(internal_key) = taproot_script_path_internal_key(&input.witness) {
        if internal_key == TAPROOT_NUMS_H_XONLY {
            return Ok(None);
        }
    }

    Ok(Some(xonly_key.public_key(Parity::Even)))
}

fn extract_p2wpkh_input_pubkey(
    input: &TxInputContext,
    expected_hash: [u8; 20],
) -> SpTweakResult<Option<PublicKey>> {
    let Some(pubkey_bytes) = input.witness.last() else {
        return Ok(None);
    };
    parse_compressed_pubkey_matching_hash(pubkey_bytes, expected_hash)
}

fn extract_p2sh_p2wpkh_input_pubkey(
    input: &TxInputContext,
    expected_script_hash: [u8; 20],
) -> SpTweakResult<Option<PublicKey>> {
    let pushes = parse_script_pushes(&input.script_sig);
    let Some(redeem_script) = pushes
        .iter()
        .rev()
        .find(|push| matches!(classify_script(push), ScriptKind::P2wpkh { .. }))
    else {
        return Ok(None);
    };

    if hash160_bytes(redeem_script) != expected_script_hash {
        return Ok(None);
    }

    let mut pubkey_hash = [0u8; 20];
    pubkey_hash.copy_from_slice(&redeem_script[2..22]);
    extract_p2wpkh_input_pubkey(input, pubkey_hash)
}

fn extract_p2pkh_input_pubkey(
    input: &TxInputContext,
    expected_hash: [u8; 20],
) -> SpTweakResult<Option<PublicKey>> {
    for push in parse_script_pushes(&input.script_sig).into_iter().rev() {
        if let Some(pubkey) = parse_compressed_pubkey_matching_hash(&push, expected_hash)? {
            return Ok(Some(pubkey));
        }
    }
    Ok(None)
}

fn parse_compressed_pubkey_matching_hash(
    bytes: &[u8],
    expected_hash: [u8; 20],
) -> SpTweakResult<Option<PublicKey>> {
    if bytes.len() != 33 || !matches!(bytes.first(), Some(0x02 | 0x03)) {
        return Ok(None);
    }
    if hash160_bytes(bytes) != expected_hash {
        return Ok(None);
    }
    Ok(Some(
        PublicKey::from_slice(bytes)
            .map_err(|source| SpTweakError::InvalidCompressedInputPublicKey { source })?,
    ))
}

fn taproot_script_path_internal_key(witness: &[Vec<u8>]) -> Option<[u8; 32]> {
    let control_block = witness.last()?;
    if control_block.len() < 33 || (control_block.len() - 33) % 32 != 0 || witness.len() < 2 {
        return None;
    }

    let mut internal_key = [0u8; 32];
    internal_key.copy_from_slice(&control_block[1..33]);
    Some(internal_key)
}

fn smallest_non_coinbase_outpoint(inputs: &[TxInputContext]) -> Option<[u8; 36]> {
    inputs
        .iter()
        .filter(|input| !input.previous_output.is_coinbase())
        .map(serialize_outpoint)
        .min()
}

fn serialize_outpoint(input: &TxInputContext) -> [u8; 36] {
    let mut outpoint = [0u8; 36];
    outpoint[..32].copy_from_slice(input.previous_output.txid.as_bytes());
    outpoint[32..].copy_from_slice(&input.previous_output.vout.to_le_bytes());
    outpoint
}

fn tagged_sha256(tag: &str, msg: &[u8]) -> [u8; 32] {
    let tag_hash = Sha256::digest(tag.as_bytes());
    let mut hasher = Sha256::new();
    hasher.update(tag_hash);
    hasher.update(tag_hash);
    hasher.update(msg);
    hasher.finalize().into()
}

fn hash160_bytes(bytes: &[u8]) -> [u8; 20] {
    hash160::Hash::hash(bytes).to_byte_array()
}

fn parse_script_pushes(script: &[u8]) -> Vec<Vec<u8>> {
    let mut pushes = Vec::new();
    let mut i = 0usize;

    while i < script.len() {
        let opcode = script[i];
        i += 1;

        let len = match opcode {
            0x01..=0x4b => opcode as usize,
            0x4c => {
                if i >= script.len() {
                    break;
                }
                let len = script[i] as usize;
                i += 1;
                len
            }
            0x4d => {
                if i + 2 > script.len() {
                    break;
                }
                let len = u16::from_le_bytes([script[i], script[i + 1]]) as usize;
                i += 2;
                len
            }
            0x4e => {
                if i + 4 > script.len() {
                    break;
                }
                let len =
                    u32::from_le_bytes([script[i], script[i + 1], script[i + 2], script[i + 3]])
                        as usize;
                i += 4;
                len
            }
            _ => continue,
        };

        if i + len > script.len() {
            break;
        }
        pushes.push(script[i..i + len].to_vec());
        i += len;
    }

    pushes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TxidBytes;

    const GENERATOR_COMPRESSED: [u8; 33] = [
        0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87,
        0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16,
        0xf8, 0x17, 0x98,
    ];
    const GENERATOR_XONLY: [u8; 32] = [
        0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
        0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8,
        0x17, 0x98,
    ];

    fn outpoint(n: u8) -> OutPointKey {
        OutPointKey {
            txid: TxidBytes::from([n; 32]),
            vout: 0,
        }
    }

    #[test]
    fn empty_prevouts_are_ineligible() {
        assert_eq!(
            compute_tx_scan_point(&[]).unwrap(),
            ScanPointStatus::Ineligible
        );
    }

    #[test]
    fn missing_prevout_blocks_partial_computation() {
        let status = compute_tx_scan_point(&[TxInputContext {
            previous_output: outpoint(1),
            script_sig: Vec::new(),
            witness: Vec::new(),
            prevout: None,
        }])
        .unwrap();
        assert_eq!(status, ScanPointStatus::MissingPrevout { missing_count: 1 });
    }

    #[test]
    fn p2tr_prevout_with_witness_computes_scan_point() {
        let status = compute_tx_scan_point(&[TxInputContext {
            previous_output: outpoint(1),
            script_sig: Vec::new(),
            witness: vec![vec![1; 64]],
            prevout: Some(PrevoutInfo {
                script: PrevoutScript::P2tr {
                    xonly_key: XOnlyPublicKey::from_slice(&GENERATOR_XONLY).unwrap(),
                },
            }),
        }])
        .unwrap();

        match status {
            ScanPointStatus::Computed(tweak) => assert_eq!(tweak.as_bytes().len(), 33),
            other => panic!("expected computed scan point, got {other:?}"),
        }
    }

    #[test]
    fn p2wpkh_prevout_computes_scan_point_from_witness_pubkey() {
        let status = compute_tx_scan_point(&[TxInputContext {
            previous_output: outpoint(2),
            script_sig: Vec::new(),
            witness: vec![vec![1; 64], GENERATOR_COMPRESSED.to_vec()],
            prevout: Some(PrevoutInfo {
                script: PrevoutScript::P2wpkh {
                    hash160: hash160_bytes(&GENERATOR_COMPRESSED),
                },
            }),
        }])
        .unwrap();

        assert!(matches!(status, ScanPointStatus::Computed(_)));
    }

    #[test]
    fn segwit_version_greater_than_one_makes_transaction_ineligible() {
        let status = compute_tx_scan_point(&[TxInputContext {
            previous_output: outpoint(3),
            script_sig: Vec::new(),
            witness: Vec::new(),
            prevout: Some(PrevoutInfo {
                script: PrevoutScript::WitnessUnknown { version: 2 },
            }),
        }])
        .unwrap();

        assert_eq!(status, ScanPointStatus::Ineligible);
    }

    #[test]
    fn non_eligible_input_is_skipped_not_treated_as_missing() {
        // Regression: a tx spending an eligible P2WPKH input alongside a
        // non-eligible (e.g. P2WSH) input must still produce a scan point from
        // the eligible input. Previously the non-eligible prevout was never
        // stored, read back as `None`, and wrongly reported MissingPrevout,
        // dropping the tweak for the whole transaction.
        let status = compute_tx_scan_point(&[
            TxInputContext {
                previous_output: outpoint(4),
                script_sig: Vec::new(),
                witness: vec![vec![1; 64], GENERATOR_COMPRESSED.to_vec()],
                prevout: Some(PrevoutInfo {
                    script: PrevoutScript::P2wpkh {
                        hash160: hash160_bytes(&GENERATOR_COMPRESSED),
                    },
                }),
            },
            TxInputContext {
                previous_output: outpoint(5),
                script_sig: Vec::new(),
                witness: Vec::new(),
                prevout: Some(PrevoutInfo {
                    script: PrevoutScript::Other,
                }),
            },
        ])
        .unwrap();

        assert!(matches!(status, ScanPointStatus::Computed(_)));
    }
}
