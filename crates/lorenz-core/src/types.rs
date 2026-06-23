//! Domain newtypes.
//!
//! We use newtypes instead of bare `u64`/`String` so the compiler stops us from
//! mixing, say, a token amount with a basis-point value. Every quantity that
//! flows through the engine has a name and a unit.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifier of a token (mint). Kept as an opaque string so this crate does
/// not need to depend on the Solana SDK; the data-plane crates parse it into a
/// real `Pubkey` at the edge.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenId(pub String);

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for TokenId {
    fn from(s: &str) -> Self {
        TokenId(s.to_string())
    }
}

/// Identifier of a liquidity pool / pair account.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PoolId(pub String);

impl fmt::Display for PoolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for PoolId {
    fn from(s: &str) -> Self {
        PoolId(s.to_string())
    }
}

/// A raw token amount expressed in the token's base units (lamports for SOL,
/// smallest unit for SPL tokens). Always an integer: we never carry floating
/// point money through the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct Amount(pub u64);

impl Amount {
    pub const ZERO: Amount = Amount(0);

    #[inline]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Basis points (1 bp = 0.01%). Used for fees, thresholds and slippage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct Bps(pub u32);

impl Bps {
    pub const DENOMINATOR: u32 = 10_000;

    /// Fraction in `[0, 1]` as `f64`. Convenience for analytics and logging,
    /// never for settlement math (which stays integer).
    #[inline]
    pub fn as_fraction(self) -> f64 {
        f64::from(self.0) / f64::from(Self::DENOMINATOR)
    }
}

impl fmt::Display for Bps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}bps", self.0)
    }
}

/// Which DEX a pool belongs to. The list mirrors the integrations that the
/// data plane targets; see `lorenz-dex` for the decoder status of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dex {
    RaydiumAmm,
    RaydiumClmm,
    RaydiumCpmm,
    MeteoraDlmm,
    MeteoraDamm,
    Whirlpool,
    PumpAmm,
    Solfi,
    Vertigo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bps_fraction() {
        assert_eq!(Bps(10_000).as_fraction(), 1.0);
        assert_eq!(Bps(25).as_fraction(), 0.0025);
        assert_eq!(Bps(0).as_fraction(), 0.0);
    }

    #[test]
    fn amount_ordering_is_numeric() {
        assert!(Amount(10) < Amount(11));
        assert_eq!(Amount::ZERO, Amount(0));
    }
}
