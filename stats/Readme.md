# Bitcoin Stats

Bitcoin Stats is the companion scanner for `bitcoin-data`. It consumes
Bitcoin blocks through the same source abstractions as the main library
and emits derived per-block statistics.

It is kept in this repository for now because it shares source handling,
block parsing, and development workflow with the rest of `bitcoin-data`.
The public boundary is still separate: `bitcoin-data` focuses on block
fetching, decoding, encoding, and delivery; Bitcoin Stats focuses on
statistics and metric exports.

## What it computes

Bitcoin Stats scans blocks and produces summary data useful for research,
dashboards, indexing, and monitoring.

Current metrics include:

- total outputs and last global output ID
- spend counts
- sorted per-block spent-UID encoding estimates
- sorted per-block spent-UID encoding estimates
- P2TR output and spend counts
- P2TR key-path and script-path spend counts
- NUMS-script detection
- inscription-envelope detection
- non-SP-eligible transaction counts
- output script-type breakdowns
- spent-prevout script-type breakdowns
- optional rolling UTXO accumulator hash

## CLI

The binary is:

```bash
btc-data-stats
```

Example using an Esplora-compatible source:

```bash
btc-data-stats --esplora https://mempool.space/api \
  --start 870000 \
  --blocks 1000 \
  --out-per-block stats.csv
```

Example using Bitcoin Core JSON-RPC with automatic cookie authentication:

```bash
btc-data-stats --rpc-url http://127.0.0.1:8332 \
  --start 870000 \
  --blocks 1000 \
  --out-per-block stats.csv
```

If `--rpc-user` and `--rpc-pass` are omitted, the RPC source tries to read
Bitcoin Core's `.cookie` file from the default datadir. For non-default
locations, pass the cookie explicitly:

```bash
btc-data-stats --rpc-url http://127.0.0.1:8332 \
  --rpc-cookie-file /path/to/.cookie \
  --start 870000 \
  --blocks 1000 \
  --out-per-block stats.csv
```

Explicit username/password authentication is still supported:

```bash
btc-data-stats --rpc-url http://127.0.0.1:8332 \
  --rpc-user "$BTC_RPC_USER" \
  --rpc-pass "$BTC_RPC_PASS" \
  --start 870000 \
  --blocks 1000 \
  --out-per-block stats.csv
```

Multiple `--rpc-url` and `--esplora` arguments may be supplied. When more
than one source is configured, the shared `MultiSource` implementation
round-robins across them and falls through on per-source errors.

## Sinks

Bitcoin Stats has its own sink system. These sinks export derived
statistics, not full block data.

Stats sinks live under `stats/src/sinks/` and implement the `StatsSink`
trait.

### CSV

Status: implemented.

The CSV sink writes one row per block. It is useful for:

- local batch scans
- quick inspection
- research workflows
- spreadsheet imports
- loading per-block data into other tools

Use `--out-per-block` to write the per-block CSV file:

```bash
btc-data-stats --esplora https://mempool.space/api \
  --start 870000 \
  --blocks 1000 \
  --out-per-block stats.csv
```

### InfluxDB

Status: planned.

The InfluxDB sink is intended for time-series storage of per-block
statistics. It is useful for:

- long-running scans
- Grafana dashboards
- historical metric queries
- monitoring metric trends by block height

### Prometheus

Status: planned.

The Prometheus sink is intended for live operational metrics. It is useful
for:

- node and scanner observability
- alerting
- Grafana dashboards
- tracking current scan progress and recent block metrics

## Relationship to `bitcoin-data`

Bitcoin Stats is not a general block-delivery sink. It is a consumer of
the same block-source layer used by `bitcoin-data`, and it has its own
stats-specific sinks.

That means:

- top-level `sinks/` are for delivering block data or encoded block streams
- `stats/src/sinks/` are for exporting derived statistics

This keeps the main README focused on `bitcoin-data` while allowing the
stats scanner to document its own metrics, CLI, and output targets here.

## Stats classes

Bitcoin Stats separates metrics into two classes internally.

### Block-local stats

Block-local stats can be computed from the current block alone and are
safe to store idempotently using:

```text
(height, block_hash, stats_version)
```

Examples include output counts, P2TR output counts, output script-type
breakdowns, transaction counts, and inscription-envelope observations that
only depend on witness data in the block being scanned.

### Chain-derived stats

Chain-derived stats require prior chain state. These are still emitted per
block, but computing them depends on scanner state such as the UTXO map,
previously seen P2TR keys, global output IDs, and the rolling UTXO
accumulator.

Examples include spent-prevout script-type breakdowns, sorted spent-UID codec
spend counts, P2TR key reuse, spend-offset histograms, and UTXO-derived
accumulator values.

The CSV output remains a flat per-block row for compatibility, but the Rust
model now exposes the distinction through `BlockLocalStats` and
`ChainDerivedStats`.


## Optional IPC source

