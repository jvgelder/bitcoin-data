//! bitcoind JSON-RPC source (verbosity=0 → raw hex).

use async_trait::async_trait;
use btc_data_core::block::RawBlockFrame;
use btc_data_core::source::BlockSource;
use bytes::Bytes;
use std::path::{Path, PathBuf};

pub struct RpcSource {
    rpc: crate::rpc_client::BitcoinRpc,
    name: String,
}

impl RpcSource {
    pub fn new(url: String, user: String, pass: String) -> Self {
        let name = url.clone();
        Self {
            rpc: crate::rpc_client::BitcoinRpc::new(url, user, pass),
            name,
        }
    }

    /// Create an RPC source using explicit username/password when supplied,
    /// otherwise falling back to Bitcoin Core cookie authentication.
    pub fn new_auto_auth(
        url: String,
        user: String,
        pass: String,
        cookie_file: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let (user, pass) = resolve_auth(&url, user, pass, cookie_file.as_deref())?;
        Ok(Self::new(url, user, pass))
    }
}

fn resolve_auth(
    url: &str,
    user: String,
    pass: String,
    cookie_file: Option<&Path>,
) -> anyhow::Result<(String, String)> {
    match (user.is_empty(), pass.is_empty()) {
        (false, false) => Ok((user, pass)),
        (true, true) => read_cookie_auth(url, cookie_file),
        _ => anyhow::bail!(
            "RPC auth requires both --rpc-user and --rpc-pass, or neither to use cookie auth"
        ),
    }
}

fn read_cookie_auth(url: &str, cookie_file: Option<&Path>) -> anyhow::Result<(String, String)> {
    let candidates = if let Some(path) = cookie_file {
        vec![path.to_path_buf()]
    } else {
        default_cookie_candidates(url)
    };

    let mut attempted = Vec::new();

    for path in candidates {
        attempted.push(path.clone());
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };

        let cookie = contents.trim();
        let Some((user, pass)) = cookie.split_once(':') else {
            anyhow::bail!("invalid RPC cookie format in {}", path.display());
        };

        if user.is_empty() || pass.is_empty() {
            anyhow::bail!(
                "invalid RPC cookie in {}: empty username or password",
                path.display()
            );
        }

        return Ok((user.to_owned(), pass.to_owned()));
    }

    let attempted = attempted
        .into_iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    anyhow::bail!(
        "RPC username/password not supplied and no readable Bitcoin Core cookie file found; tried: {attempted}. Use --rpc-user/--rpc-pass or --rpc-cookie-file"
    )
}

fn default_cookie_candidates(url: &str) -> Vec<PathBuf> {
    let Some(datadir) = default_bitcoin_datadir() else {
        return Vec::new();
    };

    let mut candidates = Vec::new();

    // Prefer the network-specific default based on the common Bitcoin Core RPC ports.
    if url.contains(":18443") {
        candidates.push(datadir.join("regtest").join(".cookie"));
    } else if url.contains(":18332") {
        candidates.push(datadir.join("testnet3").join(".cookie"));
    } else if url.contains(":38332") {
        candidates.push(datadir.join("signet").join(".cookie"));
    } else {
        candidates.push(datadir.join(".cookie"));
    }

    // Fallbacks make local development more forgiving.
    candidates.push(datadir.join(".cookie"));
    candidates.push(datadir.join("regtest").join(".cookie"));
    candidates.push(datadir.join("testnet3").join(".cookie"));
    candidates.push(datadir.join("signet").join(".cookie"));

    let mut unique = Vec::new();
    for candidate in candidates {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }
    unique
}

fn default_bitcoin_datadir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("BITCOIN_DATA_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Bitcoin"))
    }

    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|p| {
            PathBuf::from(p)
                .join("Library")
                .join("Application Support")
                .join("Bitcoin")
        })
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".bitcoin"))
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    {
        None
    }
}

#[async_trait]
impl BlockSource for RpcSource {
    async fn get_block_hash(&self, height: u64) -> anyhow::Result<[u8; 32]> {
        let hex_str = self.rpc.get_block_hash(height).await?;
        let mut out = [0u8; 32];
        hex::decode_to_slice(&hex_str, &mut out)?;
        // Bitcoin reports block hashes reversed.
        out.reverse();
        Ok(out)
    }

    async fn get_block_raw(&self, hash: [u8; 32]) -> anyhow::Result<Bytes> {
        let mut display = hash;
        display.reverse();
        let hex_str = hex::encode(display);
        Ok(Bytes::from(self.rpc.get_block_raw_hex(&hex_str).await?))
    }

    async fn get_best_height(&self) -> anyhow::Result<u64> {
        self.rpc.get_block_count().await
    }

    async fn get_block_range_by_height(
        &self,
        start_height: u64,
        count: usize,
    ) -> anyhow::Result<Vec<RawBlockFrame>> {
        let hashes_hex = self.rpc.get_block_hashes(start_height, count).await?;

        let mut hashes = Vec::with_capacity(hashes_hex.len());
        for hash_hex in &hashes_hex {
            let mut hash = [0u8; 32];
            hex::decode_to_slice(hash_hex, &mut hash)?;
            // Bitcoin Core returns display-order hashes. Internally this
            // project uses consensus/hash byte order.
            hash.reverse();
            hashes.push(hash);
        }

        let bytes = self.rpc.get_blocks_raw_hex(&hashes_hex).await?;
        if bytes.len() != hashes.len() {
            anyhow::bail!(
                "RPC batch returned {} blocks for {} hashes",
                bytes.len(),
                hashes.len()
            );
        }

        Ok(hashes
            .into_iter()
            .zip(bytes)
            .enumerate()
            .map(|(offset, (hash, bytes))| RawBlockFrame {
                height: start_height + offset as u64,
                hash,
                bytes: Bytes::from(bytes),
            })
            .collect())
    }

    fn supports_block_range_batches(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        &self.name
    }
}
