use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Bitcoin network used to choose archive defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveNetwork {
    #[default]
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

    /// The archive is fixed to P2TR-oriented data; on Taproot networks the
    /// default scan starts at Taproot activation.
    pub fn default_start_height(self) -> u64 {
        match self {
            Self::Mainnet => 709_632,
            Self::Testnet => 2_011_968,
            Self::Signet | Self::Regtest | Self::Fixture => 0,
        }
    }
}

impl FromStr for ArchiveNetwork {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mainnet" | "bitcoin" => Ok(Self::Mainnet),
            "testnet" | "test" => Ok(Self::Testnet),
            "signet" => Ok(Self::Signet),
            "regtest" => Ok(Self::Regtest),
            "fixture" => Ok(Self::Fixture),
            other => anyhow::bail!("unknown archive network: {other}"),
        }
    }
}

impl fmt::Display for ArchiveNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p2tr_archive_defaults_to_taproot_activation_on_mainnet() {
        assert_eq!(ArchiveNetwork::Mainnet.default_start_height(), 709_632);
    }

    #[test]
    fn p2tr_archive_defaults_to_taproot_activation_on_testnet() {
        assert_eq!(ArchiveNetwork::Testnet.default_start_height(), 2_011_968);
    }

    #[test]
    fn non_taproot_activation_networks_default_to_zero() {
        assert_eq!(ArchiveNetwork::Signet.default_start_height(), 0);
        assert_eq!(ArchiveNetwork::Regtest.default_start_height(), 0);
        assert_eq!(ArchiveNetwork::Fixture.default_start_height(), 0);
    }
}
