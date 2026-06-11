# btc-data light-server

Prototype crate for a Silent Payments light-data archive.

The crate has two main binaries:

- `light-indexer`: reads Bitcoin blocks from `btc-data-sources`, builds the light archive, and writes SQLite/file-backed payloads.
- `light-server`: serves cached light blocks, ranges, manifests, cut-through streams, and debug statistics over HTTP.

The production path is SQLite-first. File-backed archives remain useful for fixtures and decoder tests.

## Current archive scope

The current working scope is **`p2tr-sp`**.

```text
UID starts at 1.
uid += 1 for every P2TR output in the selected scope.
non-P2TR outputs do not receive UIDs.
reused P2TR output keys are included and receive UIDs.
spent streams contain spends of scoped UIDs only.
checkpoints are not generated or served; deterministic replay from the profile start is the recovery model.
```

Archive scope is deterministic and fixed for a database/archive. Changing from `p2tr-sp` to `p2tr` or `all-outputs` requires a full rescan because the UID namespace changes.

## Start height and immutable metadata

Archive start height is deterministic for a new archive and is persisted in metadata together with the scope/network. For P2TR-scoped archives, the default start height is the network Taproot activation height.

```text
mainnet p2tr-sp / p2tr: 709632
testnet p2tr-sp / p2tr: 2011968
signet/regtest/fixture p2tr-sp / p2tr: 0 unless overridden
all-outputs: 0 unless overridden
```

If `network`, `scope`, or `start_height` in an existing DB differs from the configured values, the indexer must stop and require a new DB/full rescan.

During active development it is often simplest to delete the SQLite archive and restart:

```bash
rm -f lightdata-mainnet.db lightdata-mainnet.db-shm lightdata-mainnet.db-wal
```

## Scope meanings

```text
p2tr-sp:
  Silent Payments candidate output scope.
  Includes P2TR outputs, including reused keys.
  Does not exclude outputs based on NUMS.
  Excludes all non-P2TR outputs.

p2tr:
  All P2TR outputs, including reused keys.

all-outputs:
  Broad scope: every Bitcoin output receives a UID when the archive is initialized with this scope.
```

Profiles inside a scope only vary by cut-through window:

```text
profile = scope + cutthrough_blocks
```

There are no profile toggles for reuse filtering or NUMS filtering. This avoids servers drifting apart while using the same profile name.

## Current crate layout

- `src/bin/light_indexer.rs`: CLI for fixture generation, source checks, raw block fetching, and live SQLite indexing.
- `src/bin/light_server.rs`: HTTP server entrypoint.
- `src/p2tr_indexer.rs`: source-agnostic scoped UID state, `outpoint -> uid`, live UID mutation state, reuse counting, spend tracking, and conversion to `LightBlockInput`.
- `src/index.rs`: `LightBlock` encoders, Elias-delta spent UID encoding, and fixed tx tweak index encoding.
- `src/server.rs`: Axum routes for `/health`, `/manifest`, `/tip`, block range, single block, cut-through streams, and debug block stats.
- `src/storage/backend.rs`: storage interface used by HTTP routes.
- `src/storage/sqlite.rs`: SQLx/SQLite implementation of `ArchiveBackend`.
- `src/storage/files.rs`: file-backed implementation for fixtures/local tests.
- `src/storage/migrations/0001_init.sql`: canonical SQLite schema.
- `schema/light.capnp`: canonical binary wire schema.

Source access is provided by the workspace `btc-data-sources` crate. The light-server crate should not duplicate block-source implementations.

## Sources and indexer operation

The indexer uses `btc_data_sources` directly:

```text
btc_data_core::source::BlockSource
btc_data_core::source::TipWatcher
btc_data_sources::IpcSource
btc_data_sources::RestSource
btc_data_sources::PollingTipWatcher
```

IPC is the default block source. REST can also be used as the block source. Undo data for Silent Payment tweak computation is always fetched from Bitcoin Core REST `/rest/spenttxouts`.

Actual `light-indexer` source arguments:

```text
--source <ipc|rest>        block source, default: ipc
--ipc-socket <PATH>        required when --source ipc
--ipc-threads <N>          IPC worker threads, default: 8
--rest-url <URL>           required for --source rest, and also required for --source ipc undo data
--poll-interval-secs <N>   REST polling interval, default: 10
```

