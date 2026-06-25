# btc-data light-server

`btc-data-light-server` builds and serves Silent Payments light-data blocks.

The current implementation is intentionally small:

- `light-indexer` reads finalized Bitcoin blocks, fetches undo/spent-prevout data when needed, derives Silent Payments 
scan points, assigns global UIDs in the native-P2TR output domain, writes per-block Cap'n Proto storage payloads, and keeps only indexer working state in RocksDB.
- `light-server` reads the storage payloads, derives compact client response blocks, and serves them over HTTP.
- `light-archive-stats` walks stored Cap'n Proto blocks, derives response blocks, and appends per-block size/count statistics to a CSV file.

## Storage model

The archive is split into two stores with different responsibilities.

```text
lightdata/                       # file archive
  manifest.json
  blocks/
    0000000000.capnp
    0000000001.capnp
    ...

light-indexer-rocksdb/           # indexer-only working state
  RocksDB files
```

### File archive

Each file in `lightdata/blocks/` is a serialized Cap'n Proto `StoredLightBlock` for exactly one block height.

`StoredLightBlock` is the server-side storage format. It keeps full output keys and metadata needed to derive different response views later:

```text
full 32-byte P2TR output keys
storage-only skipped P2TR output slots for stats/debugging
spent height per stored output
reuse flags per stored output
spend creation height for cut-through filtering
```

The server does not return `StoredLightBlock` directly. It derives the compact response `LightBlock` at request time.

### RocksDB working state

RocksDB is used only by the indexer so it can resume and resolve future spends:

```text
metadata:
  network
  start_height
  tip_height
  tip_hash
  last_uid

outpoint lookup:
  previous txid + vout -> created UID + creation height + dense stored output index + flags

seen key counts:
  P2TR x-only output key -> count
```

The outpoint lookup lets the indexer convert a later input prevout into a `spentUid` without keeping a full in-memory UTXO set. 
The dense stored output index lets the indexer update `spentHeight` for the stored output when the output is spent.

The current internal RocksDB encoding is not part of the client wire protocol. It uses prefixed keys and big-endian numeric fields for stable RocksDB ordering:

```text
outpoint key:
  "o:" || txid[32] || vout[u32-be]

outpoint value:
  uid[u64-be] || creation_height[u64-be] || output_index[u32-be] || flags[u8]
```

## Commit ordering

The indexer should only advance the RocksDB tip after the corresponding archive block files are durable.

Preferred order for each commit window:

```text
1. Fetch block and undo/spent-prevout data.
2. Derive scan points, dense stored outputs, storage-only skipped outputs, and spends.
3. Encode the block as a Cap'n Proto StoredLightBlock.
4. Write blocks/<height>.capnp.tmp.
5. fsync and rename to blocks/<height>.capnp.
6. Commit the RocksDB batch:
   - insert new scannable outpoint -> uid/output_index entries
   - delete spent outpoints
   - update seen key counts
   - update last_uid
   - update tip height/hash
7. Rewrite manifest/tip metadata if needed.
```

Critical invariant:

```text
Never advance the RocksDB tip past a height whose .capnp block file is missing.
```

If a crash happens after writing a block file but before advancing the RocksDB tip, the indexer can reprocess and overwrite the same archive block file.

## Response wire format

The served block payload is a compact Cap'n Proto `LightBlock` derived from storage:

```capnp
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
  tweak @1 :Data;              # 32-byte x-coordinate of the scan point
}

struct OutputEntry {
  key @0 :Data;                # currently 32-byte P2TR x-only output key
}

struct SpendEntry {
  spentUid @0 :UInt64;
}
```

The response has a dense output array. It is a candidate-scanning format, not a full reconstruction of every native P2TR output slot.

`TweakEntry.outputCount` always maps to consecutive entries in the dense response `outputs` array after all static and dynamic filtering:

```text
tweak A outputCount = 3 -> outputs[0..3]
tweak B outputCount = 2 -> outputs[3..5]
tweak C outputCount = 5 -> outputs[5..10]
```

Required invariant:

```text
sum(tweaks.outputCount) == outputs.len()
```

The indexer may represent a scan point internally as a 33-byte compressed public key. The served `TweakEntry.tweak` stores the 32-byte x-coordinate.

## Storage format

The archive file payload is a richer Cap'n Proto `StoredLightBlock`:

