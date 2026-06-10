//! Shared script classification used by light indexing and Silent Payments
//! input-eligibility plumbing.
//!
//! Prefer `rust-bitcoin`'s standard script classifiers for template detection.
//! This module only extracts the payload bytes the archive needs after the
//! library has confirmed the script shape.

use bitcoin::Script;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    P2pkh { hash160: [u8; 20] },
    P2sh { hash160: [u8; 20] },
    P2wpkh { hash160: [u8; 20] },
    P2wsh { sha256: [u8; 32] },
    P2tr { xonly_key: [u8; 32] },
    WitnessUnknown { version: u8, program_len: usize },
    OpReturn,
    Other,
}

pub fn classify_script(script_bytes: &[u8]) -> ScriptKind {
    let script = Script::from_bytes(script_bytes);

    if script.is_p2pkh() {
        let mut hash160 = [0u8; 20];
        hash160.copy_from_slice(&script_bytes[3..23]);
        return ScriptKind::P2pkh { hash160 };
    }

    if script.is_p2sh() {
        let mut hash160 = [0u8; 20];
        hash160.copy_from_slice(&script_bytes[2..22]);
        return ScriptKind::P2sh { hash160 };
    }

    if script.is_p2wpkh() {
        let mut hash160 = [0u8; 20];
        hash160.copy_from_slice(&script_bytes[2..22]);
        return ScriptKind::P2wpkh { hash160 };
    }

    if script.is_p2wsh() {
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&script_bytes[2..34]);
        return ScriptKind::P2wsh { sha256 };
    }

    if script.is_p2tr() {
        let mut xonly_key = [0u8; 32];
        xonly_key.copy_from_slice(&script_bytes[2..34]);
        return ScriptKind::P2tr { xonly_key };
    }

    if script.is_witness_program() {
        let version = match script_bytes[0] {
            0x00 => 0,
            op @ 0x51..=0x60 => op - 0x50,
            _ => return ScriptKind::Other,
        };
        let program_len = script_bytes.get(1).copied().unwrap_or_default() as usize;
        return ScriptKind::WitnessUnknown {
            version,
            program_len,
        };
    }

    if script.is_op_return() {
        return ScriptKind::OpReturn;
    }

    ScriptKind::Other
}

pub fn extract_p2tr_xonly(script: &[u8]) -> Option<[u8; 32]> {
    match classify_script(script) {
        ScriptKind::P2tr { xonly_key } => Some(xonly_key),
        _ => None,
    }
}

pub fn is_p2tr(script: &[u8]) -> bool {
    Script::from_bytes(script).is_p2tr()
}

pub fn is_op_return(script: &[u8]) -> bool {
    Script::from_bytes(script).is_op_return()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_p2tr_xonly_key_after_rust_bitcoin_classification() {
        let mut script = vec![0x51, 0x20];
        script.extend([7u8; 32]);

        assert_eq!(extract_p2tr_xonly(&script), Some([7u8; 32]));
        assert!(is_p2tr(&script));
        assert_eq!(
            classify_script(&script),
            ScriptKind::P2tr {
                xonly_key: [7u8; 32]
            }
        );
    }

    #[test]
    fn extracts_payloads_from_common_templates() {
        let mut p2pkh = vec![0x76, 0xa9, 0x14];
        p2pkh.extend([1u8; 20]);
        p2pkh.extend([0x88, 0xac]);
        match classify_script(&p2pkh) {
            ScriptKind::P2pkh { hash160 } => assert_eq!(hash160, [1u8; 20]),
            other => panic!("expected P2PKH, got {other:?}"),
        }

        let mut p2wpkh = vec![0x00, 0x14];
        p2wpkh.extend([2u8; 20]);
        match classify_script(&p2wpkh) {
            ScriptKind::P2wpkh { hash160 } => assert_eq!(hash160, [2u8; 20]),
            other => panic!("expected P2WPKH, got {other:?}"),
        }

        let mut p2wsh = vec![0x00, 0x20];
        p2wsh.extend([3u8; 32]);
        match classify_script(&p2wsh) {
            ScriptKind::P2wsh { sha256 } => assert_eq!(sha256, [3u8; 32]),
            other => panic!("expected P2WSH, got {other:?}"),
        }

        assert_eq!(classify_script(&[0x6a, 0x01, 0x00]), ScriptKind::OpReturn);
        assert!(is_op_return(&[0x6a, 0x01, 0x00]));
    }

    #[test]
    fn classifies_unknown_segwit_versions_after_known_templates() {
        let mut v2 = vec![0x52, 0x20];
        v2.extend([4u8; 32]);
        assert_eq!(
            classify_script(&v2),
            ScriptKind::WitnessUnknown {
                version: 2,
                program_len: 32
            }
        );
    }
}
