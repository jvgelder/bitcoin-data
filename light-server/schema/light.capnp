@0xb17c0da7a4510001;

# Client/server response format. This stays compact and does not expose
# server-side storage metadata such as reuse, spent height, or full output keys.
struct LightBlock {
  version @0 :UInt16;
  height @1 :UInt64;

  blockHash @2 :Data;          # 32 bytes
  previousBlockHash @3 :Data;  # 32 bytes

  firstUid @4 :UInt64;

  # Number of original transactions covered by skippedTxsForTweaks.
  txCount @14 :UInt16;
  # One bit per original transaction, packed LSB-first.
  # 1 = no tweak entry is present for that tx.
  # 0 = consume the next flat tweak entry.
  skippedTxsForTweaks @5 :Data;

  # Flat tweak stream. The first tweakCount UInt16 values are packed little-endian
  # in tweakOutputCounts, and txTweaks contains tweakCount consecutive 32-byte
  # x-coordinate tweak payloads.
  tweakCount @6 :UInt32;
  tweakOutputCounts @12 :Data;  # tweakCount * 2 bytes, little-endian UInt16
  txTweaks @13 :Data;           # tweakCount * 32 bytes

  # Number of bits in each packed truncated output hash. The server chooses this
  # from the stored raw block size and the requested label budget.
  truncatedOutputHashBits @7 :UInt8;
  # Packed truncated output hashes in dense output order.
  # len = ceil(sum(tweakOutputCounts) * truncatedOutputHashBits / 8)
  truncatedOutputHashes @8 :Data;

  spentIdCodec @9 :SpentIdCodec;
  spentCount @10 :UInt32;
  spentIds @11 :Data;      # Elias-delta encoded spent UIDs.
}

enum SpentIdCodec {
  eliasDeltaAscendingAbsolute @0;
}

# File archive storage format. The server reads this richer format and derives
# the compact LightBlock response from it.
struct StoredLightBlock {
  version @0 :UInt16;
  height @1 :UInt32;

  blockHash @2 :Data;          # 32 bytes
  previousBlockHash @3 :Data;  # 32 bytes

  firstUid @4 :UInt64;

  # Number of original transactions covered by skippedTxsForTweaks.
  txCount @13 :UInt16;
  # One bit per original transaction, packed LSB-first.
  # 1 = no tweak entry is present for that tx.
  # 0 = consume the next stored tweak entry.
  skippedTxsForTweaks @5 :Data;
  tweaks @6 :List(StoredTweakEntry);

  skippedOutputs @7 :List(UInt16);
  outputs @8 :List(StoredOutputEntry);

  spends @9 :List(StoredSpendEntry);

  # Raw serialized Bitcoin block size, excluding undo/spenttxouts data.
  rawBlockBytes @10 :UInt32;

  # Storage-side precomputed response truncated output hashes for the two public label
  # budgets. Full 32-byte keys remain in outputs so this can be regenerated.
  truncatedOutputHashForTwoLabels @11 :Data;
  truncatedOutputHashForHundredLabels @12 :Data;
}

struct StoredTweakEntry {
  outputCount @0 :UInt16;
  tweak @1 :Data;              # 32 bytes
}
struct StoredOutputEntry {
  key @0 :Data;                # 32 bytes
  spentHeight @1 :UInt32;      # UInt32::MAX means unspent
  flags @2 :UInt8;             # bit 0 = reused
}

struct StoredSpendEntry {
  spentUid @0 :UInt64;
  creationHeight @1 :UInt32;
}