```capnp
struct StoredLightBlock {
  version @0 :UInt16;
  height @1 :UInt32;

  blockHash @2 :Data;          # 32 bytes
  previousBlockHash @3 :Data;  # 32 bytes

  firstUid @4 :UInt64;

  skippedTxsForTweaks @5 :List(UInt16);
  tweaks @6 :List(StoredTweakEntry);

  skippedOutputs @7 :List(UInt16); # storage-only static omitted P2TR slots
  outputs @8 :List(StoredOutputEntry);

  spends @9 :List(StoredSpendEntry);
}

struct StoredTweakEntry {
  outputCount @0 :UInt16;
  tweak @1 :Data;              # 32 bytes
}

struct StoredOutputEntry {
  key @0 :Data;                # 32-byte P2TR x-only output key
  spentHeight @1 :UInt32;      # UInt32::MAX means unspent
  flags @2 :UInt8;             # bit 0 = reused
}

struct StoredSpendEntry {
  spentUid @0 :UInt64;
  creationHeight @1 :UInt32;
}
```

`StoredLightBlock.skippedOutputs` is storage-only diagnostic metadata. It records native-P2TR output slots omitted at index time, 
such as NUMS outputs or P2TR outputs from transactions without a usable Silent Payments tweak. 
It is useful for statistics and implementation checks, but clients do not receive it and do not need it for UID recovery.

Dynamic response filters do not mutate storage. When `cutthrough` or `filter_reuse` is requested, the server derives a 
response by omitting filtered stored outputs and recomputing every `TweakEntry.outputCount` so the response still maps cleanly to the dense output array.

## UID semantics

UIDs are assigned in the native-P2TR output domain:

```text
uid = firstUid + native_p2tr_output_index_in_block
```

Every native P2TR output consumes one UID slot, including outputs that are not served as scan candidates:

```text
normal scan candidates
NUMS P2TR outputs
P2TR outputs from transactions without a usable Silent Payments tweak
reuse-filtered outputs
cut-through-filtered outputs
```

Non-P2TR outputs do not consume UID slots.

The client does not need undo data or server skip lists to recover a UID after a candidate match. It fetches the full block, 
finds the matched outpoint, counts native P2TR outputs in block order up to that outpoint, and computes:

```text
uid = firstUid + count_native_p2tr_outputs_before_matched_outpoint
```

This keeps UID assignment independent of server-side eligibility decisions and dynamic response filters.

## Skipped data

`skippedTxsForTweaks` contains transaction indexes that have native P2TR outputs but no corresponding dense tweak entry.

Common skip reasons:

```text
No eligible Silent Payments input keys
Unsupported witness version greater than 1
Missing prevout context
Eligible input public-key sum is infinity
```

Normal no-eligible-input cases are expected and should be logged at debug level. Missing prevouts and infinity sums are 
warnings because they may indicate source or data issues worth investigating.

`StoredLightBlock.skippedOutputs` contains storage-only native-P2TR slot indexes for statically omitted outputs. It is not in the response and should not be used for client UID recovery.

## Client sync model

A client should treat `/blocks/light` as a paged sync stream. A range response can stop before the requested end because of `count`, server `max_range_count`, or the response byte cap.

Range responses include continuation headers:

```text
x-bitcoindata-range-start
x-bitcoindata-range-end
x-bitcoindata-range-count
x-bitcoindata-requested-range-end
x-bitcoindata-range-complete
x-bitcoindata-next-start        # present when range-complete is false
```

A new wallet can request cut-through from its birthday/start height:

```bash
curl -H 'Accept: application/octet-stream' \
  -o blocks.capnp \
  'http://127.0.0.1:3000/blocks/light?start=871932&count=1000&cutthrough=true'
```

If the response is incomplete, the client continues with `x-bitcoindata-next-start` and the same cut-through boundary:

```text
first request:
  /blocks/light?start=871932&count=1000&cutthrough_start=871932

if x-bitcoindata-range-complete: false
  next request:
  /blocks/light?start=<x-bitcoindata-next-start>&count=1000&cutthrough_start=871932
```

The client repeats until `x-bitcoindata-range-complete: true`. After that it can continue normal incremental sync from the next height above the served tip or the next archive tip it observes.

If a wallet starts after an interrupted sync, it should resume from the last fully processed block height plus one. 
For birthday cut-through, it should keep using the original `cutthrough_start` until the cut-through catch-up range is complete. 
This prevents a restart from changing which already-spent outputs were removed from earlier responses.

Cut-through and reuse filtering are opt-in. A plain range request returns the unfiltered dense response derived from storage:

