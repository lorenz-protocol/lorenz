//! Concentrated-liquidity (CLMM) pricing, Uniswap-v3 / Orca-Whirlpool style.
//!
//! Unlike a constant-product pool, a CLMM pool stores a current `sqrt_price` and
//! an active `liquidity` (plus tick arrays for liquidity outside the current
//! range). Within the current tick range the swap math is exact and simple:
//!
//! Let `s = sqrt_price` (a real number) and `L = liquidity`. For a swap of `dx`
//! of token A (token0) in, the price moves down:
//!
//! ```text
//! 1/s' = 1/s + dx/L      out_B = L * (s - s')
//! ```
//!
//! For `dy` of token B (token1) in, the price moves up:
//!
//! ```text
//! s' = s + dy/L          out_A = L * (1/s - 1/s')
//! ```
//!
//! HONEST SCOPE: this implements the *single-tick* swap, i.e. it is accurate as
//! long as the trade does not cross into a neighbouring initialized tick (where
//! `L` changes). That is exactly the right tool for the detector, which only
//! needs effective rates to surface candidate cycles; the executor/backtester
//! recompute exact, tick-crossing, integer-precise output before acting. The
//! math here uses `f64`; full Q64.64 integer math with tick crossing is roadmap.

use lorenz_core::types::{Bps, Dex, PoolId, TokenId};
use lorenz_graph::Edge;

/// Q64.64 scale: `sqrt_price = sqrt_price_x64 / 2^64`.
const Q64: f64 = 18_446_744_073_709_551_616.0; // 2^64

/// A concentrated-liquidity pool snapshot at the current tick.
#[derive(Debug, Clone, PartialEq)]
pub struct ClmmPool {
    pub id: PoolId,
    pub dex: Dex,
    pub token_a: TokenId,
    pub token_b: TokenId,
    /// Current sqrt price in Q64.64 fixed point.
    pub sqrt_price_x64: u128,
    /// Active liquidity at the current tick.
    pub liquidity: u128,
    pub fee: Bps,
}

impl ClmmPool {
    fn sqrt_price(&self) -> f64 {
        self.sqrt_price_x64 as f64 / Q64
    }

    /// Output for swapping `amount_in` of `token_in`, within the current tick.
    /// Returns `None` if `token_in` is not part of the pool.
    pub fn quote(&self, token_in: &TokenId, amount_in: u128) -> Option<u128> {
        let s = self.sqrt_price();
        let l = self.liquidity as f64;
        if l <= 0.0 || s <= 0.0 || amount_in == 0 {
            return Some(0);
        }

        // Apply fee on the input.
        let dx = amount_in as f64 * (1.0 - self.fee.as_fraction());

        let out = if token_in == &self.token_a {
            // token0 in -> price down. 1/s' = 1/s + dx/L ; out_B = L*(s - s')
            let inv_s_new = 1.0 / s + dx / l;
            let s_new = 1.0 / inv_s_new;
            l * (s - s_new)
        } else if token_in == &self.token_b {
            // token1 in -> price up. s' = s + dy/L ; out_A = L*(1/s - 1/s')
            let s_new = s + dx / l;
            l * (1.0 / s - 1.0 / s_new)
        } else {
            return None;
        };

        if out.is_finite() && out > 0.0 {
            Some(out.floor() as u128)
        } else {
            Some(0)
        }
    }

    fn effective_rate(&self, token_in: &TokenId, amount_in: u128) -> Option<f64> {
        let out = self.quote(token_in, amount_in)?;
        if amount_in == 0 {
            return Some(0.0);
        }
        Some(out as f64 / amount_in as f64)
    }

    /// Directed graph edges for both swap directions at a probe size.
    pub fn edges(&self, probe_size: u128) -> Vec<Edge> {
        let mut out = Vec::new();
        for (token_in, token_out) in [
            (&self.token_a, &self.token_b),
            (&self.token_b, &self.token_a),
        ] {
            if let Some(rate) = self.effective_rate(token_in, probe_size) {
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
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a pool whose price is ~1.0 (sqrt_price = 1.0 in Q64.64).
    fn balanced_pool(liquidity: u128, fee: Bps) -> ClmmPool {
        ClmmPool {
            id: "clmm1".into(),
            dex: Dex::Whirlpool,
            token_a: "A".into(),
            token_b: "B".into(),
            sqrt_price_x64: Q64 as u128, // sqrt_price = 1.0 -> price = 1.0
            liquidity,
            fee,
        }
    }

    #[test]
    fn unknown_token_has_no_quote() {
        let p = balanced_pool(1_000_000_000, Bps(30));
        assert_eq!(p.quote(&"C".into(), 1000), None);
    }

    #[test]
    fn deep_liquidity_price_is_near_one() {
        // With huge liquidity relative to trade size and price 1.0, output is
        // close to input minus fee, and strictly below input (impact + fee).
        let p = balanced_pool(1_000_000_000_000_000, Bps(30));
        let out = p.quote(&"A".into(), 1_000_000).unwrap();
        assert!(out > 0);
        assert!(out < 1_000_000);
        assert!(out >= 995_000); // ~0.3% fee + tiny impact
    }

    #[test]
    fn more_input_more_output() {
        let p = balanced_pool(1_000_000_000_000, Bps(0));
        let a = p.quote(&"A".into(), 1_000).unwrap();
        let b = p.quote(&"A".into(), 2_000).unwrap();
        assert!(b >= a);
    }

    #[test]
    fn zero_liquidity_yields_zero() {
        let p = balanced_pool(0, Bps(30));
        assert_eq!(p.quote(&"A".into(), 1000), Some(0));
    }

    #[test]
    fn emits_two_edges() {
        let p = balanced_pool(1_000_000_000_000, Bps(30));
        assert_eq!(p.edges(10_000).len(), 2);
    }
}
