//! The single error type used across the deterministic core.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("invalid pool state: {0}")]
    InvalidPool(String),

    #[error("arithmetic overflow in {0}")]
    Overflow(&'static str),

    #[error("no arbitrage cycle found")]
    NoCycle,

    #[error("risk limit violated: {0}")]
    RiskViolation(String),
}
