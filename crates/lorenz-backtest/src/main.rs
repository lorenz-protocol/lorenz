//! Runnable backtest over a bundled sample market.
//!
//! ```sh
//! cargo run -p lorenz-backtest            # human-readable output
//! cargo run -p lorenz-backtest -- --json  # machine-readable JSON
//! ```
//!
//! Everything here is deterministic and self-contained: the sample market is
//! embedded, so the numbers printed are reproducible by anyone who clones the
//! repo. No live RPC, no hidden state.

use lorenz_backtest::{parse_market, run_backtest, BacktestReport};
use lorenz_core::config::{CostConfig, RiskConfig};
use lorenz_core::telemetry::TradeRecord;
use lorenz_core::types::Bps;
use serde::Serialize;

const SAMPLE_MARKET: &str = include_str!("../data/sample_market.json");

const USAGE: &str = "\
Usage: lorenz-backtest [OPTIONS]

Replays the bundled sample market and reports arbitrage economics.

Options:
  --json       Emit the full result as a single JSON document (no human output)
  -h, --help   Print this help and exit";

/// Machine-readable view of a backtest run.
///
/// Borrows the aggregated [`BacktestReport`] and adds `snapshots_replayed` so
/// the JSON carries exactly the same figures the human output reports. Field
/// order is fixed and no maps are emitted, so the document is deterministic.
#[derive(Debug, Serialize)]
struct JsonReport<'a> {
    snapshots_replayed: usize,
    candidates_found: usize,
    profitable_after_costs: usize,
    total_net_profit: i128,
    records: &'a [TradeRecord],
}

impl<'a> JsonReport<'a> {
    fn new(snapshots_replayed: usize, report: &'a BacktestReport) -> Self {
        Self {
            snapshots_replayed,
            candidates_found: report.candidates_found,
            profitable_after_costs: report.profitable_after_costs,
            total_net_profit: report.total_net_profit,
            records: &report.records,
        }
    }
}

fn main() {
    let mut json = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("error: unknown flag '{other}'");
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }

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

    if json {
        let out = JsonReport::new(market.snapshots.len(), &report);
        let rendered =
            serde_json::to_string_pretty(&out).expect("backtest report serializes to JSON");
        println!("{rendered}");
        return;
    }

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
