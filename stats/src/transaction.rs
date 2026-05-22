//! Per-transaction-input classification.
//!
//! Two concerns, classified independently per P2TR input:
//! 1. **Spend path** — key-path / script-path with NUMS / script-path with non-NUMS.
//!    Decides SP-eligibility.
//! 2. **Inscription envelope** — does the witness script contain an Ordinals
//!    `OP_FALSE OP_IF "ord" ... OP_ENDIF` envelope? Independent of NUMS.
//!    Most inscriptions in the wild use NUMS, but the protocol does not
//!    require it; non-NUMS inscriptions exist.
//!
//! [`classify_p2tr_spend`] returns both as `SpendClass { path, inscription }`.

use bitcoin::Witness;
use serde::{Deserialize, Serialize};

// ─── Spend path ────────────────────────────────────────────────────────

/// Classification of a P2TR spend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpendPath {
    /// Witness is a key-path spend.
    Key,
    /// Script-path with non-NUMS internal key → SP-eligible.
    ScriptNonNums,
    /// Script-path with NUMS internal key.
    ScriptNums,
}

/// BIP341 unspendable "NUMS" point H (x-only). Reference: BIP341 "NUMS
/// point" section. Reused as the internal key for ordinals inscription
/// reveals so the commitment is provably non-keypath.
pub const NUMS_H_XONLY: [u8; 32] = [
    0x50, 0x92, 0x9b, 0x74, 0xc1, 0xa0, 0x49, 0x54,
    0xb7, 0x8b, 0x4b, 0x60, 0x35, 0xe9, 0x7a, 0x5e,
    0x07, 0x8a, 0x5a, 0x0f, 0x28, 0xec, 0x96, 0xd5,
    0x47, 0xbf, 0xee, 0x9a, 0xce, 0x80, 0x3a, 0xc0,
];

/// Spend path + inscription envelope, both inferred from a P2TR witness stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendClass {
    pub path: SpendPath,
    /// True if the witness script contains an Ordinals envelope. Always
    /// false for key-path spends because there is no script to inspect.
    pub inscription: bool,
}

/// Classify a P2TR spend from its witness stack without copying witness data.
///
/// BIP341 witness rules:
/// - Key-path spend: stack contains only the key-path signature, optionally
///   followed by an annex.
/// - Script-path spend: after stripping annex, the last item is the control
///   block and the second-to-last item is the witness script.
/// - Control block: 1 byte `(leaf_version | parity)` + 32-byte internal key
///   + N×32 bytes Merkle path. Internal key is bytes `1..33`.
///
/// This function borrows directly from [`bitcoin::Witness`] and does not
/// allocate or clone the witness stack.
pub fn classify_p2tr_spend(witness: &Witness) -> SpendClass {
    let mut len = witness.len();

    if len == 0 {
        return SpendClass { path: SpendPath::Key, inscription: false };
    }

    // BIP341 annex: if present, it is the final witness element and starts
    // with 0x50. Strip it before classifying key-path vs script-path.
    if let Some(last) = witness_item(witness, len - 1) {
        if is_taproot_annex(last) {
            len -= 1;
        }
    }

    if len < 2 {
        return SpendClass { path: SpendPath::Key, inscription: false };
    }

    let control = match witness_item(witness, len - 1) {
        Some(control) => control,
        None => return SpendClass { path: SpendPath::Key, inscription: false },
    };

    if !is_taproot_control_block(control) {
        return SpendClass { path: SpendPath::Key, inscription: false };
    }

    let script = witness_item(witness, len - 2).unwrap_or_default();
    let inscription = is_inscription_envelope(script);

    let internal_key = &control[1..33];
    let path = if internal_key == NUMS_H_XONLY {
        SpendPath::ScriptNums
    } else {
        SpendPath::ScriptNonNums
    };

    SpendClass { path, inscription }
}

fn witness_item(witness: &Witness, index: usize) -> Option<&[u8]> {
    witness.iter().nth(index)
}

fn is_taproot_annex(item: &[u8]) -> bool {
    item.first().copied() == Some(0x50)
}

