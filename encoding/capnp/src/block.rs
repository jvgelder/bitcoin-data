//! `bitcoin::Block` → capnp `Block` message.

use crate::block_capnp::{block, transaction, tx_in, tx_out};
use btc_data_core::parse::ParsedBlock;
use capnp::message::{Builder, HeapAllocator};

pub fn encode_block(parsed: &ParsedBlock) -> Builder<HeapAllocator> {
    let mut msg = Builder::new_default();
    {
        let mut b = msg.init_root::<block::Builder>();
        b.set_height(parsed.height);
        b.set_hash(&parsed.hash);
        fill_block(b, parsed);
    }
    msg
}

fn fill_block(mut b: block::Builder<'_>, parsed: &ParsedBlock) {
    let block = &parsed.block;
    let h = &block.header;
    b.set_version(h.version.to_consensus());
    b.set_prev_hash(h.prev_blockhash.as_ref());
    b.set_merkle_root(h.merkle_root.as_ref());
    b.set_time(h.time);
    b.set_bits(h.bits.to_consensus());
    b.set_nonce(h.nonce);

    let mut txs = b.init_transactions(block.txdata.len() as u32);
    for (i, tx) in block.txdata.iter().enumerate() {
        fill_tx(txs.reborrow().get(i as u32), tx);
    }
}

fn fill_tx(mut t: transaction::Builder<'_>, tx: &bitcoin::Transaction) {
    t.set_txid(tx.compute_txid().as_ref());
    t.set_wtxid(tx.compute_wtxid().as_ref());
    t.set_version(tx.version.0);
    t.set_locktime(tx.lock_time.to_consensus_u32());
    t.set_is_coinbase(tx.is_coinbase());

    let mut ins = t.reborrow().init_inputs(tx.input.len() as u32);
    for (j, vin) in tx.input.iter().enumerate() {
        fill_vin(ins.reborrow().get(j as u32), vin);
    }

    let mut outs = t.init_outputs(tx.output.len() as u32);
    for (j, vout) in tx.output.iter().enumerate() {
        fill_vout(outs.reborrow().get(j as u32), vout);
    }
}

fn fill_vin(mut ti: tx_in::Builder<'_>, vin: &bitcoin::TxIn) {
    ti.set_prev_txid(vin.previous_output.txid.as_ref());
    ti.set_prev_vout(vin.previous_output.vout);
    ti.set_script_sig(vin.script_sig.as_bytes());
    ti.set_sequence(vin.sequence.0);

    let stack: Vec<&[u8]> = vin.witness.iter().collect();
    let mut wit = ti.init_witness(stack.len() as u32);
    for (k, w) in stack.iter().enumerate() {
        wit.set(k as u32, w);
    }
}

fn fill_vout(mut to: tx_out::Builder<'_>, vout: &bitcoin::TxOut) {
    to.set_value(vout.value.to_sat());
    to.set_script_pubkey(vout.script_pubkey.as_bytes());
}
