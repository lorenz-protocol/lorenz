//! Deterministic backtest / replay harness.
//!
//! Feed it a sequence of market snapshots (pool reserves at points in time) and
//! it does, for each snapshot, exactly what the live engine would do in its
//! analysis step:
//!   1. build the arbitrage graph,
//!   2. find a candidate cycle (`lorenz-graph`),
//!   3. size it and recompute the *exact* output hop-by-hop (`lorenz-amm`),
//!   4. subtract the full cost model,
//!   5. emit an auditable [`TradeRecord`].
//!
//! The cost model is shared with the live config (`lorenz_core::config`) on
//! purpose: simulated economics use the same numbers as production accounting,
//! so a backtest can't quietly be more optimistic than reality.

use lorenz_amm::maximize_unimodal;
use lorenz_core::config::{CostConfig, RiskConfig};
use lorenz_core::telemetry::{Hop, TradeRecord};
use lorenz_core::types::{Amount, Bps, Dex, PoolId, TokenId};
use lorenz_dex::{CpmmPool, Quoter};
use lorenz_graph::{ArbitrageGraph, Cycle};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Serializable pool snapshot (the on-disk / on-wire form).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSnapshot {
    pub id: String,
    pub dex: Dex,
    pub token_a: String,
    pub token_b: String,
    pub reserve_a: u128,
    pub reserve_b: u128,
    pub fee_bps: u32,
}

