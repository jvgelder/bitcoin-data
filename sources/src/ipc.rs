//! Experimental Bitcoin Core multiprocess IPC source.
//!
//! This source uses the generated Chain interface from the experimental
//! `bitcoin-capnp-types` PR #22 branch. It is intended for performance testing
//! against a Bitcoin Core build that includes bitcoin/bitcoin#29409 and exposes
//! an IPC socket with `-ipcbind=unix`.
//!
//! The implementation runs the Cap'n Proto RPC system on a dedicated local
//! actor thread. This keeps the public `BlockSource` implementation `Send + Sync`
//! while allowing the generated capnp clients to stay on a `LocalSet`.

use async_trait::async_trait;
use btc_data_core::{
    block::RawBlockFrame,
    source::{BlockSource, TipWatcher},
};
use bytes::Bytes;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

#[cfg(feature = "ipc")]
use bitcoin_capnp_types::{
    chain_capnp::chain,
    init_capnp::init,
    proxy_capnp::{thread, thread_map},
};
#[cfg(feature = "ipc")]
use capnp_rpc::{rpc_twoparty_capnp::Side, twoparty::VatNetwork, RpcSystem};
#[cfg(feature = "ipc")]
use tokio::net::UnixStream;
#[cfg(feature = "ipc")]
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Experimental source backed by Bitcoin Core's multiprocess Chain IPC API.
#[derive(Clone)]
pub struct IpcSource {
    socket_path: PathBuf,
    name: String,
    tx: tokio_mpsc::Sender<IpcRequest>,
}

