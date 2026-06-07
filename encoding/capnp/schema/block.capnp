@0x869aa9e0fb8273ec;
# Cap'n Proto schema for decoded Bitcoin block data.
#
# Owned by `btc-data-encoding-capnp`. Encoded from rust-bitcoin types
# (the in-process canonical form) at the wire boundary; consumed by
# format adapters (json/proto/avro) and capnp-native sinks (ipc).

# ─── Block ────────────────────────────────────────────────────────────

struct Block {
    height        @0 :UInt64;
    hash          @1 :Data;       # 32 bytes, internal byte order
    version       @2 :Int32;
    prevHash      @3 :Data;
    merkleRoot    @4 :Data;
    time          @5 :UInt32;
    bits          @6 :UInt32;
    nonce         @7 :UInt32;
    transactions  @8 :List(Transaction);
}

struct Transaction {
    txid     @0 :Data;            # 32 bytes
    wtxid    @1 :Data;
    version  @2 :Int32;
    locktime @3 :UInt32;
    inputs   @4 :List(TxIn);
    outputs  @5 :List(TxOut);
    isCoinbase @6 :Bool;
}

struct TxIn {
    prevTxid     @0 :Data;
    prevVout     @1 :UInt32;
    scriptSig    @2 :Data;
    sequence     @3 :UInt32;
    witness      @4 :List(Data);  # witness stack; empty for non-segwit
}

struct TxOut {
    value        @0 :UInt64;
    scriptPubkey @1 :Data;
}

# ─── Events ───────────────────────────────────────────────────────────

struct BitcoinEvent {
    union {
        rawBlock @0 :RawBlock;
        tip      @1 :Tip;
        hash     @2 :Hash;
    }
}

struct RawBlock {
    height @0 :UInt64;
    hash   @1 :Data;
    bytes  @2 :Data;
}

struct Tip {
    height @0 :UInt64;
    hash   @1 :Data;
}

struct Hash {
    hash @0 :Data;
}
