//! Runnable backtest over a bundled sample market.
//!
//! ```sh
//! cargo run -p lorenz-backtest
//! ```
//!
//! Everything here is deterministic and self-contained: the sample market is
//! embedded, so the numbers printed are reproducible by anyone who clones the
//! repo. No live RPC, no hidden state.

use lorenz_backtest::{parse_market, run_backtest};
use lorenz_core::config::{CostConfig, RiskConfig};
use lorenz_core::types::Bps;

const SAMPLE_MARKET: &str = include_str!("../data/sample_market.json");

fn main() {
    let _ = lorenz_core::telemetry::init_tracing();

    let market = parse_market(SAMPLE_MARKET).expect("bundled sample market is valid JSON");

    let risk = RiskConfig {
        max_position: 5_000_000_000,
        min_profit: 1,
        max_consecutive_losses: 5,
    };
    let costs = CostConfig {
        flash_loan_fee: Bps(9),
        priority_fee_lamports: 50_000,
        jito_tip_lamports: 10_000,
        slippage_per_hop: Bps(5),
    };

    let report = run_backtest(&market, &risk, &costs);

    println!("Lorenz Protocol backtest (sample market)");
    println!("  snapshots replayed : {}", market.snapshots.len());
    println!("  candidate cycles   : {}", report.candidates_found);
    println!("  profitable (net)   : {}", report.profitable_after_costs);
    println!(
        "  total net profit   : {} base units",
        report.total_net_profit
    );
    println!();
    for rec in &report.records {
        let mut route: Vec<String> = rec.route.iter().map(|h| h.token_in.0.clone()).collect();
        if let Some(last) = rec.route.last() {
            route.push(last.token_out.0.clone());
        }
        println!(
            "  ts={} route={} gross={} net={} submitted={}",
            rec.ts,
            route.join("->"),
            rec.gross_profit,
            rec.net_profit,
            rec.submitted
        );
    }
}
