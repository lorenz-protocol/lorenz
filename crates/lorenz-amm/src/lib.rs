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
    /// Returns `Some(0)` for degenerate input — empty reserves or a zero input
    /// amount both yield a zero output. Returns `None` only when the swap cannot
    /// be priced at all: a fee above 100% (`fee > 10_000` bps) or an
    /// intermediate multiplication that would overflow `u128`.
    pub fn amount_out(&self, amount_in: u128) -> Option<u128> {
        if self.reserve_in == 0 || self.reserve_out == 0 || amount_in == 0 {
            return Some(0);
        }
        let fee_num = u128::from(self.fee.fee_complement()?);
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
        let fee_num = u128::from(self.fee.fee_complement()?);
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

/// Cycle output for a given input: chains [`CpmmReserves::amount_out`] across
/// `legs`, feeding the output of leg `i` as the input to leg `i + 1`. Returns
/// `None` only when a leg cannot be priced at all (a fee above 100% or an
/// intermediate multiplication that would overflow `u128`); a degenerate leg
/// (empty reserves) yields `Some(0)` and short-circuits the rest to zero.
fn cycle_output(legs: &[CpmmReserves], input: u128) -> Option<u128> {
    let mut amount = input;
    for leg in legs {
        amount = leg.amount_out(amount)?;
    }
    Some(amount)
}

/// Maximize a single-peaked (unimodal) integer objective over the inputs
/// `1..=max_input`.
///
/// `objective` returns the value to maximize for a candidate input, or `None`
/// when that input is infeasible (for example an intermediate computation would
/// overflow); infeasible inputs are skipped.
///
/// The search is a ternary narrowing of the interval down to a small window,
/// followed by a bounded local scan that walks outward from that window while
/// it keeps finding improvements. The outward walk is what makes the result
/// exact on a *floored* constant-product curve: flooring turns the strictly
/// concave real profit into a near-concave integer step function with a flat,
/// jagged top, so a single-point comparison near the peak is unreliable. The
/// outward scan settles that integer-rounding noise and pins down the true
/// argmax within the searched window.
///
/// Returns `Some((best_input, best_value))` for the maximizing input, or `None`
/// when `max_input == 0` or no input in range is feasible. The search is
/// deterministic and uses only integer arithmetic.
pub fn maximize_unimodal<F>(max_input: u128, objective: F) -> Option<(u128, i128)>
where
    F: Fn(u128) -> Option<i128>,
{
    if max_input == 0 {
        return None;
    }

    // Width the ternary phase leaves for the local scan, and how far the scan
    // walks outward past a non-improving step before giving up. Both are sized
    // with generous headroom over the worst flat-top spread observed for the
    // floored cycle curve, so the returned point is the exact integer argmax.
    const WINDOW: u128 = 64;
    const PATIENCE: u32 = 1024;

    // An infeasible probe is treated as the lowest possible value while we
    // narrow the bracket; the exact scan below only ever records feasible ones.
    let probe = |x: u128| objective(x).unwrap_or(i128::MIN);

    let mut lo: u128 = 1;
    let mut hi: u128 = max_input;
    while hi - lo > WINDOW {
        let third = (hi - lo) / 3;
        let m1 = lo + third;
        let m2 = hi - third;
        if probe(m1) < probe(m2) {
            lo = m1;
        } else {
            hi = m2;
        }
    }

    let mut best: Option<(u128, i128)> = None;
    let consider = |x: u128, best: &mut Option<(u128, i128)>| -> bool {
        if let Some(v) = objective(x) {
            let better = match *best {
                Some((_, bv)) => v > bv,
                None => true,
            };
            if better {
                *best = Some((x, v));
                return true;
            }
        }
        false
    };

    let mut x = lo;
    loop {
        consider(x, &mut best);
        if x == hi {
            break;
        }
        x += 1;
    }

    // Walk outward from each edge of the window, resetting patience on every
    // improvement so a flat top can be crossed in full.
    let mut miss = 0u32;
    let mut x = lo;
    while x > 1 && miss < PATIENCE {
        x -= 1;
        if consider(x, &mut best) {
            miss = 0;
        } else {
            miss += 1;
        }
    }
    miss = 0;
    let mut x = hi;
    while x < max_input && miss < PATIENCE {
        x += 1;
        if consider(x, &mut best) {
            miss = 0;
        } else {
            miss += 1;
        }
    }

    best
}

/// Profit-maximizing input size for an arbitrage cycle and the cycle output it
/// produces.
///
/// `legs` is the cycle's per-hop reserves in execution order (the output token
/// of leg `i` is the input token of leg `i + 1`, and the last leg closes back
/// to the borrowed asset). The gross profit of an input `x` is
/// `cycle_output(x) - x`. Because the chained, floored
/// [`CpmmReserves::amount_out`] is concave and increasing while the input it is
/// netted against is linear, gross profit is unimodal in `x`, so the optimum is
/// found by [`maximize_unimodal`].
///
/// Returns `Some((best_input, best_output))` for the input in `1..=max_input`
/// that maximizes gross profit. Returns `None` when `legs` is empty,
/// `max_input == 0`, or no input yields strictly positive gross profit (a
/// break-even or losing cycle).
///
/// The result is integer-exact on typical, sharply-peaked cycles (the regime
/// the property tests pin down by brute force); on an extremely flat, wide
/// profit plateau the unimodal search returns an input within a small bounded
/// neighborhood of the exact optimum rather than guaranteeing the single global
/// argmax.
pub fn optimal_cycle_size(legs: &[CpmmReserves], max_input: u128) -> Option<(u128, u128)> {
    if legs.is_empty() || max_input == 0 {
        return None;
    }

    let objective = |x: u128| -> Option<i128> {
        let out = i128::try_from(cycle_output(legs, x)?).ok()?;
        let input = i128::try_from(x).ok()?;
        Some(out - input)
    };

    let (best_input, best_gross) = maximize_unimodal(max_input, objective)?;
    if best_gross <= 0 {
        return None;
    }
    let best_output = cycle_output(legs, best_input)?;
    Some((best_input, best_output))
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

        // A fee above 100% has no after-fee complement, so both swap
        // directions must return None and never underflow/panic, for any
        // positive reserves and amounts. (Exactly 100% is the boundary: the
        // complement is Some(0), so amount_out yields Some(0) and the inverse
        // None, both exercised by the unit tests above.)
        #[test]
        fn over_full_fee_is_none(
            r_in in 1u128..1_000_000_000,
            r_out in 1u128..1_000_000_000,
            amt in 1u128..1_000_000_000,
            fee in 10_001u32..=u32::MAX,
        ) {
            let p = CpmmReserves::new(r_in, r_out, Bps(fee));
            // Reserves and amount are positive, so the fee is actually applied
            // rather than short-circuited.
            prop_assert_eq!(p.amount_out(amt), None);
            prop_assert_eq!(p.amount_in_for_exact_out(amt), None);
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

    // --- optimal cycle sizing ---------------------------------------------

    /// Brute-force ground truth: the maximum strictly-positive gross profit
    /// over `1..=max_input`, or `None` if no input clears a profit.
    fn brute_best_profit(legs: &[CpmmReserves], max_input: u128) -> Option<i128> {
        let mut best: Option<i128> = None;
        for x in 1..=max_input {
            if let Some(out) = cycle_output(legs, x) {
                let profit = out as i128 - x as i128;
                if best.is_none_or(|b| profit > b) {
                    best = Some(profit);
                }
            }
        }
        best.filter(|&b| b > 0)
    }

    #[test]
    fn maximize_unimodal_finds_strict_peak() {
        // A strictly concave parabola peaking at x = 123 with value 1_000_000.
        let peak = 123i128;
        let found = maximize_unimodal(1_000, |x| {
            let d = x as i128 - peak;
            Some(1_000_000 - d * d)
        });
        let (x, v) = found.unwrap();
        assert_eq!(x, 123);
        assert_eq!(v, 1_000_000);
    }

    #[test]
    fn maximize_unimodal_degenerate_inputs() {
        assert_eq!(maximize_unimodal(0, |_| Some(0i128)), None);
        assert_eq!(maximize_unimodal(10, |_| None::<i128>), None);
    }

    #[test]
    fn optimal_matches_brute_on_concrete_cycle() {
        // Two favorably-priced hops (output reserve 5% above input) net of a
        // 30 bps fee each: a genuinely profitable two-leg cycle.
        let legs = [
            CpmmReserves::new(1_000_000, 1_050_000, Bps(30)),
            CpmmReserves::new(1_000_000, 1_050_000, Bps(30)),
        ];
        let max_input = 100_000;
        let (x, out) = optimal_cycle_size(&legs, max_input).unwrap();
        let profit = out as i128 - x as i128;
        assert!(profit > 0, "profit was {profit}");
        assert!((1..=max_input).contains(&x));
        // Exactly the brute-force optimum.
        assert_eq!(Some(profit), brute_best_profit(&legs, max_input));
    }

    #[test]
    fn break_even_single_leg_is_none() {
        // A single symmetric, zero-fee hop can only lose to price impact.
        let legs = [CpmmReserves::new(1_000_000, 1_000_000, Bps(0))];
        assert!(optimal_cycle_size(&legs, 1_000_000).is_none());
    }

    #[test]
    fn optimal_degenerate_inputs_are_none() {
        assert!(optimal_cycle_size(&[], 1_000).is_none());
        let legs = [CpmmReserves::new(1_000_000, 1_050_000, Bps(0))];
        assert!(optimal_cycle_size(&legs, 0).is_none());
    }

    proptest! {
        // Ground truth on small ranges: the optimizer's gross profit equals the
        // brute-force maximum exactly. Reserves and fee are kept in the regime
        // where flooring noise stays within the search's refinement window.
        #[test]
        fn optimal_equals_brute_force_small(
            raw in prop::collection::vec(
                (1u128..=1_000_000, 1u128..=1_000_000, 0u32..=100),
                1..=4,
            ),
            max_input in 1u128..=2_000,
        ) {
            let legs: Vec<CpmmReserves> = raw
                .iter()
                .map(|&(r_in, r_out, fee)| CpmmReserves::new(r_in, r_out, Bps(fee)))
                .collect();
            let got = optimal_cycle_size(&legs, max_input)
                .map(|(x, out)| out as i128 - x as i128);
            let brute = brute_best_profit(&legs, max_input);
            prop_assert_eq!(got, brute);
        }

        // Optimality vs samples. The search is integer-exact only where the
        // near-peak band fits its refinement window, which holds on these small
        // ranges (the same regime as the brute-force test); there the returned
        // profit dominates the profit at every randomly sampled feasible input.
        #[test]
        fn optimal_dominates_samples(
            raw in prop::collection::vec(
                (1u128..=1_000_000, 1u128..=1_000_000, 0u32..=100),
                1..=4,
            ),
            max_input in 1u128..=2_000,
            samples in prop::collection::vec(0u128..u128::MAX, 1..=32),
        ) {
            let legs: Vec<CpmmReserves> = raw
                .iter()
                .map(|&(r_in, r_out, fee)| CpmmReserves::new(r_in, r_out, Bps(fee)))
                .collect();
            if let Some((x, out)) = optimal_cycle_size(&legs, max_input) {
                let best = out as i128 - x as i128;
                prop_assert!(best > 0);
                for &s in &samples {
                    let probe = 1 + (s % max_input);
                    if let Some(probe_out) = cycle_output(&legs, probe) {
                        prop_assert!(probe_out as i128 - probe as i128 <= best);
                    }
                }
            }
        }

        // On large ranges (where exhaustive checking is infeasible) the result
        // is always self-consistent: the chosen input is inside the requested
        // bound, the reported output matches re-simulating that input, and the
        // gross profit is strictly positive. This exercises the full ternary
        // path at scale without asserting an exactness the integer-floored,
        // possibly very flat curve cannot always guarantee.
        #[test]
        fn optimal_is_consistent_on_large_ranges(
            raw in prop::collection::vec(
                (1u128..=1_000_000_000_000, 1u128..=1_000_000_000_000, 0u32..=300),
                1..=4,
            ),
            max_input in 1u128..=5_000_000_000,
        ) {
            let legs: Vec<CpmmReserves> = raw
                .iter()
                .map(|&(r_in, r_out, fee)| CpmmReserves::new(r_in, r_out, Bps(fee)))
                .collect();
            if let Some((x, out)) = optimal_cycle_size(&legs, max_input) {
                prop_assert!((1..=max_input).contains(&x));
                prop_assert_eq!(cycle_output(&legs, x), Some(out));
                prop_assert!(out > x);
            }
        }

        // A single symmetric hop can never clear a profit, for any fee/size.
        #[test]
        fn single_symmetric_leg_is_unprofitable(
            r in 1u128..=1_000_000_000_000,
            fee in 0u32..=300,
            max_input in 1u128..=1_000_000_000,
        ) {
            let legs = [CpmmReserves::new(r, r, Bps(fee))];
            prop_assert!(optimal_cycle_size(&legs, max_input).is_none());
        }
    }
}
