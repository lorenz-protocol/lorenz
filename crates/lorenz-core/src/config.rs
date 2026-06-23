//! Typed configuration.
//!
//! Unknown keys are rejected (`deny_unknown_fields`) so a typo in a config file
//! fails loudly instead of being silently ignored. This is a direct lesson from
//! auditing a prior project where whole config sections (`[jito]`,
//! `[kamino_flashloan]`) were silently dropped because the struct never read
//! them; see docs/adr.

use crate::error::{Error, Result};
use crate::types::Bps;
use serde::{Deserialize, Serialize};

/// Root configuration for a single engine instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineConfig {
    pub rpc: RpcConfig,
    pub risk: RiskConfig,
    pub costs: CostConfig,
}

/// Endpoints. We deliberately do NOT hold any private key in config types:
/// signing material is loaded at the process edge, never serialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcConfig {
    /// Read endpoint (account/stream subscriptions).
    pub url: String,
    /// Optional Yellowstone/Geyser gRPC endpoint for push updates.
    #[serde(default)]
    pub geyser_url: Option<String>,
}

/// Hard limits enforced by the control plane (and mirrored on-chain). These are
/// caps the agent may tighten but must never exceed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskConfig {
    /// Maximum notional per arbitrage, in base units of the loaned asset.
    pub max_position: u64,
    /// Minimum net profit (after costs) required to fire, in base units.
    pub min_profit: u64,
    /// Consecutive failed/loss trades before the kill-switch trips.
    pub max_consecutive_losses: u32,
}

/// Cost model parameters used by both the backtester and the live engine, so
/// simulated economics match production accounting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostConfig {
    /// Flash-loan provider fee.
    pub flash_loan_fee: Bps,
    /// Priority fee per transaction, in lamports.
    pub priority_fee_lamports: u64,
    /// Jito bundle tip, in lamports.
    pub jito_tip_lamports: u64,
    /// Assumed extra slippage applied per hop on top of pool math.
    pub slippage_per_hop: Bps,
}

impl EngineConfig {
    /// Parse from a TOML string. Returns a descriptive [`Error::Config`] on
    /// failure rather than panicking.
    pub fn from_toml(s: &str) -> Result<Self> {
        toml::from_str(s).map_err(|e| Error::Config(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        [rpc]
        url = "https://api.mainnet-beta.solana.com"

        [risk]
        max_position = 2_000_000_000000
        min_profit = 100_000
        max_consecutive_losses = 5

        [costs]
        flash_loan_fee = 9
        priority_fee_lamports = 50_000
        jito_tip_lamports = 10_000
        slippage_per_hop = 5
    "#;

    #[test]
    fn parses_sample_config() {
        let cfg = EngineConfig::from_toml(SAMPLE).expect("valid config");
        assert_eq!(cfg.risk.max_consecutive_losses, 5);
        assert_eq!(cfg.costs.flash_loan_fee, Bps(9));
        assert_eq!(cfg.rpc.geyser_url, None);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let bad = format!("{SAMPLE}\n[jito]\nenabled = true\n");
        // A typo'd / unsupported section must fail loudly, not be ignored.
        assert!(EngineConfig::from_toml(&bad).is_err());
    }
}
