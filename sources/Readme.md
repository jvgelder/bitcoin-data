# btc-data-sources

`btc-data-sources` contains concrete block-source implementations used by
workspace consumers. It does not own scanner, indexer, sink, archive, or server
logic.

The public boundary is defined in `btc_data_core::source`:

```text
BlockSource  fetches block data and chain-tip information
TipWatcher   waits for chain-tip changes when a source can support it
```

Consumers such as scanners, indexers, and servers should depend on these traits
instead of depending on a specific transport directly.

## Source implementations

The crate currently provides:

```text
EsploraSource    HTTP source backed by an Esplora-compatible API
RpcSource        Bitcoin Core JSON-RPC source
RestSource       Bitcoin Core REST source
IpcSource        Bitcoin Core multiprocess IPC source
MultiSource      wrapper that distributes requests across multiple sources
```

`IpcSource` is available when the source crate is built with IPC support and the
workspace has the generated Bitcoin Core IPC bindings available.

## BlockSource

`BlockSource` is the shared block-fetching abstraction. A source implementation
is expected to provide:

```text
name()
get_best_height()
get_block_hash(height)
get_block_raw(hash)
get_block_by_height(height)
get_block_range_by_height(start_height, count)
```

The default range helper may call the single-block methods repeatedly, while a
source is free to override it with a more efficient transport-specific strategy.

All hashes are raw 32-byte block hashes in internal byte order. Presentation code
may reverse bytes when displaying conventional block-hash hex.

## TipWatcher

`TipWatcher` is the shared waiting abstraction for long-running consumers.

```text
wait_for_tip_change(old_tip)
```

A watcher should return when the best chain tip differs from `old_tip`. Native
sources should use transport-level notifications where available. Polling should
be used only for sources that do not expose notifications.

Current intended behavior:

```text
IPC      waits on Bitcoin Core chain notifications
REST     uses polling fallback
RPC      may use polling fallback
Esplora  may use polling fallback
```

This keeps long-running services from duplicating their own tip-watching logic.

## EsploraSource

`EsploraSource` talks to an Esplora-compatible HTTP API.

It is useful for:

- public or remote data sources
- development when no local Bitcoin Core node is available
- environments where trusted local validation is not required by the consumer

Characteristics:

- simple HTTP transport
- no Bitcoin Core cookie or RPC credentials
- external service availability and rate limits may apply
- block payload retrieval depends on the Esplora-compatible endpoint behavior

Consumers that require local trust-minimized operation should prefer a local
Bitcoin Core source such as REST, RPC, or IPC.

## RpcSource

`RpcSource` talks to Bitcoin Core JSON-RPC.

It is useful for:

- local Bitcoin Core nodes without REST enabled
- deployments that already use RPC authentication
- sources where batch JSON-RPC requests are desirable

Authentication can be configured by the consumer through the RPC source
configuration. Supported authentication modes include cookie authentication and
explicit username/password authentication, depending on the current source
constructor used by the workspace.

Characteristics:

- widely available on Bitcoin Core nodes
- can use batch requests for hash and raw-block calls
- raw blocks are commonly returned as hex strings, which adds decoding overhead
- HTTP round trips and JSON payload sizes can become bottlenecks on large scans

RPC batching is most useful when the consumer needs many contiguous blocks and
can tolerate larger JSON responses.

## RestSource

`RestSource` talks to Bitcoin Core REST endpoints.

It is useful for:

- local Bitcoin Core nodes with REST enabled
- high-throughput raw block reads without JSON block hex wrapping
- simple local development and benchmarking

Characteristics:

- returns raw `.bin` block payloads for block reads
- avoids JSON-RPC hex decoding overhead for block payloads
- still uses HTTP
- requires Bitcoin Core REST to be enabled on the node

REST generally should not group raw block downloads into artificial large
transport batches. Concurrency is usually controlled by the consumer pipeline.

## IpcSource

`IpcSource` talks to Bitcoin Core through the multiprocess IPC Chain interface.

It is useful for:

- local high-throughput block ingestion
- long-running consumers that should wait on native chain-tip notifications
- avoiding JSON-RPC/REST HTTP overhead

Characteristics:

- fetches block data through the Bitcoin Core Chain IPC interface
- can use multiple IPC thread clients for concurrent requests
- supports native tip waiting through Chain notification methods when wired
- depends on compatible Bitcoin Core multiprocess IPC bindings

The IPC source maps the shared source abstraction onto Chain IPC methods, for
example:

```text
get_best_height()       -> Chain height query
get_block_hash(height)  -> Chain block-hash query
get_block_raw(hash)     -> Chain block lookup with block data
get_block_by_height()   -> Chain height lookup with hash and block data
TipWatcher              -> Chain wait-for-notifications methods
```

IPC-specific concurrency should be controlled with the IPC thread count exposed
by the source configuration. Consumer-level request buffering can then keep
multiple height or hash requests in flight.

## MultiSource

`MultiSource` combines multiple `BlockSource` implementations behind one source.

It is useful for:

- simple failover
- spreading requests across equivalent sources
- comparing source behavior behind the same consumer pipeline

Typical behavior:

- requests are distributed across configured sources
- if one source fails, another source can be tried
- consumers still interact with a single `BlockSource`

All configured sources should refer to the same Bitcoin network and chain. A
consumer that mixes networks or inconsistent chain views should expect errors or
invalid results.

## Source selection guidance

At the source layer, the usual tradeoffs are:

```text
Esplora  easiest remote HTTP source, but external-service dependent
RPC      broadly available local Core interface, but JSON/hex overhead
REST     simple local Core HTTP source with raw block bytes
IPC      fastest local Core path and native notification support
Multi    wrapper for failover or request distribution
```

Source choice should be made by the consuming application based on its trust,
performance, deployment, and availability requirements.

## Error handling

Concrete sources should return `anyhow::Result` errors with enough context for
the consumer to identify the failing transport operation.

`MultiSource` can try alternate sources on per-source failures, but it should not
hide persistent configuration errors such as invalid credentials, wrong network,
or malformed URLs.

Long-running consumers should treat source errors separately from data errors:

```text
source/transport error  retry or switch source
block decode error      investigate data/corruption/implementation bug
chain mismatch          stop and require operator action
```

## Reuse boundary

This crate should stay focused on source transport. It should not contain:

- statistics logic
- Silent Payments light-sync logic
- database sinks
- HTTP serving routes
- application-specific checkpoint formats

Shared script classification, Taproot spend classification, and BIP352 helper
logic should live in a common chain/protocol crate rather than in
`btc-data-sources`. The source crate should only fetch chain data and report tip
changes.
