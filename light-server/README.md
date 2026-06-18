# btc-data light-server

`btc-data-light-server` builds and serves Silent Payments light-data blocks.

The current implementation is intentionally small:

- `light-indexer` reads finalized Bitcoin blocks, derives Silent Payments scan points, assigns dense output UIDs, writes per-block Cap'n Proto payloads, and keeps only indexer working state in RocksDB.
- `light-server` serves the already encoded block payload files over HTTP.

There is no SQLite serving path, no profiles, no checkpoints, and no client-specific cut-through state in the current format.

## Storage model

The archive is split into two stores with different responsibilities.

```text
lightdata/                       # served archive
  manifest.json
  blocks/
    0000000000.capnp
    0000000001.capnp
    ...

light-indexer-rocksdb/           # indexer-only working state
  RocksDB files
```

### Served archive

Each file in `lightdata/blocks/` is a serialized Cap'n Proto `LightBlock` for exactly one block height. The server reads these files and returns the cached bytes directly.

The archive files are the public sync data. RocksDB is not served to clients.

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
  previous txid + vout -> created UID + creation height

seen key counts:
  P2TR x-only output key -> count
```

The outpoint lookup lets the indexer convert a later input prevout into a `spentUid` without keeping a full in-memory UTXO set.

The current internal RocksDB encoding is not part of the client wire protocol. It uses prefixed keys and big-endian numeric fields for stable RocksDB ordering:

```text
outpoint key:
  "o:" || txid[32] || vout[u32-be]

outpoint value:
  uid[u64-be] || creation_height[u64-be]
```

Older staged 8-byte and 17-byte outpoint values may be accepted for local migration compatibility, but new writes should use the 16-byte no-flags value.

## Commit ordering

The indexer should only advance the RocksDB tip after the corresponding archive block files are durable.

Preferred order for each commit window:

```text
1. Fetch block and undo/spent-prevout data.
2. Derive scan points, dense outputs, skipped outputs, and spends.
3. Encode the block as a Cap'n Proto LightBlock.
4. Write blocks/<height>.capnp.tmp.
5. fsync and rename to blocks/<height>.capnp.
6. Commit the RocksDB batch:
   - insert new outpoint -> uid entries
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

## Wire format

The served block payload is a Cap'n Proto `LightBlock`:

```capnp
struct LightBlock {
  version @0 :UInt16;
  height @1 :UInt64;

  blockHash @2 :Data;          # 32 bytes
  previousBlockHash @3 :Data;  # 32 bytes

  firstUid @4 :UInt64;

  skippedTxsForTweaks @5 :List(UInt16);
  tweaks @6 :List(TweakEntry);

  skippedOutputs @7 :List(UInt16);
  outputs @8 :List(OutputEntry);

  spends @9 :List(SpendEntry);
}

struct TweakEntry {
  outputCount @0 :UInt16;
  tweak @1 :Data;              # 32-byte x-coordinate of the scan point
}

struct OutputEntry {
  key @0 :Data;                # 32-byte P2TR x-only output key
}

struct SpendEntry {
  spentUid @0 :UInt64;
}
```

The indexer may represent a scan point internally as a 33-byte compressed public key. The served `TweakEntry.tweak` stores the 32-byte x-coordinate.

## UID semantics

UIDs are assigned only to the dense scannable output section of a block.

```text
uid = firstUid + dense_output_index
```

A dense output is a Taproot output whose transaction has a usable Silent Payments scan point. If a transaction has Taproot outputs but no usable scan point, those outputs are recorded in `skippedOutputs` and do not receive UIDs.

Required invariant:

```text
sum(tweaks.outputCount) == outputs.len()
```

`TweakEntry.outputCount` tells the client how many consecutive dense outputs use that scan point.

`SpendEntry.spentUid` identifies an output created in an earlier block. The spend height is the containing `LightBlock.height`, so no separate spent-height field is needed.

## Skipped data

`skippedTxsForTweaks` contains transaction indexes that have Taproot outputs but no corresponding tweak entry.

Common skip reasons:

```text
No eligible Silent Payments input keys
Unsupported witness version greater than 1
Missing prevout context
Eligible input public-key sum is infinity
```

Normal no-eligible-input cases are expected and should be logged at debug level. Missing prevouts and infinity sums are warnings because they may indicate source or data issues worth investigating.

`skippedOutputs` contains flattened block-output indexes for Taproot outputs omitted from the dense `outputs` list.

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
  --max-range-count 1000
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

The current API does not expose `scope`, `profile`, `cutthrough`, or checkpoint endpoints.

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
per-output flags
spent-height fields
reuse flags in served payloads
live unspent UID snapshots
client-specific cut-through state
payload_cache/checkpoint_cache tables
```

Reuse detection may still exist as an internal counter/debug aid, but it is not part of the served wire format and does not change UID assignment in the current model.

## Development invariants

These invariants should hold for every encoded block:

```text
blockHash.len() == 32
previousBlockHash.len() == 32
all OutputEntry.key values are 32 bytes
all TweakEntry.tweak values are 32 bytes
sum(tweaks.outputCount) == outputs.len()
skippedTxsForTweaks is sorted and unique
skippedOutputs is sorted and unique
```

Indexing invariants:

```text
RocksDB tip only advances after archive files exist
last_uid is monotonic
new outpoint lookups are written before later blocks can spend them
spent outpoints are removed from RocksDB during the commit that emits their spentUid
missing prevouts should be warnings, not silently treated as no eligible inputs
public-key sum infinity should be rare and investigated
```

## Crate layout

```text
src/bin/light_indexer.rs   indexer CLI, source loop, archive/RocksDB commit path
src/bin/light_server.rs    HTTP server entrypoint
src/p2tr_indexer.rs        dense UID assignment, spend resolution, block assembly
src/sp_tweak.rs            BIP352 scan-point calculation
src/index.rs               LightBlock validation and Cap'n Proto encoding
src/index_store.rs         RocksDB working state
src/storage/files.rs       file archive implementation
src/server.rs              Axum routes
src/types.rs               fixed-size byte newtypes
schema/light.capnp         canonical wire schema
```

Source access lives in the workspace `btc-data-sources` crate. This crate should not duplicate block source implementations.
