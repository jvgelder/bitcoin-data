//! Output script-type classification.
//!
//! Counts outputs by script type per block. Uses rust-bitcoin's pattern
//! detection (`is_p2pk`, `is_p2pkh`, `is_p2sh`, `is_p2wpkh`, `is_p2wsh`,
//! `is_p2tr`, `is_op_return`) plus a hand-rolled Pay-to-Anchor detector.
//!
//! Inputs are classified by the *prevout's* script type, which means the
//! caller must look up the prevout (the scan loop already does this for
//! UTXO offset tracking) and pass the script_pubkey into [`classify_script`].
//! Without a prevout lookup, only the spend-witness gives weak hints.

use bitcoin::Script;
use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScriptType {
    P2pk,
    P2pkh,
    P2sh,
    P2wpkh,
    P2wsh,
    P2tr,
    /// Pay-to-Anchor — `OP_1 <2-byte push: 0x4e 0x73>` (the literal "Ns" /
    /// `0x4e73`). See BIP-431.
    P2a,
    OpReturn,
    /// Anything else. Includes nonstandard scripts and unrecognized
    /// future witness versions.
    Unknown,
}

impl ScriptType {
    pub const ALL: [ScriptType; 9] = [
        ScriptType::P2pk,
        ScriptType::P2pkh,
        ScriptType::P2sh,
        ScriptType::P2wpkh,
        ScriptType::P2wsh,
        ScriptType::P2tr,
        ScriptType::P2a,
        ScriptType::OpReturn,
        ScriptType::Unknown,
    ];

    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => ScriptType::P2pk,
            1 => ScriptType::P2pkh,
            2 => ScriptType::P2sh,
            3 => ScriptType::P2wpkh,
            4 => ScriptType::P2wsh,
            5 => ScriptType::P2tr,
            6 => ScriptType::P2a,
            7 => ScriptType::OpReturn,
            _ => ScriptType::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ScriptType::P2pk => "p2pk",
            ScriptType::P2pkh => "p2pkh",
            ScriptType::P2sh => "p2sh",
            ScriptType::P2wpkh => "p2wpkh",
            ScriptType::P2wsh => "p2wsh",
            ScriptType::P2tr => "p2tr",
            ScriptType::P2a => "p2a",
            ScriptType::OpReturn => "op_return",
            ScriptType::Unknown => "unknown",
        }
    }
}

/// Classify a `script_pubkey` into a [`ScriptType`].
pub fn classify_script(script: &Script) -> ScriptType {
    if is_p2a(script) { return ScriptType::P2a; }
    if script.is_p2tr() { return ScriptType::P2tr; }
    if script.is_p2wpkh() { return ScriptType::P2wpkh; }
    if script.is_p2wsh() { return ScriptType::P2wsh; }
    if script.is_p2sh() { return ScriptType::P2sh; }
    if script.is_p2pkh() { return ScriptType::P2pkh; }
    if script.is_p2pk() { return ScriptType::P2pk; }
    if script.is_op_return() { return ScriptType::OpReturn; }
    ScriptType::Unknown
}

/// Pay-to-Anchor: `OP_1 <push 0x4e73>`.
/// Bytes: `[0x51, 0x02, 0x4e, 0x73]`.
fn is_p2a(script: &Script) -> bool {
    let b = script.as_bytes();
    b.len() == 4 && b[0] == 0x51 && b[1] == 0x02 && b[2] == 0x4e && b[3] == 0x73
}

/// Per-block counters by script type. Both outputs (created in this block)
/// and inputs (prevouts spent in this block) are counted.
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct ScriptCounts {
    pub outputs: TypeCounts,
    pub inputs: TypeCounts,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct TypeCounts {
    pub p2pk: u64,
    pub p2pkh: u64,
    pub p2sh: u64,
    pub p2wpkh: u64,
    pub p2wsh: u64,
    pub p2tr: u64,
    pub p2a: u64,
    pub op_return: u64,
    pub unknown: u64,
}

impl TypeCounts {
    pub fn record(&mut self, t: ScriptType) {
        match t {
            ScriptType::P2pk => self.p2pk += 1,
            ScriptType::P2pkh => self.p2pkh += 1,
            ScriptType::P2sh => self.p2sh += 1,
            ScriptType::P2wpkh => self.p2wpkh += 1,
            ScriptType::P2wsh => self.p2wsh += 1,
            ScriptType::P2tr => self.p2tr += 1,
            ScriptType::P2a => self.p2a += 1,
            ScriptType::OpReturn => self.op_return += 1,
            ScriptType::Unknown => self.unknown += 1,
        }
    }

    pub fn merge(&mut self, other: &TypeCounts) {
        self.p2pk += other.p2pk;
        self.p2pkh += other.p2pkh;
        self.p2sh += other.p2sh;
        self.p2wpkh += other.p2wpkh;
        self.p2wsh += other.p2wsh;
        self.p2tr += other.p2tr;
        self.p2a += other.p2a;
        self.op_return += other.op_return;
        self.unknown += other.unknown;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::ScriptBuf;

    #[test]
    fn detects_p2tr() {
        // OP_1 OP_PUSHBYTES_32 <32 bytes>
        let mut bytes = vec![0x51, 0x20];
        bytes.extend_from_slice(&[0u8; 32]);
        let s = ScriptBuf::from(bytes);
        assert_eq!(classify_script(&s), ScriptType::P2tr);
    }

    #[test]
    fn detects_p2wpkh() {
        // OP_0 OP_PUSHBYTES_20 <20 bytes>
        let mut bytes = vec![0x00, 0x14];
        bytes.extend_from_slice(&[0u8; 20]);
        let s = ScriptBuf::from(bytes);
        assert_eq!(classify_script(&s), ScriptType::P2wpkh);
    }

    #[test]
    fn detects_p2a() {
        let s = ScriptBuf::from(vec![0x51, 0x02, 0x4e, 0x73]);
        assert_eq!(classify_script(&s), ScriptType::P2a);
    }

    #[test]
    fn detects_op_return() {
        let s = ScriptBuf::from(vec![0x6a, 0x04, 0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(classify_script(&s), ScriptType::OpReturn);
    }

    #[test]
    fn unknown_script() {
        let s = ScriptBuf::from(vec![0x55, 0x55, 0x55]);
        assert_eq!(classify_script(&s), ScriptType::Unknown);
    }
}