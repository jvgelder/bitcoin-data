use crate::light_capnp::{self, light_block_profile};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Bitcoin network used to choose archive defaults.
///
/// The archive network is immutable once a SQLite DB/archive is initialized.
/// Changing it requires a separate DB/rescan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveNetwork {
    Mainnet,
    Testnet,
    Signet,
    Regtest,
    Fixture,
}

impl ArchiveNetwork {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Signet => "signet",
            Self::Regtest => "regtest",
            Self::Fixture => "fixture",
        }
    }
}

impl Default for ArchiveNetwork {
    fn default() -> Self {
        Self::Mainnet
    }
}

impl FromStr for ArchiveNetwork {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mainnet" | "bitcoin" | "btc" => Ok(Self::Mainnet),
            "testnet" | "testnet3" => Ok(Self::Testnet),
            "signet" => Ok(Self::Signet),
            "regtest" => Ok(Self::Regtest),
            "fixture" | "fixtures" => Ok(Self::Fixture),
            other => anyhow::bail!("unknown archive network: {other}"),
        }
    }
}

impl fmt::Display for ArchiveNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Immutable archive UID namespace. Scope is selected when an archive/DB is
/// initialized; switching scope requires a full rescan because UID assignment
/// changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveScope {
    /// Silent Payments candidate scope: UID every P2TR output after Taproot
    /// activation. Reused P2TR keys are included. NUMS is an input-side BIP352
    /// spend-eligibility concern, not an output-level creation filter.
    P2trSp,
    /// UID every P2TR output, including reused keys.
    P2tr,
    /// Broad scope: UID every Bitcoin output when the archive is initialized with this scope.
    AllOutputs,
}

impl ArchiveScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::P2trSp => "p2tr-sp",
            Self::P2tr => "p2tr",
            Self::AllOutputs => "all-outputs",
        }
    }

    pub fn capnp(self) -> light_capnp::ArchiveScope {
        match self {
            Self::P2trSp => light_capnp::ArchiveScope::P2trSp,
            Self::P2tr => light_capnp::ArchiveScope::P2tr,
            Self::AllOutputs => light_capnp::ArchiveScope::AllOutputs,
        }
    }

    pub fn include_output(self, is_p2tr: bool, _is_nums: bool) -> bool {
        match self {
            // Do not apply NUMS filtering at output creation time. BIP352 NUMS
            // handling is an input-side Taproot script-path spend rule.
            Self::P2trSp => is_p2tr,
            Self::P2tr => is_p2tr,
            Self::AllOutputs => true,
        }
    }

    pub fn include_p2tr_output(self, is_nums: bool) -> bool {
        self.include_output(true, is_nums)
    }

    /// Default first block to scan for this archive scope and network.
    ///
    /// For P2TR-scoped archives, the UID namespace starts at Taproot activation.
    /// For `all-outputs`, the default starts at genesis unless explicitly overridden.
    pub fn default_start_height(self, network: ArchiveNetwork) -> u64 {
        match self {
            Self::P2trSp | Self::P2tr => match network {
                ArchiveNetwork::Mainnet => 709_632,
                ArchiveNetwork::Testnet => 2_011_968,
                ArchiveNetwork::Signet | ArchiveNetwork::Regtest | ArchiveNetwork::Fixture => 0,
            },
            Self::AllOutputs => 0,
        }
    }
}

impl FromStr for ArchiveScope {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "p2tr-sp" | "p2trSp" | "sp" => Ok(Self::P2trSp),
            "p2tr" | "p2tr-only" | "p2trOnly" => Ok(Self::P2tr),
            "all-outputs" | "allOutputs" => Ok(Self::AllOutputs),
            other => anyhow::bail!("unknown archive scope: {other}"),
        }
    }
}

impl fmt::Display for ArchiveScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Materialized view inside one immutable archive scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Profile {
    pub scope: ArchiveScope,
    /// 0 means raw/no cut-through. Non-zero profiles are materialized from the
    /// same canonical scope and must use matching checkpoints.
    pub cutthrough_blocks: u32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            scope: ArchiveScope::P2trSp,
            cutthrough_blocks: 0,
        }
    }
}

impl Profile {
    pub fn raw_p2tr_sp() -> Self {
        Self::default()
    }

    pub fn file_tag(self) -> String {
        format!("scope-{}.ct{}", self.scope.as_str(), self.cutthrough_blocks)
    }

    pub fn fill_capnp(self, mut b: light_block_profile::Builder<'_>) {
        b.set_scope(self.scope.capnp());
        b.set_cut_through_blocks(self.cutthrough_blocks);
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file_tag())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_tag() {
        assert_eq!(Profile::default().file_tag(), "scope-p2tr-sp.ct0");
    }

    #[test]
    fn p2tr_sp_does_not_filter_output_nums() {
        assert!(ArchiveScope::P2trSp.include_p2tr_output(false));
        assert!(ArchiveScope::P2trSp.include_p2tr_output(true));
    }

    #[test]
    fn p2tr_includes_p2tr_outputs() {
        assert!(ArchiveScope::P2tr.include_p2tr_output(true));
    }

    #[test]
    fn p2tr_scopes_default_to_taproot_activation_on_mainnet() {
        assert_eq!(ArchiveScope::P2trSp.default_start_height(ArchiveNetwork::Mainnet), 709_632);
        assert_eq!(ArchiveScope::P2tr.default_start_height(ArchiveNetwork::Mainnet), 709_632);
    }

    #[test]
    fn p2tr_scopes_default_to_taproot_activation_on_testnet() {
        assert_eq!(ArchiveScope::P2trSp.default_start_height(ArchiveNetwork::Testnet), 2_011_968);
        assert_eq!(ArchiveScope::P2tr.default_start_height(ArchiveNetwork::Testnet), 2_011_968);
    }
}