enum IpcRequest {
    GetBestHeight {
        reply: oneshot::Sender<anyhow::Result<u64>>,
    },
    GetBlockHash {
        height: u64,
        reply: oneshot::Sender<anyhow::Result<[u8; 32]>>,
    },
    GetBlockRaw {
        hash: [u8; 32],
        reply: oneshot::Sender<anyhow::Result<Bytes>>,
    },
    GetBlockByHeight {
        height: u64,
        reply: oneshot::Sender<anyhow::Result<RawBlockFrame>>,
    },
    WaitForTipChange {
        old_tip: Option<[u8; 32]>,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
}

impl IpcSource {
    /// Connect to a Bitcoin Core multiprocess IPC Unix socket, for example:
    /// `/home/user/.bitcoin/regtest/node.sock`.
    #[cfg(feature = "ipc")]
    pub fn connect(socket_path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let thread_count = std::env::var("BTC_DATA_IPC_THREADS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(8);
        Self::connect_with_threads(socket_path, thread_count)
    }

    /// Connect with an explicit number of Bitcoin Core IPC worker threads.
    ///
    /// Each `Thread` client maps to an independent server-side worker in
    /// libmultiprocess. A pool of 4-8 workers is usually a better starting
    /// point than one worker when fetching historical block data.
    #[cfg(feature = "ipc")]
    pub fn connect_with_threads(
        socket_path: impl Into<PathBuf>,
        thread_count: usize,
    ) -> anyhow::Result<Self> {
        let socket_path = socket_path.into();
        let thread_count = thread_count.max(1);
        let name = format!("ipc:{}", socket_path.display());
        let channel_capacity = (thread_count * 8).max(128);
        let (tx, rx) = tokio_mpsc::channel(channel_capacity);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let actor_path = socket_path.clone();

        std::thread::Builder::new()
            .name("btc-data-ipc-source".to_owned())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        let _ = ready_tx.send(Err(anyhow::anyhow!(err)));
                        return;
                    }
                };

                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    match IpcActor::connect(&actor_path, thread_count).await {
                        Ok(actor) => {
                            let _ = ready_tx.send(Ok(()));
                            actor.run(rx).await;
                        }
                        Err(err) => {
                            let _ = ready_tx.send(Err(err));
                        }
                    }
                });
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                socket_path,
                name,
                tx,
            }),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(anyhow::anyhow!("IPC actor failed to start: {err}")),
        }
    }

    /// Non-IPC builds keep this constructor so CLI code can produce a clear
    /// error without conditional type-level changes.
    #[cfg(not(feature = "ipc"))]
    pub fn connect(_socket_path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        anyhow::bail!("IpcSource requires building btc-data-sources with --features ipc")
    }

    #[cfg(not(feature = "ipc"))]
    pub fn connect_with_threads(
        _socket_path: impl Into<PathBuf>,
        _thread_count: usize,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("IpcSource requires building btc-data-sources with --features ipc")
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    async fn request<T, F>(&self, make: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(oneshot::Sender<anyhow::Result<T>>) -> IpcRequest + Send,
    {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(make(reply))
            .await
            .map_err(|_| anyhow::anyhow!("IPC actor is not running"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("IPC actor dropped response"))?
    }
}

#[async_trait]
impl BlockSource for IpcSource {
    async fn get_block_hash(&self, height: u64) -> anyhow::Result<[u8; 32]> {
        self.request(|reply| IpcRequest::GetBlockHash { height, reply })
            .await
    }

    async fn get_block_raw(&self, hash: [u8; 32]) -> anyhow::Result<Bytes> {
        self.request(|reply| IpcRequest::GetBlockRaw { hash, reply })
            .await
    }

    async fn get_best_height(&self) -> anyhow::Result<u64> {
        self.request(|reply| IpcRequest::GetBestHeight { reply })
            .await
    }

    async fn get_block_by_height(&self, height: u64) -> anyhow::Result<RawBlockFrame> {
        self.request(|reply| IpcRequest::GetBlockByHeight { height, reply })
            .await
    }

    fn prefers_height_fetch(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait]
impl TipWatcher for IpcSource {
    async fn wait_for_tip_change(&self, old_tip: Option<[u8; 32]>) -> anyhow::Result<()> {
        self.request(|reply| IpcRequest::WaitForTipChange { old_tip, reply })
            .await
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(feature = "ipc")]
struct IpcActor {
    threads: Vec<thread::Client>,
    next: Cell<usize>,
    chain: chain::Client,
}

#[cfg(feature = "ipc")]
impl IpcActor {
    async fn connect(socket_path: &Path, thread_count: usize) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(socket_path).await.map_err(|err| {
            anyhow::anyhow!(
                "failed to connect to IPC socket {}: {err}",
                socket_path.display()
            )
        })?;

        let (reader, writer) = stream.into_split();
        let reader = futures::io::BufReader::with_capacity(4 << 20, reader.compat());
        let writer = futures::io::BufWriter::with_capacity(64 << 10, writer.compat_write());
        let mut reader_options = capnp::message::ReaderOptions::new();
        reader_options.traversal_limit_in_words = Some(16 * 1024 * 1024);
        let network = VatNetwork::new(reader, writer, Side::Client, reader_options);
        let mut rpc_system = RpcSystem::new(Box::new(network), None);
        let init: init::Client = rpc_system.bootstrap(Side::Server);
        tokio::task::spawn_local(async move {
            if let Err(err) = rpc_system.await {
                eprintln!("IPC RpcSystem terminated: {err}");
            }
        });

        let construct = init
            .construct_request()
            .send()
            .promise
            .await
            .map_err(|err| anyhow::anyhow!("IPC Init.construct failed: {err}"))?;
        let thread_map: thread_map::Client = construct
            .get()
            .map_err(|err| anyhow::anyhow!("IPC Init.construct response failed: {err}"))?
            .get_thread_map()
            .map_err(|err| {
                anyhow::anyhow!("IPC Init.construct did not return thread map: {err}")
            })?;

        let mut threads = Vec::with_capacity(thread_count);
        for index in 0..thread_count {
            let thread_response = thread_map
                .make_thread_request()
                .send()
                .promise
                .await
                .map_err(|err| {
                    anyhow::anyhow!("IPC ThreadMap.makeThread #{index} failed: {err}")
                })?;
            let thread: thread::Client = thread_response
                .get()
                .map_err(|err| anyhow::anyhow!("IPC makeThread #{index} response failed: {err}"))?
                .get_result()
                .map_err(|err| {
                    anyhow::anyhow!("IPC makeThread #{index} did not return thread: {err}")
                })?;
            threads.push(thread);
        }

        let mut req = init.make_chain_request();
        req.get()
            .get_context()
            .map_err(|err| anyhow::anyhow!("IPC makeChain context failed: {err}"))?
            .set_thread(threads[0].clone());
        let resp = req
            .send()
            .promise
            .await
            .map_err(|err| anyhow::anyhow!("IPC Init.makeChain failed: {err}"))?;
        let chain: chain::Client = resp
            .get()
            .map_err(|err| anyhow::anyhow!("IPC makeChain response failed: {err}"))?
            .get_result()
            .map_err(|err| anyhow::anyhow!("IPC makeChain did not return Chain client: {err}"))?;

        Ok(Self {
            threads,
            next: Cell::new(0),
            chain,
        })
    }

    fn pick_thread(&self) -> thread::Client {
        let index = self.next.get();
        self.next.set((index + 1) % self.threads.len());
        self.threads[index].clone()
    }

    async fn run(self, mut rx: tokio_mpsc::Receiver<IpcRequest>) {
        while let Some(req) = rx.recv().await {
            let chain = self.chain.clone();
            let thread = self.pick_thread();
            tokio::task::spawn_local(async move {
                match req {
                    IpcRequest::GetBestHeight { reply } => {
                        let _ = reply.send(get_best_height(&chain, &thread).await);
                    }
                    IpcRequest::GetBlockHash { height, reply } => {
                        let _ = reply.send(get_block_hash(&chain, &thread, height).await);
                    }
                    IpcRequest::GetBlockRaw { hash, reply } => {
                        let _ = reply.send(get_block_raw(&chain, &thread, hash).await);
                    }
                    IpcRequest::GetBlockByHeight { height, reply } => {
                        let _ = reply.send(get_block_by_height(&chain, &thread, height).await);
                    }
                    IpcRequest::WaitForTipChange { old_tip, reply } => {
                        let _ = reply.send(wait_for_tip_change(&chain, &thread, old_tip).await);
                    }
                }
            });
        }
    }
}

#[cfg(feature = "ipc")]
async fn get_best_height(chain: &chain::Client, thread: &thread::Client) -> anyhow::Result<u64> {
    let mut req = chain.get_height_request();
    req.get()
        .get_context()
        .map_err(|err| anyhow::anyhow!("IPC getHeight context failed: {err}"))?
        .set_thread(thread.clone());
    let resp = req
        .send()
        .promise
        .await
        .map_err(|err| anyhow::anyhow!("IPC Chain.getHeight failed: {err}"))?;
    let result = resp
        .get()
        .map_err(|err| anyhow::anyhow!("IPC getHeight response failed: {err}"))?;
    if !result.get_has_result() {
        anyhow::bail!("IPC Chain.getHeight returned no active tip");
    }
    let height = result.get_result();
    if height < 0 {
        anyhow::bail!("IPC Chain.getHeight returned negative height {height}");
    }
    Ok(height as u64)
}

#[cfg(feature = "ipc")]
async fn get_block_hash(
    chain: &chain::Client,
    thread: &thread::Client,
    height: u64,
) -> anyhow::Result<[u8; 32]> {
    let height_i32 = i32::try_from(height).map_err(|_| {
        anyhow::anyhow!("height {height} does not fit Bitcoin Core IPC Int32 height")
    })?;
    let mut req = chain.get_block_hash_request();
    req.get()
        .get_context()
        .map_err(|err| anyhow::anyhow!("IPC getBlockHash context failed: {err}"))?
        .set_thread(thread.clone());
    req.get().set_height(height_i32);

    let resp = req
        .send()
        .promise
        .await
        .map_err(|err| anyhow::anyhow!("IPC Chain.getBlockHash({height}) failed: {err}"))?;
    let hash = resp
        .get()
        .map_err(|err| anyhow::anyhow!("IPC getBlockHash response failed: {err}"))?
        .get_result()
        .map_err(|err| anyhow::anyhow!("IPC getBlockHash returned invalid data: {err}"))?;
    data_to_hash(hash, "getBlockHash")
}

#[cfg(feature = "ipc")]
async fn get_block_by_height(
    chain: &chain::Client,
    thread: &thread::Client,
    height: u64,
) -> anyhow::Result<RawBlockFrame> {
    let height_i32 = i32::try_from(height).map_err(|_| {
        anyhow::anyhow!("height {height} does not fit Bitcoin Core IPC Int32 height")
    })?;

    let mut req = chain.find_first_block_with_time_and_height_request();
    req.get()
        .get_context()
        .map_err(|err| {
            anyhow::anyhow!("IPC findFirstBlockWithTimeAndHeight context failed: {err}")
        })?
        .set_thread(thread.clone());
    req.get().set_min_time(0);
    req.get().set_min_height(height_i32);
    {
        let mut params = req.get().init_block();
        params.set_want_hash(true);
        params.set_want_height(true);
        params.set_want_data(true);
    }

    let resp = req.send().promise.await.map_err(|err| {
        anyhow::anyhow!("IPC Chain.findFirstBlockWithTimeAndHeight({height}) failed: {err}")
    })?;
    let result = resp.get().map_err(|err| {
        anyhow::anyhow!("IPC findFirstBlockWithTimeAndHeight response failed: {err}")
    })?;
    if !result.get_result() {
        anyhow::bail!(
            "IPC Chain.findFirstBlockWithTimeAndHeight did not find block at height {height}"
        );
    }

    let block = result.get_block().map_err(|err| {
        anyhow::anyhow!("IPC findFirstBlockWithTimeAndHeight block result invalid: {err}")
    })?;
    if !block.get_found() {
        anyhow::bail!(
            "IPC Chain.findFirstBlockWithTimeAndHeight returned found=false for height {height}"
        );
    }

    let returned_height = block.get_height();
    if returned_height != height_i32 {
        anyhow::bail!(
            "IPC Chain.findFirstBlockWithTimeAndHeight returned height {}, expected {}",
            returned_height,
            height
        );
    }

    let hash = data_to_hash(
        block
            .get_hash()
            .map_err(|err| anyhow::anyhow!("IPC height block returned no hash: {err}"))?,
        "findFirstBlockWithTimeAndHeight.hash",
    )?;
    let data = block
        .get_data()
        .map_err(|err| anyhow::anyhow!("IPC height block returned no block data: {err}"))?;
    let bytes = Bytes::copy_from_slice(data);

    Ok(RawBlockFrame {
        height,
        hash,
        bytes,
        spent_txouts: None,
    })
}

#[cfg(feature = "ipc")]
async fn get_block_raw(
    chain: &chain::Client,
    thread: &thread::Client,
    hash: [u8; 32],
) -> anyhow::Result<Bytes> {
    let mut req = chain.find_block_request();
    req.get()
        .get_context()
        .map_err(|err| anyhow::anyhow!("IPC findBlock context failed: {err}"))?
        .set_thread(thread.clone());
    req.get().set_hash(&hash);
    {
        let mut params = req.get().init_block();
        params.set_want_data(true);
        params.set_want_hash(false);
        params.set_want_height(false);
    }

    let resp = req
        .send()
        .promise
        .await
        .map_err(|err| anyhow::anyhow!("IPC Chain.findBlock failed: {err}"))?;
    let result = resp
        .get()
        .map_err(|err| anyhow::anyhow!("IPC findBlock response failed: {err}"))?;
    if !result.get_result() {
        anyhow::bail!(
            "IPC Chain.findBlock did not find block {}",
            display_hash(hash)
        );
    }
    let block = result
        .get_block()
        .map_err(|err| anyhow::anyhow!("IPC findBlock block result invalid: {err}"))?;
    let data = block
        .get_data()
        .map_err(|err| anyhow::anyhow!("IPC findBlock returned no block data: {err}"))?;
    Ok(Bytes::copy_from_slice(data))
}

#[cfg(feature = "ipc")]
async fn wait_for_tip_change(
    chain: &chain::Client,
    thread: &thread::Client,
    old_tip: Option<[u8; 32]>,
) -> anyhow::Result<()> {
    if let Some(old_tip) = old_tip {
        let mut req = chain.wait_for_notifications_if_tip_changed_request();
        req.get()
            .get_context()
            .map_err(|err| {
                anyhow::anyhow!("IPC waitForNotificationsIfTipChanged context failed: {err}")
            })?
            .set_thread(thread.clone());
        req.get().set_old_tip(&old_tip);
        req.send().promise.await.map_err(|err| {
            anyhow::anyhow!("IPC Chain.waitForNotificationsIfTipChanged failed: {err}")
        })?;
    } else {
        let mut req = chain.wait_for_notifications_request();
        req.get()
            .get_context()
            .map_err(|err| anyhow::anyhow!("IPC waitForNotifications context failed: {err}"))?
            .set_thread(thread.clone());
        req.send()
            .promise
            .await
            .map_err(|err| anyhow::anyhow!("IPC Chain.waitForNotifications failed: {err}"))?;
    }

    Ok(())
}

#[cfg(feature = "ipc")]
fn data_to_hash(data: &[u8], field: &str) -> anyhow::Result<[u8; 32]> {
    if data.len() != 32 {
        anyhow::bail!("IPC {field} returned {} bytes, expected 32", data.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(data);
    Ok(out)
}

#[cfg(feature = "ipc")]
fn display_hash(mut hash: [u8; 32]) -> String {
    hash.reverse();
    hex::encode(hash)
}
