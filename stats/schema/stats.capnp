@0xc25cceaa7df522ce;
# Cap'n Proto schema for stats messages.

struct Stats {
    block @0 :BlockStats;
}

struct BlockStats {
    height                @0 :UInt64;
    lastGlobalId          @1 :UInt64;
    outputCount           @2 :UInt64;
    p2trOutputCount       @3 :UInt64;
    p2trReusedCount       @4 :UInt64;
    spends                @5 :UInt64;
    p2trSpends            @6 :UInt64;
    p2trKeypathSpends     @7 :UInt64;
    p2trScriptpathSpends  @8 :UInt64;
    p2trNumsSpends        @9 :UInt64;
    p2trSpEligibleSpends  @10 :UInt64;
    nonspTxs              @11 :UInt64;
    nonspTxOutputs        @12 :UInt64;
    utxoHash              @13 :Data;
    utxoHashWindow        @14 :UInt64;

    # Inscription detection (orthogonal to NUMS).
    p2trInscriptionSpends         @15 :UInt64;
    p2trNumsAndInscriptionSpends  @16 :UInt64;

    # Output script-type breakdown.
    scriptOutputs  @17 :ScriptCounts;
    # Input (prevout) script-type breakdown.
    scriptInputs   @18 :ScriptCounts;

    # Sorted per-block spent-UID encoding estimates.
    sortedSpentValues                 @19 :UInt64;
    sortedSpentLeb128Bytes            @20 :UInt64;
    sortedSpentRiceBestK              @21 :UInt32;
    sortedSpentRiceBestBits           @22 :UInt64;
    sortedSpentEliasDeltaBits         @23 :UInt64;
    sortedSpentEfBitsWith64BitBase    @24 :UInt64;

    sortedP2trSpentValues              @25 :UInt64;
    sortedP2trSpentLeb128Bytes         @26 :UInt64;
    sortedP2trSpentRiceBestK           @27 :UInt32;
    sortedP2trSpentRiceBestBits        @28 :UInt64;
    sortedP2trSpentEliasDeltaBits      @29 :UInt64;
    sortedP2trSpentEfBitsWith64BitBase @30 :UInt64;
}

struct ScriptCounts {
    p2pk     @0 :UInt64;
    p2pkh    @1 :UInt64;
    p2sh     @2 :UInt64;
    p2wpkh   @3 :UInt64;
    p2wsh    @4 :UInt64;
    p2tr     @5 :UInt64;
    p2a      @6 :UInt64;
    opReturn @7 :UInt64;
    unknown  @8 :UInt64;
}

struct Log2Hist {
    zeros    @0 :UInt64;
    buckets  @1 :List(UInt64);    # always length 64
    sum      @2 :UInt64;
    max      @3 :UInt64;
}