Use the REST base URL only, not an endpoint path:

```text
--rest-url http://127.0.0.1:8332
```

Bitcoin Core must have REST enabled:

```conf
rest=1
```

The intended run loop is:

```text
1. read archive metadata and indexed tip
2. ask source for current tip
3. finalized_tip = source_tip - finality_depth
4. index missing blocks next_height..=finalized_tip in ranges
5. only once caught up, wait for source tip notification
6. wake, recompute finalized tip, repeat
```

`light-indexer` no longer uses a RocksDB prevout store. Tweak data is derived from REST undo data.

When `--source ipc` is used, blocks and tip watching come from IPC, while undo data comes from `--rest-url`.
When `--source rest` is used, blocks, tip polling, and undo data all use the same REST source.

## Running the indexer

Check the IPC source:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- source-tip \
  --source ipc \
  --ipc-socket /var/lib/bitcoind/.bitcoin/node.sock \
  --rest-url http://127.0.0.1:8332
```

Fetch one block:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- fetch-block \
  --source ipc \
  --ipc-socket /var/lib/bitcoind/.bitcoin/node.sock \
  --rest-url http://127.0.0.1:8332 \
  --height 709632 \
  --output block-709632.bin
```

Run mainnet indexing with P2TR/SP emission from Taproot activation. Tweak data is derived from `/rest/spenttxouts`; no RocksDB prevout store is used:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- run \
  --source ipc \
  --ipc-socket /var/lib/bitcoind/.bitcoin/node.sock \
  --rest-url http://127.0.0.1:8332 \
  --database-url sqlite:lightdata-mainnet.db \
  --network mainnet \
  --scope p2tr-sp \
  --finality-depth 6
```

For a REST-only smoke test that performs one pass and exits:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- run \
  --source rest \
  --rest-url http://127.0.0.1:8332 \
  --database-url sqlite:lightdata-mainnet.db \
  --network mainnet \
  --scope p2tr-sp \
  --finality-depth 6 \
  --once
```

Check the REST source:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- source-tip \
  --source rest \
  --rest-url http://127.0.0.1:8332
```

## Fixture generation

Generate a file-backed fixture archive:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- fixture \
  --archive lightdata \
  --network fixture \
  --scope p2tr-sp \
  --start-height 800000 \
  --count 3
```

Generate a SQLite fixture archive and serve it:

```bash
cargo run -p btc-data-light-server --bin light-indexer -- fixture-db \
  --database-url sqlite:lightdata.db \
  --network fixture \
  --scope p2tr-sp \
  --start-height 800000 \
  --count 3

cargo run -p btc-data-light-server --bin light-server -- \
  --backend sqlite \
  --database-url sqlite:lightdata.db \
  --bind 127.0.0.1:3000
```

Serve a file-backed fixture archive:

```bash
cargo run -p btc-data-light-server --bin light-server -- \
  --backend files \
  --archive lightdata \
  --bind 127.0.0.1:3000
```

## Serving SQLite data

Run the HTTP server:

```bash
cargo run -p btc-data-light-server --bin light-server -- \
  --backend sqlite \
  --database-url sqlite:lightdata-mainnet.db \
  --bind 127.0.0.1:3000
```

For an empty DB, the server can initialize schema before serving:

```bash
cargo run -p btc-data-light-server --bin light-server -- \
  --backend sqlite \
  --database-url sqlite:lightdata.db \
  --migrate \
  --bind 127.0.0.1:3000
```

The production serving path reads exact cached wire bytes:

```text
payload_cache(profile_id, height)      -> serialized LightBlock bytes
```

## Client-facing API examples