```bash
curl -H 'Accept: application/octet-stream' \
  -o blocks.capnp \
  'http://127.0.0.1:3000/blocks/light?start=871932&count=1000'
```

## Client output scanning

For each response block:

```text
1. Keep firstUid, height, blockHash, previousBlockHash.
2. Walk tweaks in order.
3. For each tweak, take the next outputCount entries from the dense outputs array.
4. For each wallet scan key and each output in that range, derive the expected output key for that tweak.
5. Compare the derived key with the response output key.
6. Treat a match as a candidate and download the full Bitcoin block for confirmation.
```

Pseudocode:

```text
output_index = 0

for tweak in block.tweaks:
  range_start = output_index
  range_end = output_index + tweak.outputCount

  for output in block.outputs[range_start..range_end]:
    for scan_key in wallet.scan_keys:
      candidate_key = derive_silent_payment_output_key(scan_key, tweak.tweak)
      if candidate_key == output.key:
        mark_candidate(block.height, block.blockHash, tweak, output.key)

  output_index = range_end
```

On candidate match, the client downloads the full Bitcoin block and verifies:

```text
1. The downloaded block hash equals LightBlock.blockHash.
2. The candidate transaction output exists in the block.
3. The output script key equals the derived key.
4. The output belongs to a transaction whose Silent Payments tweak matches the response tweak.
5. The UID is computed by counting native P2TR outputs before the matched outpoint.
```

The final UID is:

```text
uid = LightBlock.firstUid + native_p2tr_index_of_matched_outpoint
```

The client should store the UID with its wallet note/UTXO state. Later spend notifications use this UID.

## Client spend scanning

`spends` is a dense list of global spent UIDs for outputs spent by the containing block.

For each response block:

```text
for spend in block.spends:
  if wallet owns spend.spentUid:
    mark wallet output as potentially spent at block.height
```

If the client wants confirmation beyond trusting the light server, it can download the full Bitcoin block and verify:

```text
1. The downloaded block hash equals LightBlock.blockHash.
2. A transaction input spends the previously known outpoint for that UID.
3. The spending transaction is included in the confirmed block.
```

The light response does not include the spent outpoint itself. The wallet learns and stores the outpoint when it first confirms the received output, 
so it can verify future spends from full blocks.

## Running the indexer

Check a REST source:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- source-tip \
  --source rest \
  --rest-url http://127.0.0.1:8332
```

Check an IPC source. `--rest-url` is still required because undo data is fetched through REST:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- source-tip \
  --source ipc \
  --ipc-socket /var/lib/bitcoind/.bitcoin/node.sock \
  --rest-url http://127.0.0.1:8332
```

Run mainnet indexing from the default network start height:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- run \
  --source rest \
  --rest-url http://127.0.0.1:8332 \
  --archive-dir lightdata-mainnet \
  --index-db-dir light-indexer-mainnet.rocksdb \
  --network mainnet \
  --finality-depth 6 \
  --catchup-batch-size 100 \
  --flush-blocks 100 \
  --memory-budget-mb 4000
```

Run with IPC block fetches and REST undo data:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- run \
  --source ipc \
  --ipc-socket /var/lib/bitcoind/.bitcoin/node.sock \
  --rest-url http://127.0.0.1:8332 \
  --archive-dir lightdata-mainnet \
  --index-db-dir light-indexer-mainnet.rocksdb \
  --network mainnet \
  --finality-depth 6 \
  --catchup-batch-size 100 \
  --flush-blocks 100 \
  --memory-budget-mb 4000
```

For a smoke test that catches up once and exits:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- run \
  --source rest \
  --rest-url http://127.0.0.1:8332 \
  --archive-dir lightdata-test \
  --index-db-dir light-indexer-test.rocksdb \
  --network mainnet \
  --finality-depth 6 \
  --catchup-batch-size 10 \
  --flush-blocks 10 \
  --once
```

## Running the server

Serve the file archive:

```bash
cargo run -p btc-data-light-server --bin light-server -- \
  --archive lightdata-mainnet \
  --bind 127.0.0.1:3000 \
  --max-range-count 1000 \
  --max-response-bytes 4194304
```

The server has no SQLite backend selection. It serves the file archive rooted at `--archive`.

## HTTP API

Health check:

```bash
curl http://127.0.0.1:3000/health
```

Manifest:

```bash
curl http://127.0.0.1:3000/manifest
```

Tip:

```bash
curl http://127.0.0.1:3000/tip
```

Single light block:

```bash
curl -H 'Accept: application/octet-stream' \
  -o block.capnp \
  http://127.0.0.1:3000/blocks/871932/light
