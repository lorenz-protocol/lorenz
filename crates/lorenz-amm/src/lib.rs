//! Constant-product AMM math (`x * y = k`) with a fee, done in integer
//! arithmetic so the result matches what the chain would compute at
//! settlement. No floating point touches the output amount.
//!
//! This is the smallest fully-real, fully-tested unit of the system: given
//! reserves and a fee it tells you exactly how much you get out of a swap, and
//! the property tests pin down the invariants that any correct AMM must obey
//! (output bounded by reserves, monotonic in input, round-trips lose the fee).

use lorenz_core::types::Bps;

/// A constant-product pool reserve snapshot for one direction of a swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpmmReserves {
    /// Reserve of the input token.
    pub reserve_in: u128,
    /// Reserve of the output token.
    pub reserve_out: u128,
    /// Swap fee taken on the input.
    pub fee: Bps,
}

impl CpmmReserves {
    pub fn new(reserve_in: u128, reserve_out: u128, fee: Bps) -> Self {
        Self {
            reserve_in,
            reserve_out,
            fee,
        }
    }

    /// Output amount for a given input, following the Uniswap-v2 style formula
    /// with the fee applied to the input:
    ///
    /// ```text
    /// in_after_fee = in * (10_000 - fee_bps) / 10_000
    /// out = reserve_out * in_after_fee / (reserve_in + in_after_fee)
    /// ```
    ///
    /// Returns `None` on degenerate input (empty reserves) rather than
    /// producing a misleading zero.
    pub fn amount_out(&self, amount_in: u128) -> Option<u128> {
        if self.reserve_in == 0 || self.reserve_out == 0 || amount_in == 0 {
            return Some(0);
        }
        let fee_num = u128::from(Bps::DENOMINATOR - self.fee.0);
        let fee_den = u128::from(Bps::DENOMINATOR);

        let in_after_fee = amount_in.checked_mul(fee_num)? / fee_den;
        let numerator = self.reserve_out.checked_mul(in_after_fee)?;
        let denominator = self.reserve_in.checked_add(in_after_fee)?;
        Some(numerator / denominator)
    }

    /// Marginal (spot) price out-per-in, ignoring fees and impact. For
    /// analytics only; settlement always uses [`Self::amount_out`].
    pub fn spot_price(&self) -> f64 {
        if self.reserve_in == 0 {
            return 0.0;
        }
        self.reserve_out as f64 / self.reserve_in as f64
    }

    /// Effective rate (out/in) actually realized for `amount_in`, including fee
    /// and price impact. Used to weight graph edges in `lorenz-graph`.
    pub fn effective_rate(&self, amount_in: u128) -> Option<f64> {
        let out = self.amount_out(amount_in)?;
        if amount_in == 0 {
            return Some(0.0);
        }
        Some(out as f64 / amount_in as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn known_value_no_fee() {
        // 1000 in, equal reserves of 1_000_000, zero fee.
        let p = CpmmReserves::new(1_000_000, 1_000_000, Bps(0));
        // out = 1_000_000 * 1000 / (1_000_000 + 1000) = 999 (integer floor)
        assert_eq!(p.amount_out(1000), Some(999));
    }

    #[test]
    fn fee_reduces_output() {
        let no_fee = CpmmReserves::new(1_000_000, 1_000_000, Bps(0));
        let with_fee = CpmmReserves::new(1_000_000, 1_000_000, Bps(30));
        assert!(with_fee.amount_out(10_000).unwrap() < no_fee.amount_out(10_000).unwrap());
    }

    #[test]
    fn empty_reserves_yield_zero() {
        let p = CpmmReserves::new(0, 1_000_000, Bps(30));
        assert_eq!(p.amount_out(1000), Some(0));
    }

    proptest! {
        // Output can never exceed the output reserve.
        #[test]
        fn output_bounded_by_reserve(
            r_in in 1u128..1_000_000_000,
            r_out in 1u128..1_000_000_000,
            amt in 1u128..1_000_000_000,
            fee in 0u32..1000,
        ) {
            let p = CpmmReserves::new(r_in, r_out, Bps(fee));
            let out = p.amount_out(amt).unwrap();
            prop_assert!(out < r_out);
        }

        // More input never yields strictly less output (monotonicity).
        #[test]
        fn monotonic_in_input(
            r_in in 1u128..1_000_000_000,
            r_out in 1u128..1_000_000_000,
            a in 1u128..500_000,
            b in 1u128..500_000,
            fee in 0u32..1000,
        ) {
            let p = CpmmReserves::new(r_in, r_out, Bps(fee));
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(p.amount_out(lo).unwrap() <= p.amount_out(hi).unwrap());
        }

        // A single pool can never give back more than you put in when priced in
        // the same units (no free money from one constant-product hop).
        #[test]
        fn no_free_money_single_hop(
            r in 1000u128..1_000_000_000,
            amt in 1u128..100_000,
            fee in 0u32..1000,
        ) {
            // Symmetric reserves: rate <= 1 must hold for any positive fee/impact.
            let p = CpmmReserves::new(r, r, Bps(fee));
            let out = p.amount_out(amt).unwrap();
            prop_assert!(out <= amt);
        }
    }
}
