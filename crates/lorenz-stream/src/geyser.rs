//! Production transport seam for a live Yellowstone/Geyser gRPC subscription.
//!
//! This module defines the typed integration point and the exact data flow a
//! live consumer follows. It deliberately does NOT pull the heavyweight
//! `yellowstone-grpc-client` (and its transitive `solana-sdk`) into the core
//! workspace: that dependency belongs in a deployment build, not in the
//! deterministic crates that must stay light and fast to compile.
//!
//! Intended wiring (production step):
//!
//! 1. Connect to the Geyser endpoint with `yellowstone-grpc-client`, sending an
//!    auth token if required.
//! 2. Subscribe to `accounts` updates filtered to the pool accounts and their
//!    vault token accounts.
//! 3. For each pool-account update, decode it with the matching
//!    [`lorenz_dex::decoder::PoolAccountDecoder`]; for each vault update, read the
//!    balance with [`lorenz_dex::decoder::spl::token_account_amount`].
//! 4. When both vault balances for a pool are known, assemble a
//!    [`lorenz_dex::CpmmPool`] via [`lorenz_dex::decoder::PoolAccounts::assemble`]
//!    and emit an updated [`crate::PoolSnapshot`].
//!
//! Step 3 and 4 are already implemented and tested in `lorenz-dex`; only the gRPC
//! transport in steps 1-2 is environment-specific.

use crate::StreamError;

/// Connection parameters for a Geyser endpoint.
#[derive(Debug, Clone)]
pub struct GeyserConfig {
    pub endpoint: String,
    /// Optional `x-token` auth header for managed providers.
    pub x_token: Option<String>,
    /// Commitment level string ("processed" | "confirmed" | "finalized").
    pub commitment: String,
}

impl Default for GeyserConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            x_token: None,
            commitment: "confirmed".to_string(),
        }
    }
}

/// A live Geyser-backed source. Construction is cheap; `connect` is where the
/// transport would be established.
#[derive(Debug, Clone)]
pub struct GeyserSource {
    config: GeyserConfig,
}

impl GeyserSource {
    pub fn new(config: GeyserConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &GeyserConfig {
        &self.config
    }

    /// Establish the subscription. Not implemented in the core build by design;
    /// wire `yellowstone-grpc-client` in a deployment crate following this
    /// module's documented flow.
    pub fn connect(&self) -> Result<(), StreamError> {
        Err(StreamError::NotImplemented(
            "Geyser transport: add yellowstone-grpc-client in a deployment build",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_reports_explicit_not_implemented() {
        let s = GeyserSource::new(GeyserConfig::default());
        assert!(matches!(s.connect(), Err(StreamError::NotImplemented(_))));
    }
}