The shared source layer contains an experimental, feature-gated `IpcSource` for
Bitcoin Core multiprocess IPC. It uses the generated Chain bindings from the
`bitcoin-capnp-types` PR #22 branch, which corresponds to Bitcoin Core Chain
IPC testing work. This is intended for performance experiments, not as a stable
production interface yet.

Build with:

```bash
cargo build -p btc-data-stats --features ipc --release
```

Then configure an IPC socket from a multiprocess-enabled Bitcoin Core node:

```bash
btc-data-stats --ipc /path/to/node.sock --ipc-threads 8 ...
```

The IPC source maps the shared `BlockSource` calls to Chain IPC methods:

- `get_best_height()` → `Chain.getHeight`
- `get_block_hash(height)` → `Chain.getBlockHash`
- `get_block_raw(hash)` → `Chain.findBlock` with `wantData = true`
- `get_block_by_height(height)` → `Chain.findFirstBlockWithTimeAndHeight` with `wantHash = true` and `wantData = true`

For throughput testing, IPC creates multiple Bitcoin Core IPC `Thread` clients
with `--ipc-threads` / `BTC_DATA_IPC_THREADS`. These are round-robined per
request so calls do not all share one server-side libmultiprocess worker.

Without the `ipc` feature, passing `--ipc` returns a clear error.

## Recovery and finality depth

Checkpoints are stored as internal `postcard` binary snapshots. They are
intended for fast local recovery, not as a public interchange format.

Bitcoin Stats supports restart recovery with checkpoint snapshots. A
checkpoint is written only after a block has been fully processed and every
configured sink has committed that block.

A checkpoint at height `H` means:

- scanner state includes block `H`;
- sink output is committed through block `H`;
- resume starts at `H + 1`.

A checkpoint stores both the block height and block hash, plus the scanner
state needed to continue chain-derived stats without replaying from the
beginning of the scan. Height alone is not enough because a different block
can occupy the same height after a reorg.

Enable recovery with:

```bash
btc-data-stats --resume \
  --checkpoint-dir .btc-data-stats/checkpoints \
  --checkpoint-every 1 \
  --checkpoint-keep 10
```

`--checkpoint-every` is counted in committed blocks, not requested blocks.
For example, `--checkpoint-every 100` writes after every 100 successfully
processed and sink-committed blocks. `--checkpoint-every 1` writes after
every committed block. `--checkpoint-every 0` disables checkpoint writes.

Full checkpoint snapshots are built only when a checkpoint is actually due.
The checkpoint writer keeps counters only; it does not retain a copy of the
scanner state between writes. This avoids holding an extra UTXO map in memory
while scanning.

At the end of a scan, Bitcoin Stats writes one final checkpoint if there are
committed blocks since the previous checkpoint.

Checkpoint files are named with both height and hash, for example:

```text
checkpoint-000870000-00000000000000000003....postcard
```

On startup with `--resume`, Bitcoin Stats:

1. loads checkpoints newest first;
2. verifies `checkpoint.height` still maps to `checkpoint.block_hash` on the
   configured source;
3. skips checkpoints with stale stats versions or incompatible scanner
   configuration;
4. restores the newest valid checkpoint;
5. asks each sink to roll back to the restored height;
6. resumes from `checkpoint.next_height`.

Bitcoin Stats also intentionally lags the tip by a configurable finality
depth:

```bash
btc-data-stats --finality-depth 6
```

With `--finality-depth 6`, the scanner only processes blocks that are at
least six blocks behind the current best tip. This avoids most reorgs for
long-running analytical scans while keeping the lower-level reorg model
available for future push-based services.

For historical batch scans where the requested range is already far behind
the current tip, `--finality-depth 0` is acceptable.


## Timing metrics

Bitcoin Stats prints coarse timing metrics during scans so source transport can
be compared with parsing, stats processing, sink writes, and checkpointing.
Progress lines include cumulative timings for:

- `fetch+decode`: time spent waiting for decoded block frames from the shared core pipeline;
- `process`: stats and UTXO-state update time;
- `sink`: output sink write/flush time;
- `checkpoint`: checkpoint construction, serialization, write, and pruning time.

This is intended to show whether JSON-RPC/Esplora transport is actually the
bottleneck before investing in faster local sources such as Bitcoin Core IPC.

The fetch pipeline has two tuning knobs:

```bash
--buffer 64
--source-batch-size 128
```

`--buffer` controls how many source requests or source batches are in flight.
`--source-batch-size` controls how many contiguous heights are requested per
source-level batch. JSON-RPC sources use this to batch `getblockhash` calls
and then batch `getblock <hash> 0` calls. REST and IPC normally should keep
`--source-batch-size 1`, because REST already returns raw `.bin` payloads and
IPC can fetch a height/hash/data frame in one Chain request.

## Processing map implementation

