//! DEX pool model and the boundary traits between raw on-chain bytes and the
//! deterministic engine.
//!
//! Status, stated plainly:
//! - [`CpmmPool`]/[`Quoter`] (constant-product) and [`clmm::ClmmPool`]
//!   (concentrated liquidity, single-tick) are implemented and tested.
//! - The `decoder` module turns raw account bytes into pool snapshots with real
//!   on-chain offsets: SPL token accounts, Raydium AMM v4 / CP-Swap (CPMM), and
//!   Orca Whirlpool (CLMM) are fixture-tested. Raydium CLMM and Meteora DLMM are
//!   honestly `NotImplemented`.
//! - [`Pool`] unifies the kinds so the streaming/detection pipeline is generic.

pub mod clmm;
pub mod decoder;

pub use clmm::ClmmPool;

use lorenz_amm::CpmmReserves;
use lorenz_core::types::{Bps, Dex, PoolId, TokenId};
use lorenz_graph::Edge;

/// Anything that can price a swap of a known size.
pub trait Quoter {
    /// Output amount for swapping `amount_in` of `token_in` into the other
    /// side of the pool. Returns `None` if `token_in` is not part of the pool.
    fn quote(&self, token_in: &TokenId, amount_in: u128) -> Option<u128>;

    fn dex(&self) -> Dex;
    fn pool_id(&self) -> &PoolId;
}

/// A two-sided constant-product pool snapshot. This is the concrete, working
/// pool type the engine reasons about; live decoders normalize into this (or a
/// future concentrated-liquidity variant).
#[derive(Debug, Clone, PartialEq)]
pub struct CpmmPool {
    pub id: PoolId,
    pub dex: Dex,
    pub token_a: TokenId,
    pub token_b: TokenId,
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub fee: Bps,
}

impl CpmmPool {
    /// Reserves oriented for a swap that *spends* `token_in`.
    fn reserves_for(&self, token_in: &TokenId) -> Option<CpmmReserves> {
        if token_in == &self.token_a {
            Some(CpmmReserves::new(self.reserve_a, self.reserve_b, self.fee))
        } else if token_in == &self.token_b {
            Some(CpmmReserves::new(self.reserve_b, self.reserve_a, self.fee))
        } else {
            None
        }
    }

    /// Produce both directed graph edges for a given probe size, so the
    /// detector can consider swapping either way through this pool.
    pub fn edges(&self, probe_size: u128) -> Vec<Edge> {
        let mut out = Vec::new();
        for (token_in, token_out) in [
            (&self.token_a, &self.token_b),
            (&self.token_b, &self.token_a),
        ] {
            if let Some(r) = self.reserves_for(token_in) {
                if let Some(rate) = r.effective_rate(probe_size) {
                    if rate > 0.0 {
                        out.push(Edge {
                            from: token_in.clone(),
                            to: token_out.clone(),
                            pool: self.id.clone(),
                            rate,
                        });
                    }
                }
            }
        }
        out
    }
}

impl Quoter for CpmmPool {
    fn quote(&self, token_in: &TokenId, amount_in: u128) -> Option<u128> {
        self.reserves_for(token_in)?.amount_out(amount_in)
    }

    fn dex(&self) -> Dex {
        self.dex
    }

    fn pool_id(&self) -> &PoolId {
        &self.id
    }
}

/// A pool of any supported kind. Lets the streaming and detection pipeline
/// treat constant-product and concentrated-liquidity pools uniformly.
#[derive(Debug, Clone, PartialEq)]
pub enum Pool {
    Cpmm(CpmmPool),
    Clmm(ClmmPool),
}

impl Pool {
    /// Directed graph edges for both swap directions at a probe size.
    pub fn edges(&self, probe_size: u128) -> Vec<Edge> {
        match self {
            Pool::Cpmm(p) => p.edges(probe_size),
            Pool::Clmm(p) => p.edges(probe_size),
        }
    }

    /// Output for swapping `amount_in` of `token_in`.
    pub fn quote(&self, token_in: &TokenId, amount_in: u128) -> Option<u128> {
        match self {
            Pool::Cpmm(p) => p.quote(token_in, amount_in),
            Pool::Clmm(p) => p.quote(token_in, amount_in),
        }
    }

    pub fn id(&self) -> &PoolId {
        match self {
            Pool::Cpmm(p) => &p.id,
            Pool::Clmm(p) => &p.id,
        }
    }

    pub fn dex(&self) -> Dex {
        match self {
            Pool::Cpmm(p) => p.dex,
            Pool::Clmm(p) => p.dex,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> CpmmPool {
        CpmmPool {
            id: "pool1".into(),
            dex: Dex::RaydiumAmm,
            token_a: "SOL".into(),
            token_b: "USDC".into(),
            reserve_a: 1_000_000_000,
            reserve_b: 1_000_000_000,
            fee: Bps(30),
        }
    }

    #[test]
    fn quotes_both_directions() {
        let p = pool();
        assert!(p.quote(&"SOL".into(), 1_000).unwrap() > 0);
        assert!(p.quote(&"USDC".into(), 1_000).unwrap() > 0);
    }

    #[test]
    fn unknown_token_has_no_quote() {
        let p = pool();
        assert_eq!(p.quote(&"BONK".into(), 1_000), None);
    }

    #[test]
    fn emits_two_edges() {
        let p = pool();
        let edges = p.edges(1_000);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].pool, "pool1".into());
    }

    #[test]
    fn pool_enum_unifies_cpmm_and_clmm() {
        let cpmm = Pool::Cpmm(pool());
        let clmm = Pool::Clmm(ClmmPool {
            id: "clmm1".into(),
            dex: Dex::Whirlpool,
            token_a: "SOL".into(),
            token_b: "USDC".into(),
            sqrt_price_x64: 1u128 << 64,
            liquidity: 1_000_000_000_000,
            fee: Bps(30),
        });
        assert_eq!(cpmm.edges(1_000).len(), 2);
        assert_eq!(clmm.edges(1_000).len(), 2);
        assert_eq!(cpmm.dex(), Dex::RaydiumAmm);
        assert_eq!(clmm.dex(), Dex::Whirlpool);
        assert!(clmm.quote(&"SOL".into(), 1_000).is_some());
    }
}