impl PoolSnapshot {
    pub fn to_pool(&self) -> CpmmPool {
        CpmmPool {
            id: PoolId(self.id.clone()),
            dex: self.dex,
            token_a: TokenId(self.token_a.clone()),
            token_b: TokenId(self.token_b.clone()),
            reserve_a: self.reserve_a,
            reserve_b: self.reserve_b,
            fee: Bps(self.fee_bps),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSnapshot {
    pub ts: u64,
    pub pools: Vec<PoolSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    /// Token used to denominate borrowing and profit (e.g. the loaned asset).
    pub base_token: String,
    /// Notional borrowed per attempt, in base-token base units.
    pub notional: u64,
    pub snapshots: Vec<MarketSnapshot>,
}

/// The cost model, derived from the shared [`CostConfig`].
#[derive(Debug, Clone)]
pub struct CostModel {
    cfg: CostConfig,
}

impl CostModel {
    pub fn new(cfg: CostConfig) -> Self {
        Self { cfg }
    }

    /// Total cost (in base units) of executing a cycle of `hops` hops for a
    /// given `notional`. Includes flash-loan fee, fixed network fees and an
    /// assumed slippage buffer per hop.
    pub fn total_cost(&self, notional: u128, hops: usize) -> u128 {
        let flash = notional * u128::from(self.cfg.flash_loan_fee.0) / u128::from(Bps::DENOMINATOR);
        let slippage = notional * u128::from(self.cfg.slippage_per_hop.0) * (hops as u128)
            / u128::from(Bps::DENOMINATOR);
        let fixed =
            u128::from(self.cfg.priority_fee_lamports) + u128::from(self.cfg.jito_tip_lamports);
        flash + slippage + fixed
    }
}

/// Aggregated result of a backtest run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BacktestReport {
    pub records: Vec<TradeRecord>,
    pub candidates_found: usize,
    pub profitable_after_costs: usize,
    /// Sum of `net_profit` over the *submitted* records (the money path).
    pub total_net_profit: i128,
    /// Sum of `gross_profit` over *all* records (submitted or not).
    pub total_gross_profit: i128,
    /// Highest `net_profit` across all records, or `0` when there are none.
    pub best_net_profit: i128,
    /// Lowest `net_profit` across all records, or `0` when there are none.
    pub worst_net_profit: i128,
    /// Count of records grouped by route hop-count. A `BTreeMap` is used so the
    /// serialized form is deterministic (keys emitted in ascending order).
    pub hop_histogram: BTreeMap<usize, usize>,
}

impl BacktestReport {
    /// Fraction of candidate cycles that were actually submitted, in `0.0..=1.0`
    /// (`0.0` when no candidates were found). This is a reporting metric only,
    /// never part of the money path, so `f64` is acceptable here.
    pub fn submission_rate(&self) -> f64 {
        if self.candidates_found == 0 {
            return 0.0;
        }
        self.profitable_after_costs as f64 / self.candidates_found as f64
    }
}

/// Walk a detected cycle hop-by-hop using exact pool math, returning the route
/// and the final output for a given input notional.
fn simulate_cycle(
    pools: &HashMap<PoolId, CpmmPool>,
    cycle: &Cycle,
    base: &TokenId,
    notional: u128,
) -> Option<(Vec<Hop>, u128)> {
    // Rotate the cycle so it starts (and ends) at the base token, otherwise we
    // can't denominate profit in the loaned asset.
    let start = cycle.edges.iter().position(|e| &e.from == base)?;
    let n = cycle.edges.len();

    let mut hops = Vec::with_capacity(n);
    let mut amount = notional;
    for k in 0..n {
        let e = &cycle.edges[(start + k) % n];
        let pool = pools.get(&e.pool)?;
        let out = pool.quote(&e.from, amount)?;
        if out == 0 {
            return None;
        }
        hops.push(Hop {
            pool: e.pool.clone(),
            token_in: e.from.clone(),
            token_out: e.to.clone(),
            amount_in: Amount(amount.min(u128::from(u64::MAX)) as u64),
            amount_out: Amount(out.min(u128::from(u64::MAX)) as u64),
        });
        amount = out;
    }
    Some((hops, amount))
}

/// Upper bound on edge-disjoint opportunities analyzed per snapshot.
const MAX_CYCLES_PER_SNAPSHOT: usize = 8;

/// Run the backtest over the whole market.
pub fn run_backtest(market: &Market, risk: &RiskConfig, costs: &CostConfig) -> BacktestReport {
    let base = TokenId(market.base_token.clone());
    let notional = u128::from(market.notional.min(risk.max_position));
    let cost_model = CostModel::new(costs.clone());
    let mut report = BacktestReport::default();

    for snap in &market.snapshots {
        let mut graph = ArbitrageGraph::new();
        let mut pools: HashMap<PoolId, CpmmPool> = HashMap::new();
        for ps in &snap.pools {
            let pool = ps.to_pool();
            for edge in pool.edges(notional) {
                graph.add_edge(edge);
            }
            pools.insert(pool.id.clone(), pool);
        }

        // Detect several pool-disjoint opportunities per snapshot instead of a
        // single one; each is sized and priced independently below, exactly as
        // the single cycle used to be. Pool-disjoint (no shared pool between
        // cycles) mirrors the live engine: two cycles that touch the same pool
        // can't both execute against the same reserves in one bundle.
        let cycles = graph.find_pool_disjoint_arbitrage(MAX_CYCLES_PER_SNAPSHOT);
        for cycle in &cycles {
            report.candidates_found += 1;

            // Size the trade at the input that maximizes *net* profit, capped
            // by the risk ceiling. The cycle output is concave-increasing in
            // the input while the input and the (linear) cost subtracted from
            // it are linear, so net profit is unimodal and a ternary search
            // finds the optimum. `simulate_cycle` is the per-size oracle; the
            // hop count (and therefore the cost shape) is fixed for the cycle,
            // so it is read once.
            let hops = cycle.edges.len();
            let max_input = u128::from(risk.max_position);
            let net_at = |input: u128| -> Option<i128> {
                let (_, out) = simulate_cycle(&pools, cycle, &base, input)?;
                let out = i128::try_from(out).ok()?;
                let input_i = i128::try_from(input).ok()?;
                let cost = i128::try_from(cost_model.total_cost(input, hops)).ok()?;
                Some(out - input_i - cost)
            };
            let Some((notional, _)) = maximize_unimodal(max_input, net_at) else {
                continue;
            };

            let Some((route, final_out)) = simulate_cycle(&pools, cycle, &base, notional) else {
                continue;
            };

            let gross_profit = final_out as i128 - notional as i128;
            let cost = cost_model.total_cost(notional, route.len()) as i128;
            let net_profit = gross_profit - cost;
            let submitted = net_profit >= i128::from(risk.min_profit);

            if submitted {
                report.profitable_after_costs += 1;
                report.total_net_profit += net_profit;
            }

            // Aggregate analytics over *all* records (submitted or not).
            report.total_gross_profit += gross_profit;
            if report.records.is_empty() {
                report.best_net_profit = net_profit;
                report.worst_net_profit = net_profit;
            } else {
                report.best_net_profit = report.best_net_profit.max(net_profit);
                report.worst_net_profit = report.worst_net_profit.min(net_profit);
            }
            *report.hop_histogram.entry(route.len()).or_insert(0) += 1;

            report.records.push(TradeRecord {
                ts: snap.ts,
                route,
                notional: Amount(notional.min(u128::from(u64::MAX)) as u64),
                gross_profit,
                net_profit,
                submitted,
            });
        }
    }

    report
}

/// Select the indices of the `n` records with the highest `net_profit`, most
/// profitable first, breaking ties by original position (stable). Returns at
/// most `min(n, records.len())` indices; an empty slice or `n == 0` yields an
/// empty vector. Kept as a pure, index-returning helper so it is trivially
/// testable and so callers can render the originals without cloning them.
pub fn top_by_net_profit(records: &[TradeRecord], n: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..records.len()).collect();
    // Sort by net_profit descending; `sort_by` is stable, so equal-profit
    // records keep their original relative order (the tie-break).
    indices.sort_by(|&a, &b| records[b].net_profit.cmp(&records[a].net_profit));
    indices.truncate(n);
    indices
}

/// Parse a market from JSON.
pub fn parse_market(json: &str) -> Result<Market, serde_json::Error> {
    serde_json::from_str(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn costs() -> CostConfig {
        CostConfig {
            flash_loan_fee: Bps(9),
            priority_fee_lamports: 5_000,
            jito_tip_lamports: 1_000,
            slippage_per_hop: Bps(1),
        }
    }

    fn risk() -> RiskConfig {
        RiskConfig {
            max_position: 10_000_000_000,
            min_profit: 1,
            max_consecutive_losses: 5,
        }
    }

    /// Net profit at a fixed input `size` for the cycle detected in the first
    /// snapshot, mirroring exactly how `run_backtest` builds the graph and the
    /// cost model. Used to pin the "optimal is never worse than fixed-size"
    /// invariant without hardcoding numbers we cannot recompute by hand.
    fn first_cycle_net(
        market: &Market,
        risk: &RiskConfig,
        costs: &CostConfig,
        size: u128,
    ) -> Option<i128> {
        let base = TokenId(market.base_token.clone());
        let probe = u128::from(market.notional.min(risk.max_position));
        let snap = market.snapshots.first()?;
        let mut graph = ArbitrageGraph::new();
        let mut pools: HashMap<PoolId, CpmmPool> = HashMap::new();
        for ps in &snap.pools {
            let pool = ps.to_pool();
            for edge in pool.edges(probe) {
                graph.add_edge(edge);
            }
            pools.insert(pool.id.clone(), pool);
        }
        let cycle = graph.find_arbitrage()?;
        let (route, out) = simulate_cycle(&pools, &cycle, &base, size)?;
        let cm = CostModel::new(costs.clone());
        Some(out as i128 - size as i128 - cm.total_cost(size, route.len()) as i128)
    }

    #[test]
    fn cost_model_scales_with_hops() {
        let m = CostModel::new(costs());
        let c2 = m.total_cost(1_000_000, 2);
        let c3 = m.total_cost(1_000_000, 3);
        assert!(c3 > c2);
    }

    #[test]
    fn detects_and_prices_a_real_triangular_arb() {
        // Construct a market with a genuine cyclic mispricing in SOL terms.
        // SOL->USDC->BONK->SOL where the product of rates exceeds 1.
        let market = Market {
            base_token: "SOL".to_string(),
            notional: 1_000_000,
            snapshots: vec![MarketSnapshot {
                ts: 1,
                pools: vec![
                    PoolSnapshot {
                        id: "sol_usdc".into(),
                        dex: Dex::RaydiumAmm,
                        token_a: "SOL".into(),
                        token_b: "USDC".into(),
                        reserve_a: 1_000_000_000_000,
                        reserve_b: 1_000_000_000_000,
                        fee_bps: 1,
                    },
                    PoolSnapshot {
                        id: "usdc_bonk".into(),
                        dex: Dex::MeteoraDlmm,
                        token_a: "USDC".into(),
                        token_b: "BONK".into(),
                        reserve_a: 1_000_000_000_000,
                        reserve_b: 1_000_000_000_000,
                        fee_bps: 1,
                    },
                    // This pool is mispriced: lots more SOL per BONK than the
                    // implied cross-rate, creating the cycle.
                    PoolSnapshot {
                        id: "bonk_sol".into(),
                        dex: Dex::Whirlpool,
                        token_a: "BONK".into(),
                        token_b: "SOL".into(),
                        reserve_a: 1_000_000_000_000,
                        reserve_b: 1_050_000_000_000,
                        fee_bps: 1,
                    },
                ],
            }],
        };

        let report = run_backtest(&market, &risk(), &costs());
        // At least one disjoint opportunity is detected and recorded. (The
        // exact count is search-dependent, so we assert invariants, not a
        // hardcoded number.)
        assert!(report.candidates_found >= 1);
        assert!(!report.records.is_empty());

        // Every record is well-formed: it has a route and is sized within the
        // risk ceiling as a real, positive notional (never the old fixed
        // `min(notional, max_position)`).
        let max_pos = u128::from(risk().max_position);
        for rec in &report.records {
            assert!(!rec.route.is_empty());
            let chosen = u128::from(rec.notional.0);
            assert!((1..=max_pos).contains(&chosen));
        }

        // A genuine SOL -> ... -> SOL round-trip is submitted with positive
        // gross profit on this constructed mispricing.
        let sol: TokenId = "SOL".into();
        let winner = report
            .records
            .iter()
            .find(|r| {
                r.submitted
                    && r.route.first().is_some_and(|h| h.token_in == sol)
                    && r.route.last().is_some_and(|h| h.token_out == sol)
            })
            .expect("a profitable SOL round-trip is submitted");
        assert!(winner.gross_profit > 0, "gross was {}", winner.gross_profit);

        // Optimality invariant: the first detected cycle (identical to
        // `find_arbitrage`) is sized no worse than at the old fixed size.
        // (Exact numbers now depend on the search and are verified by the
        // property tests in `lorenz-amm`.)
        let fixed = u128::from(market.notional.min(risk().max_position));
        let fixed_net =
            first_cycle_net(&market, &risk(), &costs(), fixed).expect("fixed-size cycle simulates");
        assert!(
            report.records[0].net_profit >= fixed_net,
            "optimal net {} < fixed net {fixed_net}",
            report.records[0].net_profit
        );
    }

    #[test]
    fn report_serializes_and_round_trips_over_bundled_sample() {
        // The same embedded market the `lorenz-backtest` binary replays, so the
        // JSON mode is exercised against real computed values, not a fixture.
        const SAMPLE_MARKET: &str = include_str!("../data/sample_market.json");

        let market = parse_market(SAMPLE_MARKET).expect("bundled sample market is valid JSON");
        let report = run_backtest(&market, &risk(), &costs());

        // Structural invariants that hold regardless of the (search-dependent)
        // exact figures: every recorded trade is sized within `1..=max_position`,
        // and the submitted set is exactly the profitable-after-costs count.
        let max_pos = u128::from(risk().max_position);
        let mut submitted: usize = 0;
        for rec in &report.records {
            let chosen = u128::from(rec.notional.0);
            assert!((1..=max_pos).contains(&chosen));
            if rec.submitted {
                assert!(rec.net_profit >= i128::from(risk().min_profit));
                submitted += 1;
            }
        }
        assert_eq!(submitted, report.profitable_after_costs);

        // Serializes without error...
        let json = serde_json::to_string_pretty(&report).expect("report serializes to JSON");
        // ...and round-trips back into an equivalent value.
        let parsed: BacktestReport =
            serde_json::from_str(&json).expect("report round-trips from JSON");

        assert_eq!(parsed.candidates_found, report.candidates_found);
        assert_eq!(parsed.profitable_after_costs, report.profitable_after_costs);
        assert_eq!(parsed.total_net_profit, report.total_net_profit);
        assert_eq!(parsed.total_gross_profit, report.total_gross_profit);
        assert_eq!(parsed.best_net_profit, report.best_net_profit);
        assert_eq!(parsed.worst_net_profit, report.worst_net_profit);
        assert_eq!(parsed.hop_histogram, report.hop_histogram);
        assert_eq!(parsed.records, report.records);
    }

    /// A report with at least one record, replayed from the bundled sample so
    /// the aggregate analytics are exercised against real computed figures.
    fn sample_report() -> BacktestReport {
        const SAMPLE_MARKET: &str = include_str!("../data/sample_market.json");
        let market = parse_market(SAMPLE_MARKET).expect("bundled sample market is valid JSON");
        run_backtest(&market, &risk(), &costs())
    }

    #[test]
    fn best_is_never_below_worst() {
        let report = sample_report();
        assert!(report.best_net_profit >= report.worst_net_profit);
    }

    #[test]
    fn hop_histogram_counts_every_record() {
        let report = sample_report();
        let total: usize = report.hop_histogram.values().sum();
        assert_eq!(total, report.records.len());
    }

    #[test]
    fn total_gross_matches_record_sum() {
        let report = sample_report();
        let sum: i128 = report.records.iter().map(|r| r.gross_profit).sum();
        assert_eq!(report.total_gross_profit, sum);
    }

    #[test]
    fn submission_rate_is_a_valid_fraction() {
        let report = sample_report();
        let rate = report.submission_rate();
        assert!((0.0..=1.0).contains(&rate));
        if report.candidates_found == 0 {
            assert_eq!(rate, 0.0);
        } else {
            let expected = report.profitable_after_costs as f64 / report.candidates_found as f64;
            assert_eq!(rate, expected);
        }
    }

    #[test]
    fn best_and_worst_bracket_every_record() {
        let report = sample_report();
        for rec in &report.records {
            assert!(rec.net_profit <= report.best_net_profit);
            assert!(rec.net_profit >= report.worst_net_profit);
        }
    }

    #[test]
    fn top_by_net_profit_picks_the_highest_stably() {
        // net profits: [5, 1, 5, 9, 1]. Descending with stable tie-break by
        // original index: 9 (idx 3), 5 (idx 0), 5 (idx 2), 1 (idx 1), 1 (idx 4).
        let nets = [5_i128, 1, 5, 9, 1];
        let records: Vec<TradeRecord> = nets
            .iter()
            .map(|&net| TradeRecord {
                ts: 0,
                route: Vec::new(),
                notional: Amount(0),
                gross_profit: net,
                net_profit: net,
                submitted: false,
            })
            .collect();

        assert_eq!(top_by_net_profit(&records, 3), vec![3, 0, 2]);
        // n larger than the slice returns all indices (still ordered).
        assert_eq!(top_by_net_profit(&records, 99), vec![3, 0, 2, 1, 4]);
        // n == 0 and empty input both yield nothing.
        assert!(top_by_net_profit(&records, 0).is_empty());
        assert!(top_by_net_profit(&[], 3).is_empty());
    }
}