```

Range of light blocks:

```bash
curl -H 'Accept: application/octet-stream' \
  -o blocks.capnp \
  'http://127.0.0.1:3000/blocks/light?start=871932&count=100'
```

Cut-through range:

```bash
curl -H 'Accept: application/octet-stream' \
  -o blocks.capnp \
  'http://127.0.0.1:3000/blocks/light?start=871932&count=1000&cutthrough_start=871932'
```

Optional query parameters for `/blocks/light`:

```text
start                 required first block height
count                 requested block count, capped by server max_range_count
cutthrough=true       use start as the cut-through boundary
cutthrough_start=H    explicit cut-through boundary; preferred for resumed sync
filter_reuse=true     omit outputs marked reused in storage
max_bytes=N           per-request response byte cap, capped by server max_response_bytes
```

The current API does not expose `scope`, `profile`, or checkpoint endpoints.

## Archive statistics

Append per-block stored-vs-response statistics to a CSV:

```bash
cargo run -p btc-data-light-server --bin light-archive-stats -- \
  --archive-dir lightdata-mainnet \
  --csv responsestats.csv \
  --start 871932
```

For the same derived cut-through response the server would send:

```bash
cargo run -p btc-data-light-server --bin light-archive-stats -- \
  --archive-dir lightdata-mainnet \
  --csv responsestats-cutthrough.csv \
  --start 871932 \
  --cutthrough-start 871932 \
  --filter-reuse
```

The stats output is useful for checking stored/response byte deltas, stored skipped outputs, dense response output counts, spend-list bytes, and Elias-delta spend-ID estimates.

## Fixture generation

Generate a deterministic file-backed fixture archive:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- fixture \
  --archive lightdata-fixture \
  --network fixture \
  --start-height 800000 \
  --count 3
```

Serve it:

```bash
cargo run -p btc-data-light-server --bin light-server -- \
  --archive lightdata-fixture \
  --bind 127.0.0.1:3000
```

## Removed concepts

The current format intentionally removed the previous heavier archive model:

```text
checkpoints
profiles
SQLite
live unspent UID snapshots
client-specific persistent cut-through state
payload_cache/checkpoint_cache tables
```

Reuse detection and spent-height tracking still exist as storage metadata, but they are not part of the served wire format except through dynamically filtered responses.

## Development invariants

These invariants should hold for every encoded response block:

```text
blockHash.len() == 32
previousBlockHash.len() == 32
all OutputEntry.key values are 32 bytes
all TweakEntry.tweak values are 32 bytes
sum(tweaks.outputCount) == outputs.len()
skippedTxsForTweaks is sorted and unique
```

These invariants should hold for every stored block:

```text
blockHash.len() == 32
previousBlockHash.len() == 32
all StoredOutputEntry.key values are 32 bytes
all StoredTweakEntry.tweak values are 32 bytes
sum(tweaks.outputCount) == outputs.len()
skippedTxsForTweaks is sorted and unique
skippedOutputs is sorted and unique
stored P2TR output domain size = outputs.len() + skippedOutputs.len()
```

Indexing invariants:

```text
RocksDB tip only advances after archive files exist
last_uid is monotonic
last_uid advances by every native P2TR output, not just stored outputs
new outpoint lookups are written before later blocks can spend them
spent outpoints are removed from RocksDB during the commit that emits their spentUid
missing prevouts should be warnings, not silently treated as no eligible inputs
public-key sum infinity should be rare and investigated
```

## Crate layout

```text
src/bin/light_indexer.rs       indexer CLI, source loop, archive/RocksDB commit path
src/bin/light_server.rs        HTTP server entrypoint
src/bin/light_archive_stats.rs archive statistics CLI
src/p2tr_indexer.rs            P2TR UID assignment, spend resolution, block assembly
src/sp_tweak.rs                BIP352 scan-point calculation
src/index.rs                   LightBlock/StoredLightBlock validation and Cap'n Proto encoding
src/index_store.rs             RocksDB working state
src/range.rs                   framed multi-block response format
src/storage/files.rs           file archive implementation
src/server.rs                  Axum routes
src/types.rs                   fixed-size byte newtypes
schema/light.capnp             canonical response and storage schema
```

Source access lives in the workspace `btc-data-sources` crate. This crate should not duplicate block source implementations.
