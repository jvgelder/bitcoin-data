//! Rolling commutative hash over recent blocks' UTXO deltas.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;

/// Rolling commutative hash over recent blocks' UTXO deltas.
///
/// Uses XOR of SHA-256 of each (added | removed) event, which is
/// order-independent and supports windowed "add the new block, evict
/// the block that falls off the window" updates in O(1) per event.
///
/// This is suitable for *oracle-agreement* checks (multiple servers should
/// produce the same accumulator), **not** for cryptographic commitment.
/// XOR is malleable; swap for MuHash if real soundness is needed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RollingUtxoHash {
    window: VecDeque<[u8; 32]>,
    acc: [u8; 32],
    window_blocks: u64,
    current_block: [u8; 32],
}

impl RollingUtxoHash {
    pub fn new(window_blocks: u64) -> Self {
        Self {
            window: VecDeque::new(),
            acc: [0u8; 32],
            window_blocks,
            current_block: [0u8; 32],
        }
    }

    pub fn enabled(&self) -> bool {
        self.window_blocks > 0
    }

    pub fn add_output(&mut self, global_id: u64) {
        if !self.enabled() {
            return;
        }
        let mut h = Sha256::new();
        h.update(b"add");
        h.update(global_id.to_be_bytes());
        let d = h.finalize();
        xor_into(&mut self.current_block, &d);
    }

    pub fn remove_output(&mut self, global_id: u64) {
        if !self.enabled() {
            return;
        }
        let mut h = Sha256::new();
        h.update(b"rem");
        h.update(global_id.to_be_bytes());
        let d = h.finalize();
        xor_into(&mut self.current_block, &d);
    }

    /// Finalize the current block and update the accumulator.
    /// Returns the current accumulator hex-encoded.
    pub fn finalize_block(&mut self) -> String {
        if !self.enabled() {
            return String::new();
        }

        xor_into(&mut self.acc, &self.current_block);
        self.window.push_back(self.current_block);
        self.current_block = [0u8; 32];

        while self.window.len() as u64 > self.window_blocks {
            let old = self.window.pop_front().unwrap();
            xor_into(&mut self.acc, &old);
        }

        hex_encode(&self.acc)
    }
}

fn xor_into(dst: &mut [u8; 32], src: &[u8]) {
    for i in 0..32 {
        dst[i] ^= src[i];
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