The chain-derived scanner keeps a large in-memory UTXO map and exact P2TR
key-reuse set. These are internal scan-state structures, so the stats crate uses
`hashbrown` with `ahash` for those hot-path maps instead of the standard
library hasher. This keeps average lookup/insert/remove behavior at `O(1)` but
reduces the constant cost of hashing millions of `OutPoint` keys during long
scans.

## Durable stores and sync targets

CSV is useful for batch scans and quick inspection. SQLite and DuckDB are
better fits for idempotent local storage and downstream synchronization.

The intended durable-store key is:

```text
(height, block_hash, stats_version)
```

This allows a store to:

- skip rows it already has for the same exact block and stats version;
- keep alternate rows if a reorg produced a different block at the same height;
- remove or ignore rows above a restored checkpoint;
- invalidate old rows when `STATS_STATE_VERSION` changes.

Planned targets:

### SQLite

SQLite is the preferred local durable store for resumable scans and simple
applications. It should use `INSERT OR REPLACE` / upsert semantics keyed by
`(height, block_hash, stats_version)`.

### DuckDB

DuckDB is the preferred analytics sync/export target. It is useful for large
local scans, ad hoc SQL analysis, and exporting per-block stats into Parquet
or other warehouse-friendly formats.

### Bitcoin Core REST source

For local Bitcoin Core nodes, `--rest-url` can be faster than JSON-RPC for block payloads because `/rest/block/<hash>.bin` returns raw consensus bytes directly instead of a JSON string containing hex. Start Bitcoin Core with REST enabled:

```bash
bitcoind -rest=1
```

Then run:

```bash
btc-data-stats --rest-url http://127.0.0.1:8332 --start 870000 --blocks 1000
```

The REST source still uses the same `BlockSource` abstraction and can be combined with other sources. For REST comparisons, start with `--source-batch-size 1` and tune `--buffer`.

### JSON-RPC batch tuning

JSON-RPC is slower than local REST for raw block scans because `getblock <hash> 0` returns a JSON string containing hex-encoded block data. For users who cannot enable `-rest=1`, the RPC source supports JSON-RPC batch requests:

```bash
btc-data-stats \
  --rpc-url http://127.0.0.1:8332 \
  --start 870000 \
  --blocks 1000 \
  --buffer 4 \
  --source-batch-size 128
```

This sends batched `getblockhash` calls followed by batched `getblock` calls.
Large values reduce HTTP round trips but can create large JSON responses, so
benchmark values such as 32, 64, 128, and 256.

### IPC concurrency note

When built with `--features ipc`, the IPC source fetches blocks by height
through the Chain interface so a single request can return the block hash and
raw block bytes together. The scan pipeline keeps up to `--buffer` height-based
fetches in flight and still delivers results in height order before updating
UTXO-derived stats.

The IPC source also creates a pool of Bitcoin Core IPC `Thread` clients. Each
thread maps to an independent server-side worker in libmultiprocess, so use
`--ipc-threads` to keep block-data requests parallel on the Bitcoin Core side:

```bash
--ipc /path/to/node.sock --buffer 64 --ipc-threads 8
```

The default is 8 and can also be set with `BTC_DATA_IPC_THREADS`.


## Fetch strategies

`btc-data-stats` chooses a source-specific fetch strategy:

- **REST** uses a hash-prefetch phase followed by concurrent raw `.bin` block downloads. This avoids grouping full block downloads into artificial batches.
- **RPC** may use `--source-batch-size` for true JSON-RPC batch requests (`getblockhash` and `getblock <hash> 0`).
- **IPC** uses direct height-based Chain requests and ignores `--source-batch-size` unless a real batch IPC method is added later. Tune IPC with `--buffer` and `--ipc-threads` instead.

Recommended starting points:

```bash
# REST
--buffer 64 --source-batch-size 1

# RPC
--buffer 4 --source-batch-size 128

# IPC
--buffer 64 --source-batch-size 1 --ipc-threads 8
```


## Block payload memory

The shared core pipeline stores raw block payloads as `bytes::Bytes`. This lets HTTP sources pass through response bodies without copying and gives future IPC work a place to attach owner-backed zero-copy block data.

### Process profiling and map reserves

The scanner reports both cumulative and per-progress-interval timings.
The `process delta` line breaks down the sequential chain-state hot path:

```text
input_remove    UTXO lookup/removal for spent prevouts
p2tr_spend      Taproot spend classification and P2TR spend counters
output_classify output script classification and Taproot key extraction
seen_keys       exact P2TR key reuse set updates
utxo_insert     insertion of newly created outputs into the UTXO map
utxo_hash       rolling UTXO accumulator updates
row             per-block output row construction
other           remaining process time not covered by the sub-timers
```

For long scans, the UTXO map and Taproot-key set can be pre-allocated to
reduce repeated growth and rehashing:

```bash
btc-data-stats \
  --utxo-reserve 30000000 \
  --seen-keys-reserve 1000000
```

This can improve process time, but it allocates memory earlier.
Use values appropriate for the scan range and available RAM.
