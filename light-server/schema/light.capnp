@0xb17c0da7a4510001;

# Client/server response format. This stays compact and does not expose
# server-side storage metadata such as reuse or spent height.
struct LightBlock {
  version @0 :UInt16;
  height @1 :UInt64;

  blockHash @2 :Data;          # 32 bytes
  previousBlockHash @3 :Data;  # 32 bytes

  firstUid @4 :UInt64;

  skippedTxsForTweaks @5 :List(UInt16);
  tweaks @6 :List(TweakEntry);

  outputs @7 :List(OutputEntry);

  spends @8 :List(SpendEntry);
}

struct TweakEntry {
  outputCount @0 :UInt16;
  tweak @1 :Data;              # 32 bytes
}

struct OutputEntry {
  key @0 :Data;                # 32 bytes
}

struct SpendEntry {
  spentUid @0 :UInt64;
}

# File archive storage format. The server reads this richer format and derives
# the compact LightBlock response from it.
struct StoredLightBlock {
  version @0 :UInt16;
  height @1 :UInt32;

  blockHash @2 :Data;          # 32 bytes
  previousBlockHash @3 :Data;  # 32 bytes

  firstUid @4 :UInt64;

  skippedTxsForTweaks @5 :List(UInt16);
  tweaks @6 :List(StoredTweakEntry);

  skippedOutputs @7 :List(UInt16);
  outputs @8 :List(StoredOutputEntry);

  spends @9 :List(StoredSpendEntry);
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
