//! Observability bootstrap and the structured records that make every run
//! replayable and auditable.

use crate::types::{Amount, PoolId, TokenId};
use serde::{Deserialize, Serialize};

/// Initialize a structured `tracing` subscriber driven by the `RUST_LOG`
/// environment variable. Idempotent-friendly: callers in tests can ignore the
/// error if a global subscriber is already set.
pub fn init_tracing() -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = fmt().with_env_filter(filter).with_target(true).finish();
    tracing::subscriber::set_global_default(subscriber)
}

/// A single hop in an arbitrage route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hop {
    pub pool: PoolId,
    pub token_in: TokenId,
    pub token_out: TokenId,
    pub amount_in: Amount,
    pub amount_out: Amount,
}

/// An immutable record of a decision the engine made. Persisted append-only so
/// any run can be reconstructed and any agent action explained after the fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeRecord {
    /// Unix timestamp (seconds).
    pub ts: u64,
    pub route: Vec<Hop>,
    /// Notional borrowed for the cycle.
    pub notional: Amount,
    /// Gross output minus the notional, before fees.
    pub gross_profit: i128,
    /// Net profit after the full cost model.
    pub net_profit: i128,
    /// Whether the engine actually submitted (vs simulated/skipped).
    pub submitted: bool,
}