```bash
curl http://127.0.0.1:3000/health

# Full finalized tip for a selected scope.
curl 'http://127.0.0.1:3000/tip?scope=p2tr-sp'

# Cut-through finalized tip for a selected scope.
curl 'http://127.0.0.1:3000/tip/cutthrough?scope=p2tr-sp'

# Single full light block.
curl -H 'Accept: application/octet-stream' \
  -o block.capnp \
  'http://127.0.0.1:3000/blocks/709632/light?scope=p2tr-sp'

# Single cut-through light block.
curl -H 'Accept: application/octet-stream' \
  -o block-ct.capnp \
  'http://127.0.0.1:3000/blocks/709632/light/cutthrough?scope=p2tr-sp'

# JSON debug view of one full light block.
curl -H 'Accept: application/json' \
  'http://127.0.0.1:3000/blocks/709632/light?scope=p2tr-sp'

# Full range sync.
curl -H 'Accept: application/octet-stream' \
  -o range.bdsr \
  'http://127.0.0.1:3000/blocks/light?start=709632&count=1000&scope=p2tr-sp'

# Cut-through historical backfill.
curl -H 'Accept: application/octet-stream' \
  -o range-ct.bdsr \
  'http://127.0.0.1:3000/blocks/light/cutthrough?start=709632&count=1000&scope=p2tr-sp'

# Per-block stats.
curl 'http://127.0.0.1:3000/debug/blocks/709632/stats'
```

The public API intentionally exposes `scope=` and dedicated full vs cut-through endpoints, but not `profile=`, `domain=`, or numeric `ct=` parameters. The selected `scope` defines the UID namespace. The server validates that the requested scope exists in the archive and returns a clear error when it does not.

`cutthrough=true` is not accepted on the full endpoints. Use the `/cutthrough` endpoints instead so clients cannot accidentally treat a reduced stream as a full stream.

## Sync target and cut-through

The public sync target should be finalized blocks only:

```text
finality_depth = 6
full served_tip = indexed_tip - 6
cut-through served_tip = floor((indexed_tip - 6 - ct) / 144) * 144
```

The SQLite-backed archive may materialize fixed cut-through profiles on 144-block boundaries, for example:

```text
raw-sp      ct=0
ct12-sp     ct=12
ct144-sp    ct=144
ct1008-sp   ct=1008
ct4320-sp   ct=4320
ct12960-sp  ct=12960
ct52560-sp  ct=52560
ct105120-sp ct=105120
```

For `/blocks/light/cutthrough`, the server chooses the largest materialized cut-through window that can serve the requested start height.

Cut-through requests must end at or below:

```text
full_tip - suggested_reorg_cache_depth
```

This keeps wallet clients from using a reduced stream for the recent reorg window. The default suggested reorg cache depth is 24 blocks.

## Wallet cold-boot flow

A v1 Silent Payments wallet client scans block payloads from its profile start. Checkpoints are intentionally removed for now; cut-through-from-genesis/profile-start is deterministic and avoids a large checkpoint materialization path in the indexer.

Recommended cold boot:

```text
1. GET /manifest
2. GET /tip?scope=p2tr-sp
3. recent_full_depth = manifest.suggested_reorg_cache_depth, default 24
4. stable_tip = tip.height - recent_full_depth
5. scan /blocks/light/cutthrough from wallet birthday/start height through stable_tip
6. scan /blocks/light from stable_tip + 1 through tip.height
7. live sync new blocks with /blocks/light only
```

The cut-through stream is a historical backfill optimization. It may omit outputs created and spent inside the cut-through window, so it is not a complete activity-history stream. The full stream is required near the tip for shallow reorg handling and short-lived recent wallet outputs.

## Wire payload semantics

`LightBlock` contains:

```text
version
height
blockHash
previousBlockHash
profile
blockAnchorLastUid
outputIdBytes
tweakCount
txTweakIndexes
txTweaks
outputs
outputIds
spentIdCodec
spentCount
spentIds
```

Tweak naming follows Blindbit/light-client terminology. `txTweaks` are **33-byte compressed public tweak keys**, not 32-byte scalar tweaks:

```text
server computes: input_hash * A
client computes: b_scan * tweak
```

Current implementation status:

```text
TxTweak type and wire length: 33 bytes
fake placeholder tweak emission: removed
BIP352 scan-point computation: implemented for P2TR, P2WPKH, P2SH-P2WPKH, and P2PKH inputs
```

If `/rest/spenttxouts` undo data is missing for a non-coinbase transaction that has Taproot outputs, live indexing fails rather than falling back to a local prevout database or serving guessed tweak data.

## BIP352 tweak computation requirements

A correct Silent Payments tweak requires transaction input eligibility and spent prevout context. The live indexer obtains that context from Bitcoin Core `/rest/spenttxouts`.

Eligible input types:

```text
P2TR
P2WPKH
P2SH-P2WPKH
P2PKH
```

