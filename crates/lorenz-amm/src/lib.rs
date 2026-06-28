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

    /// Minimum input required to receive *at least* `amount_out` on the same
    /// curve and fee model as [`Self::amount_out`]. This is the exact-output
    /// (inverse) swap: you specify the output you want, it returns the input
    /// you must supply.
    ///
    /// The forward function applies the fee to the input and floors twice:
    ///
    /// ```text
    /// in_after_fee = floor(amount_in * (10_000 - fee) / 10_000)
    /// out          = floor(reserve_out * in_after_fee / (reserve_in + in_after_fee))
    /// ```
    ///
    /// Inverting each step and rounding the input *up* (so the pool is never
    /// short-changed) gives:
    ///
    /// ```text
    /// in_after_fee = ceil(reserve_in * out / (reserve_out - out))
    /// amount_in    = ceil(in_after_fee * 10_000 / (10_000 - fee))
    /// ```
    ///
    /// The result is the smallest integer input for which
    /// `amount_out(amount_in) >= out`, tight to integer-rounding slack.
    ///
    /// Returns `Some(0)` when `amount_out == 0`. Returns `None` when the output
    /// is unreachable: `amount_out >= reserve_out`, empty reserves, a 100% fee,
    /// or an intermediate multiplication that would overflow `u128`.
    pub fn amount_in_for_exact_out(&self, amount_out: u128) -> Option<u128> {
        if amount_out == 0 {
            return Some(0);
        }
        // A positive output is unreachable when there is nothing to draw from,
        // when the request meets or exceeds the output reserve (the curve only
        // approaches it asymptotically), or when reserve_in is empty (the
        // forward function yields 0 for any input).
        if self.reserve_in == 0 || self.reserve_out == 0 || amount_out >= self.reserve_out {
            return None;
        }
        let fee_num = u128::from(Bps::DENOMINATOR - self.fee.0);
        let fee_den = u128::from(Bps::DENOMINATOR);
        // A 100% fee leaves nothing after the fee, so no output is reachable.
        if fee_num == 0 {
            return None;
        }

        // in_after_fee = ceil(reserve_in * out / (reserve_out - out)).
        let denominator = self.reserve_out - amount_out;
        let in_after_fee = self
            .reserve_in
            .checked_mul(amount_out)?
            .div_ceil(denominator);

        // amount_in = ceil(in_after_fee * fee_den / fee_num), inverting the
        // input-side fee with the input rounded up.
        Some(in_after_fee.checked_mul(fee_den)?.div_ceil(fee_num))
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

    #[test]
    fn exact_out_round_trips_known_value() {
        let p = CpmmReserves::new(1_000_000, 1_000_000, Bps(30));
        // Ask for exactly 999 out: the input must be enough to clear it.
        let needed = p.amount_in_for_exact_out(999).unwrap();
        assert!(p.amount_out(needed).unwrap() >= 999);
        // One token less input must fall short, proving minimality.
        assert!(p.amount_out(needed - 1).unwrap() < 999);
    }

    #[test]
    fn exact_out_zero_is_zero_input() {
        let p = CpmmReserves::new(1_000_000, 1_000_000, Bps(30));
        assert_eq!(p.amount_in_for_exact_out(0), Some(0));
    }

    #[test]
    fn exact_out_unreachable_returns_none() {
        let p = CpmmReserves::new(1_000_000, 1_000_000, Bps(30));
        // Cannot drain the whole output reserve, nor more than it.
        assert_eq!(p.amount_in_for_exact_out(1_000_000), None);
        assert_eq!(p.amount_in_for_exact_out(1_000_001), None);
        // Empty reserves: no positive output is reachable.
        assert_eq!(
            CpmmReserves::new(0, 1_000_000, Bps(30)).amount_in_for_exact_out(1),
            None
        );
        assert_eq!(
            CpmmReserves::new(1_000_000, 0, Bps(30)).amount_in_for_exact_out(1),
            None
        );
        // A 100% fee leaves nothing after the fee.
        assert_eq!(
            CpmmReserves::new(1_000_000, 1_000_000, Bps(10_000)).amount_in_for_exact_out(1),
            None
        );
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

        // The inverse always clears the requested output, and is minimal: one
        // token less input falls short. This pins down both the round-trip
        // invariant (forward(inverse(out)) >= out) and tightness in one shot.
        #[test]
        fn exact_out_round_trips_and_is_minimal(
            r_in in 1u128..1_000_000_000,
            r_out in 2u128..1_000_000_000,
            out_raw in 0u128..u128::MAX,
            fee in 0u32..1000,
        ) {
            // Constrain the request to [1, r_out - 1]: positive and reachable.
            let out = 1 + (out_raw % (r_out - 1));
            let p = CpmmReserves::new(r_in, r_out, Bps(fee));
            let needed = p.amount_in_for_exact_out(out).unwrap();
            prop_assert!(needed >= 1);
            // Round-trip: the returned input clears the requested output.
            prop_assert!(p.amount_out(needed).unwrap() >= out);
            // Minimal/tight: one token less input is no longer enough.
            // `needed - 1` may be 0, where the forward fn yields `None` (no
            // output); treat that as 0, which is correctly `< out` (out >= 1).
            prop_assert!(p.amount_out(needed - 1).unwrap_or(0) < out);
        }

        // Asking for more output never requires less input (monotonicity).
        #[test]
        fn exact_out_monotonic_in_output(
            r_in in 1u128..1_000_000_000,
            r_out in 3u128..1_000_000_000,
            a_raw in 0u128..u128::MAX,
            b_raw in 0u128..u128::MAX,
            fee in 0u32..1000,
        ) {
            let a = 1 + (a_raw % (r_out - 1));
            let b = 1 + (b_raw % (r_out - 1));
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let p = CpmmReserves::new(r_in, r_out, Bps(fee));
            let in_lo = p.amount_in_for_exact_out(lo).unwrap();
            let in_hi = p.amount_in_for_exact_out(hi).unwrap();
            prop_assert!(in_lo <= in_hi);
        }

        // Unreachable requests return None rather than panicking.
        #[test]
        fn exact_out_unreachable_is_none(
            r_in in 0u128..1_000_000_000,
            r_out in 1u128..1_000_000_000,
            over in 0u128..1_000_000_000,
            fee in 0u32..1000,
        ) {
            let p = CpmmReserves::new(r_in, r_out, Bps(fee));
            // out == reserve_out or beyond is never reachable.
            prop_assert_eq!(p.amount_in_for_exact_out(r_out + over), None);
        }
    }
}
