# bitcoin-data

`bitcoin-data` is a Rust workspace for getting Bitcoin block and transaction
information out of one or more sources and into reusable in-process or
cross-process representations.

The native Bitcoin data format is Bitcoin Core's consensus-encoded binary block
format, usually fetched from a node through JSON-RPC, REST, or local IPC. That is
ideal for Bitcoin nodes, but downstream consumers often need a different access
pattern: analytics jobs, dashboards, search indexes, warehouses, scanners, or
light clients. This workspace provides shared source, parsing, and encoding
building blocks for those consumers.

## Naming

```text
bitcoin-data       workspace / repository
btc-data-*         workspace crates
btc-data-sources   concrete block-source implementations
btc-data-stats     companion statistics scanner
```

The root README describes the workspace. Crate-specific behavior lives in the
crate READMEs.

## What it does

```text
                ┌──────────────┐      ┌───────────────┐      ┌──────────────┐
   sources ────▶│  raw blocks  │ ───▶ │   decoded     │ ───▶ │   encoded    │ ───▶ consumers
                │ bytes::Bytes │      │ bitcoin::Block│      │ capnp/proto/ │
                └──────────────┘      └───────────────┘      │  avro/json   │
                                                             └──────────────┘
```

Three reusable layers:

- **Sources** decide where blocks come from. The shared source crate provides
  JSON-RPC, REST, Esplora, Bitcoin Core IPC, and `MultiSource` implementations.
  See [`sources/Readme.md`](sources/Readme.md).
- **Core pipeline** owns shared traits and block-frame types. Raw block payloads
  use `bytes::Bytes`, so sources that already receive shared byte buffers can
  pass payloads through without copying.
- **Encodings** decide what blocks look like on the wire. Cap'n Proto is the
  canonical wire-format pivot; JSON / Protobuf / Avro adapters read the Cap'n
  Proto form so the schema stays the single source of truth.

## Workspace layout

```text
core/             # shared BlockSource, TipWatcher, block frames, pipeline helpers
encoding/
  capnp/          # canonical wire format; schema lives here
  raw/            # LEB128 / Elias gamma / CompactSize byte-level primitives
light-server/     # Silent payment light client server
sources/          # JSON-RPC, REST, Esplora, IPC, MultiSource implementations
stats/            # Bitcoin Stats companion scanner; see stats/README.md
```

## Shared source layer

`core/` defines the source-facing traits used across the workspace:

- `BlockSource`: resolves heights and hashes to raw consensus-encoded block
  bytes.
- `TipWatcher`: lets long-running consumers wait for chain-tip changes when a
  source supports notifications. IPC can use Bitcoin Core chain notifications;
  polling is a fallback for sources without native notifications.

Concrete implementations live in `sources/`:

```text
RpcSource       Bitcoin Core JSON-RPC
RestSource      Bitcoin Core REST .bin block payloads
EsploraSource   Esplora-compatible HTTP APIs
IpcSource       experimental Bitcoin Core multiprocess IPC source
MultiSource     round-robin/fallback wrapper over several sources
```

Detailed source behavior, configuration concepts, and tuning guidance are in
[`sources/Readme.md`](sources/Readme.md). Keep downstream scanner/indexer CLI
usage in the consuming crate READMEs, not in the source crate docs.

## Encodings

`bitcoin-data` sources fetch raw consensus block bytes. The shared core parse
layer can decode those bytes once into `bitcoin::Block`, the rust-bitcoin
in-memory representation. For consumers in the same process, that may be enough.
For cross-process or cross-language consumers, encoding adapters convert the
block into a wire format.

| Encoding | Status | Notes |
| -------- | ------ | ----- |
| **Cap'n Proto** | Schema + encoder implemented | The canonical wire format. Schemas in `encoding/capnp/schema/`. Other adapters read the Cap'n Proto form so the schema is the single source of truth. |
| **Raw bytes** | Implemented | LEB128 / Elias / CompactSize primitives. Used by stats-side compact encodings. |

Schemas are kept aligned manually. Add fields to the canonical Cap'n Proto
schema first, then update the Protobuf/Avro mirrors.

## Bitcoin Stats

Bitcoin Stats is a companion scanner under `stats/`. It reuses the shared source
layer to compute per-block and chain-derived statistics, and it has its own CLI,
checkpointing, metrics, and export documentation.

See [`stats/Readme.md`](stats/Readme.md).

## Optional IPC source

The shared source layer contains an experimental Bitcoin Core multiprocess IPC
source. IPC is no longer just a planned source: it is used by workspace consumers
that need local high-throughput block access and, where wired, chain-tip
notification support through `TipWatcher`.

The IPC implementation depends on Bitcoin Core multiprocess/Chain IPC bindings.
Exact feature flags are crate-specific; check the consuming crate README and
Cargo features before building IPC-enabled binaries.

At the source layer, IPC maps the shared block-source operations to Chain IPC
methods such as:

```text
get_best_height()
get_block_hash(height)
get_block_raw(hash)
get_block_by_height(height)
```

For long-running consumers, IPC should prefer native chain notifications over
polling when `TipWatcher` is implemented for the active source.

## Build

```bash
# Install Cap'n Proto compiler (needed by encoding/capnp build)
sudo apt install capnproto    # Debian/Ubuntu
brew install capnproto        # macOS

cargo build --workspace --release
```

See crate-specific READMEs for running binaries and choosing source settings:

```text
sources/Readme.md       source behavior and configuration concepts
stats/Readme.md         Bitcoin Stats scanner usage
```
