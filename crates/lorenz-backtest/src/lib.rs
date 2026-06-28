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

use lorenz_core::config::{CostConfig, RiskConfig};
use lorenz_core::telemetry::{Hop, TradeRecord};
use lorenz_core::types::{Amount, Bps, Dex, PoolId, TokenId};
use lorenz_dex::{CpmmPool, Quoter};
use lorenz_graph::{ArbitrageGraph, Cycle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    pub total_net_profit: i128,
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

        let Some(cycle) = graph.find_arbitrage() else {
            continue;
        };
        report.candidates_found += 1;

        let Some((route, final_out)) = simulate_cycle(&pools, &cycle, &base, notional) else {
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

        report.records.push(TradeRecord {
            ts: snap.ts,
            route,
            notional: Amount(notional.min(u128::from(u64::MAX)) as u64),
            gross_profit,
            net_profit,
            submitted,
        });
    }

    report
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
        assert_eq!(report.candidates_found, 1);
        let rec = &report.records[0];
        // Route closes back to SOL.
        assert_eq!(rec.route.first().unwrap().token_in, "SOL".into());
        assert_eq!(rec.route.last().unwrap().token_out, "SOL".into());
        // Gross profit is positive on this constructed mispricing.
        assert!(rec.gross_profit > 0, "gross was {}", rec.gross_profit);
    }

    #[test]
    fn report_serializes_and_round_trips_over_bundled_sample() {
        // The same embedded market the `lorenz-backtest` binary replays, so the
        // JSON mode is exercised against real computed values, not a fixture.
        const SAMPLE_MARKET: &str = include_str!("../data/sample_market.json");

        let market = parse_market(SAMPLE_MARKET).expect("bundled sample market is valid JSON");
        let report = run_backtest(&market, &risk(), &costs());

        // Serializes without error...
        let json = serde_json::to_string_pretty(&report).expect("report serializes to JSON");
        // ...and round-trips back into an equivalent value.
        let parsed: BacktestReport =
            serde_json::from_str(&json).expect("report round-trips from JSON");

        assert_eq!(parsed.candidates_found, report.candidates_found);
        assert_eq!(parsed.profitable_after_costs, report.profitable_after_costs);
        assert_eq!(parsed.total_net_profit, report.total_net_profit);
        assert_eq!(parsed.records, report.records);
    }
}