fn is_taproot_control_block(item: &[u8]) -> bool {
    // BIP341 control block length: 33 + 32m, where m is 0..=128.
    item.len() >= 33 && (item.len() - 33) % 32 == 0
}

// ─── Inscription envelope detection ────────────────────────────────────

/// Does this witness script contain an Ordinals inscription envelope?
///
/// The canonical envelope is:
/// ```text
/// OP_FALSE        (0x00)
/// OP_IF           (0x63)
/// OP_PUSHBYTES_3  (0x03) "ord"
/// ...inscription content...
/// OP_ENDIF        (0x68)
/// ```
///
/// We scan the script bytes for the prefix `[0x00, 0x63, 0x03, b'o', b'r', b'd']`
/// anywhere in the script. We do not require a matching `OP_ENDIF` since
/// malformed inscriptions still indicate inscription intent.
pub fn is_inscription_envelope(script: &[u8]) -> bool {
    const NEEDLE: &[u8] = &[0x00, 0x63, 0x03, b'o', b'r', b'd'];
    script.windows(NEEDLE.len()).any(|w| w == NEEDLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn witness_from_items(items: Vec<Vec<u8>>) -> Witness {
        let mut witness = Witness::new();
        for item in items {
            witness.push(item);
        }
        witness
    }

    fn key_witness() -> Witness {
        witness_from_items(vec![vec![0u8; 64]])
    }

    fn script_path_witness(internal_key: &[u8; 32], script: Vec<u8>) -> Witness {
        let mut control = vec![0xc0];
        control.extend_from_slice(internal_key);
        witness_from_items(vec![script, control])
    }

    #[test]
    fn key_path() {
        let c = classify_p2tr_spend(&key_witness());
        assert_eq!(c.path, SpendPath::Key);
        assert!(!c.inscription);
    }

    #[test]
    fn nums_inscription() {
        let mut script = vec![0x20];
        script.extend_from_slice(&[0u8; 32]);
        script.push(0xac);
        script.extend_from_slice(&[0x00, 0x63, 0x03, b'o', b'r', b'd']);
        script.push(0x68);

        let w = script_path_witness(&NUMS_H_XONLY, script);
        let c = classify_p2tr_spend(&w);
        assert_eq!(c.path, SpendPath::ScriptNums);
        assert!(c.inscription);
    }

    #[test]
    fn non_nums_inscription() {
        let mut script = vec![0x00, 0x63, 0x03, b'o', b'r', b'd', 0x68];
        script.insert(0, 0xac);
        let other_key = [0xaa; 32];
        let w = script_path_witness(&other_key, script);
        let c = classify_p2tr_spend(&w);
        assert_eq!(c.path, SpendPath::ScriptNonNums);
        assert!(c.inscription);
    }

    #[test]
    fn non_nums_no_inscription() {
        let other_key = [0xaa; 32];
        let w = script_path_witness(&other_key, vec![0xac]);
        let c = classify_p2tr_spend(&w);
        assert_eq!(c.path, SpendPath::ScriptNonNums);
        assert!(!c.inscription);
    }

    #[test]
    fn nums_without_envelope() {
        let w = script_path_witness(&NUMS_H_XONLY, vec![0xac]);
        let c = classify_p2tr_spend(&w);
        assert_eq!(c.path, SpendPath::ScriptNums);
        assert!(!c.inscription);
    }

    #[test]
    fn annex_stripped_for_key_path() {
        let w = witness_from_items(vec![vec![0u8; 64], vec![0x50, 0x01]]);
        let c = classify_p2tr_spend(&w);
        assert_eq!(c.path, SpendPath::Key);
        assert!(!c.inscription);
    }

    #[test]
    fn annex_stripped_for_script_path() {
        let other_key = [0xaa; 32];
        let mut control = vec![0xc0];
        control.extend_from_slice(&other_key);
        let w = witness_from_items(vec![vec![0xac], control, vec![0x50, 0x01]]);
        let c = classify_p2tr_spend(&w);
        assert_eq!(c.path, SpendPath::ScriptNonNums);
        assert!(!c.inscription);
    }
}
