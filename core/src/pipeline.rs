//! Fetch → ordered-delivery pipeline.
//!
//! Sources only provide raw consensus bytes. This module provides both raw
//! streams and decoded streams. The decoded streams are the shared boundary
//! where raw bytes are converted into rust-bitcoin `Block`s.
//!
//! Stages:
//!   1. **Hash prefetch** where useful: resolve height → hash.
//!   2. **Fetch**: keep up to `buffer` raw block requests in flight.
//!   3. **Decode**: convert raw consensus bytes into `bitcoin::Block`.
//!   4. **Ordered delivery**: frames are yielded in strict height order via
//!      `futures::stream::buffered`.
//!
//! Sequential commit (UTXO map updates, ID assignment) must still happen in
//! height order because block N+1's spends reference block N's outputs. That
//! sequencing is the consumer's responsibility, not the pipeline's.

use crate::block::RawBlockFrame;
use crate::parse::{decode_raw_block, DecodedBlockFrame};
use crate::source::BlockSource;
use futures::stream::{Stream, StreamExt};
use std::sync::Arc;

pub async fn prefetch_hashes(
    source: &Arc<dyn BlockSource>,
    start: u64,
    count: u64,
    concurrency: usize,
) -> anyhow::Result<Vec<[u8; 32]>> {
    let hashes: Vec<anyhow::Result<[u8; 32]>> = futures::stream::iter(start..(start + count))
        .map(|h| {
            let src = source.clone();
            async move { src.get_block_hash(h).await }
        })
        .buffered(concurrency)
        .collect()
        .await;
    hashes.into_iter().collect()
}

/// Stream of [`RawBlockFrame`] in strict height order, with up to `buffer`
/// fetches in flight concurrently.
pub fn raw_block_stream(
    source: Arc<dyn BlockSource>,
    start_height: u64,
    hashes: Vec<[u8; 32]>,
    buffer: usize,
) -> impl Stream<Item = anyhow::Result<RawBlockFrame>> {
    futures::stream::iter(hashes.into_iter().enumerate())
        .map(move |(i, hash)| {
            let source = source.clone();
            let height = start_height + i as u64;
            async move {
                let bytes = source.get_block_raw(hash).await?;
                Ok::<_, anyhow::Error>(RawBlockFrame {
                    height,
                    hash,
                    bytes,
                    spent_txouts: None,
                })
            }
        })
        .buffered(buffer)
}

/// Stream of decoded blocks in strict height order, using an existing ordered
/// height->hash list and fetching raw blocks by hash.
///
/// Sources still only provide raw consensus bytes. This stage is the shared
/// core boundary where raw bytes are decoded into rust-bitcoin blocks.
pub fn decoded_block_stream(
    source: Arc<dyn BlockSource>,
    start_height: u64,
    hashes: Vec<[u8; 32]>,
    buffer: usize,
) -> impl Stream<Item = anyhow::Result<DecodedBlockFrame>> {
    raw_block_stream(source, start_height, hashes, buffer).map(|raw| raw.and_then(decode_raw_block))
}

/// Stream of [`RawBlockFrame`] in strict height order, fetching directly by
/// height with up to `buffer` requests in flight.
///
/// This lets sources such as Bitcoin Core IPC use one height-based call that
/// returns height/hash/data together, while sources without an override fall
/// back to the default hash+raw implementation.
pub fn raw_block_stream_by_height(
    source: Arc<dyn BlockSource>,
    start_height: u64,
    count: u64,
    buffer: usize,
) -> impl Stream<Item = anyhow::Result<RawBlockFrame>> {
    futures::stream::iter(start_height..(start_height + count))
        .map(move |height| {
            let source = source.clone();
            async move { source.get_block_by_height(height).await }
        })
        .buffered(buffer)
}

/// Stream of decoded blocks in strict height order, fetching each raw block by
/// height and decoding it in the shared core pipeline.
pub fn decoded_block_stream_by_height(
    source: Arc<dyn BlockSource>,
    start_height: u64,
    count: u64,
    buffer: usize,
) -> impl Stream<Item = anyhow::Result<DecodedBlockFrame>> {
    raw_block_stream_by_height(source, start_height, count, buffer)
        .map(|raw| raw.and_then(decode_raw_block))
}

/// Stream of [`RawBlockFrame`] in strict height order using source-native
/// batches when available.
///
/// `buffer` controls how many batch requests can be in flight.
/// `batch_size` controls how many contiguous heights each batch contains.
/// Use `batch_size = 1` to preserve one request per height. RPC sources can
/// use larger values to send JSON-RPC batch requests.
pub fn raw_block_stream_by_height_batched(
    source: Arc<dyn BlockSource>,
    start_height: u64,
    count: u64,
    buffer: usize,
    batch_size: usize,
) -> impl Stream<Item = anyhow::Result<RawBlockFrame>> {
    let batch_size = batch_size.max(1) as u64;
    let batches = if count == 0 {
        0
    } else {
        count.div_ceil(batch_size)
    };

    futures::stream::iter(0..batches)
        .map(move |batch_idx| {
            let source = source.clone();
            let batch_start = start_height + batch_idx * batch_size;
            let remaining = start_height + count - batch_start;
            let this_count = remaining.min(batch_size) as usize;

            async move {
                source
                    .get_block_range_by_height(batch_start, this_count)
                    .await
            }
        })
        .buffered(buffer)
        .flat_map(|result| {
            let items: Vec<anyhow::Result<RawBlockFrame>> = match result {
                Ok(frames) => frames.into_iter().map(Ok).collect(),
                Err(err) => vec![Err(err)],
            };
            futures::stream::iter(items)
        })
}

/// Stream of decoded blocks in strict height order using source-native raw block
/// batches when available, then decoding every raw frame through the shared
/// core parser.
pub fn decoded_block_stream_by_height_batched(
    source: Arc<dyn BlockSource>,
    start_height: u64,
    count: u64,
    buffer: usize,
    batch_size: usize,
) -> impl Stream<Item = anyhow::Result<DecodedBlockFrame>> {
    raw_block_stream_by_height_batched(source, start_height, count, buffer, batch_size)
        .map(|raw| raw.and_then(decode_raw_block))
}