Transactions should only emit tweak data if they have at least one Taproot output, at least one eligible input, and do not spend unsupported SegWit version > 1 outputs.

The indexer needs enough undo/prevout data to derive/sum eligible input public keys:

```text
P2TR:        spent prevout output key, except NUMS script-path inputs
P2WPKH:      compressed pubkey from witness
P2SH-P2WPKH: compressed pubkey from witness + redeem script validation
P2PKH:       compressed pubkey from scriptSig
```

The BIP352 NUMS exception is input-side only: Taproot script-path spends with the NUMS internal key are excluded from tweak derivation. They should not affect output UID assignment.

## Storage backend interface

The HTTP server is storage-agnostic:

```text
Axum routes
  -> Arc<dyn ArchiveBackend>
     -> SqliteArchive using SQLx
     -> FileArchive using files per block
```

Backend selection happens in `src/bin/light_server.rs`, not inside route handlers.

The trait boundary is:

```text
manifest()
resolve_profile()
tip()
read_block()
read_blocks()
read_cutthrough_delta_blocks()
read_cutthrough_snapshot()
block_stats()
```

Any future backend must implement that interface and return the same cached wire bytes for the same scope/profile/height.

## SQLite schema

The schema is stored in `src/storage/migrations/0001_init.sql` and applied through SQLx.

Core tables:

```text
meta
blocks
block_stats
block_exclusion_stats
profiles
p2tr_outputs
p2tr_spends
p2tr_key_stats
tx_tweaks
payload_cache
```

Important invariants:

```text
uid INTEGER PRIMARY KEY CHECK(uid > 0)
first UID = 1
network, scope, and start_height are stored in meta and immutable for the DB
p2tr-sp indexed_output_count = p2tr_output_count
p2tr_reused_count is counted but not subtracted from p2tr-sp
p2tr_nums_count is not an output-exclusion count; NUMS is input-side for BIP352 tweaks
```

During indexing, live UIDs are kept in memory for fast insert/remove. The indexer does not materialize UID checkpoints; committed SQL rows and deterministic replay define recovery state.

## Debug and validation commands

While indexing:

```bash
sqlite3 lightdata-mainnet.db "SELECT COUNT(*), MIN(height), MAX(height) FROM blocks;"
sqlite3 lightdata-mainnet.db "SELECT SUM(p2tr_created_count), SUM(p2tr_spent_count), MAX(anchor_last_uid) FROM blocks;"
sqlite3 lightdata-mainnet.db "SELECT profile_id,name,scope,cutthrough_blocks,served_tip_height FROM profiles;"
sqlite3 lightdata-mainnet.db "SELECT key,value FROM meta ORDER BY key;"
```

Expected live indexing log shape:

```text
source=http://127.0.0.1:8332 watcher=polling:http://127.0.0.1:8332 best_height=... finalized_tip=... next_height=709632 database=sqlite:lightdata-mainnet.db
fetched finalized range 709632..=709759 count=128 bytes=...
indexed finalized range 709632..=709759 last_uid=... live_uids=...
```

## Shared-code/refactor notes

There is overlap between light-server indexing code and existing stats code. The reusable pieces should move into a common crate instead of being duplicated:

```text
script classification
P2TR x-only output key extraction
Taproot spend-path classification
NUMS internal-key detection
block preparation / source batching patterns
```

Recommended future layout:

```text
btc-data-chain
  script.rs        ScriptType, classify_script, p2tr_xonly_output_key, P2A
  taproot.rs       SpendPath, SpendClass, classify_p2tr_spend, NUMS_H_XONLY
  block_prepare.rs compact decoded block representation
  outpoint.rs      shared outpoint/txid helpers

btc-data-sources
  BlockSource implementations and TipWatcher implementations

btc-data-stats
  depends on btc-data-chain + btc-data-sources

btc-data-light-server
  depends on btc-data-chain + btc-data-sources
```

## Strong domain types

The Rust API should not pass unrelated fixed-size values as raw arrays at module boundaries. The crate defines explicit byte-newtypes in `src/types.rs`:

```text
BlockHashBytes  [u8; 32]
TxidBytes       [u8; 32]
TxTweak         [u8; 33]
OutputIdHash    [u8; 32]
```

`TxTweak` is intentionally 33 bytes because it is a compressed public tweak key.
