@0xb17c0da7a4510001;

struct LightBlock {
  version @0 :UInt16;
  height @1 :UInt64;
  blockHash @2 :Data;
  previousBlockHash @3 :Data;
  blockAnchorLastUid @4 :UInt64;
  profile @5 :LightBlockProfile;
  outputIdBytes @6 :UInt8;
  tweakCount @7 :UInt32;
  # Elias-delta encoded sorted tx indexes: first index as first+1, later indexes as deltas.
  txTweakIndexes @8 :Data;
  # Concatenated 33-byte compressed public tweak keys.
  # Each element is input_hash*A for the tx at the corresponding txTweakIndexes entry.
  txTweaks @9 :Data;
  outputs @10 :List(OutputRef);
  outputIds @11 :Data;
  spentIdCodec @12 :SpentIdCodec;
  spentCount @13 :UInt32;
  spentIds @14 :Data;
}

struct OutputRef {
  txIndex @0 :UInt32;
  vout @1 :UInt32;
  uid @2 :UInt64;
}

# SnapshotBlock is a live-output snapshot block, not a normal block delta.
# A cut-through snapshot at height H contains only outputs that were created in
# this block and are still live at H. It has no spent stream; outputs omitted
# from the snapshot were spent by H or are outside the selected scope.
struct SnapshotBlock {
  version @0 :UInt16;
  height @1 :UInt64;
  blockHash @2 :Data;
  previousBlockHash @3 :Data;
  blockAnchorLastUid @4 :UInt64;
  outputIdBytes @5 :UInt8;
  txs @6 :List(SnapshotTx);
}

struct SnapshotTx {
  txIndex @0 :UInt32;
  # 33-byte compressed public scan point input_hash*A for this transaction.
  tweak @1 :Data;
  outputs @2 :List(SnapshotOutputRef);
  # Packed truncated output identifiers for outputs, using the parent block's
  # outputIdBytes. len = outputs.len * outputIdBytes.
  outputIds @3 :Data;
}

struct SnapshotOutputRef {
  vout @0 :UInt32;
  uid @1 :UInt64;
}

struct LightBlockProfile {
  scope @0 :ArchiveScope;
  # 0 means raw/no cut-through. Non-zero cut-through profiles are materialized
  # on fixed boundaries and have their own checkpoints and served tips.
  cutThroughBlocks @1 :UInt32;
}

enum ArchiveScope {
  # Silent Payments candidate scope: every P2TR output after Taproot activation receives a UID.
  # Reused P2TR keys are included. NUMS is an input-side BIP352 spend rule, not an output filter.
  p2trSp @0;
  # Every P2TR output receives a UID, including NUMS and reused keys.
  p2tr @1;
  # Every Bitcoin output receives a UID when the archive is initialized with this scope.
  allOutputs @2;
}

enum SpentIdCodec {
  eliasDeltaSorted @0;
}

struct UidCheckpoint {
  version @0 :UInt16;
  height @1 :UInt64;
  blockHash @2 :Data;
  lastUid @3 :UInt64;
  profile @4 :LightBlockProfile;
  uidCodec @5 :UidSetCodec;
  unspentCount @6 :UInt64;
  unspentUids @7 :Data;
}

enum UidSetCodec {
  chunkedAdaptive @0;
  eliasDeltaSorted @1;
  leb128Sorted @2;
}
