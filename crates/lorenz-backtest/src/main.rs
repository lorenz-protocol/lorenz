//! Runnable backtest over a market (the bundled sample by default).
//!
//! ```sh
//! cargo run -p lorenz-backtest                          # bundled sample, human output
//! cargo run -p lorenz-backtest -- --json                # bundled sample, JSON output
//! cargo run -p lorenz-backtest -- --market market.json  # replay an external market
//! ```
//!
//! Without `--market` everything is deterministic and self-contained: the
//! sample market is embedded, so the numbers printed are reproducible by anyone
//! who clones the repo. With `--market <FILE>` the same deterministic pipeline
//! replays an external market JSON file. No live RPC, no hidden state.

use lorenz_backtest::{parse_market, run_backtest, BacktestReport, Market};
use lorenz_core::config::{CostConfig, RiskConfig};
use lorenz_core::telemetry::TradeRecord;
use lorenz_core::types::Bps;
use serde::Serialize;

const SAMPLE_MARKET: &str = include_str!("../data/sample_market.json");

const USAGE: &str = "\
Usage: lorenz-backtest [OPTIONS]

Replays a market (the bundled sample by default) and reports arbitrage economics.

Options:
  --market <FILE>  Replay an external market JSON file instead of the bundled sample
  --json           Emit the full result as a single JSON document (no human output)
  -h, --help       Print this help and exit";

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

/// Default risk limits used by the binary. Factored out so the same numbers are
/// reused by tests and stay the single source of truth for determinism.
fn default_risk() -> RiskConfig {
    RiskConfig {
        max_position: 5_000_000_000,
        min_profit: 1,
        max_consecutive_losses: 5,
    }
}

/// Default cost model used by the binary (see [`default_risk`]).
fn default_costs() -> CostConfig {
    CostConfig {
        flash_loan_fee: Bps(9),
        priority_fee_lamports: 50_000,
        jito_tip_lamports: 10_000,
        slippage_per_hop: Bps(5),
    }
}

/// Read and parse an external market JSON file.
///
/// Returns a human-readable error (rather than panicking) so the caller can
/// report it on stderr and exit non-zero. Reuses [`parse_market`] for decoding.
fn load_external_market(path: &str) -> Result<Market, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read market file '{path}': {e}"))?;
    parse_market(&contents).map_err(|e| format!("failed to parse market file '{path}': {e}"))
}

fn main() {
    let mut json = false;
    let mut market_path: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            "--market" => {
                let Some(path) = args.next() else {
                    eprintln!("error: --market requires a <FILE> path argument");
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                };
                market_path = Some(path);
            }
            other => {
                if let Some(path) = other.strip_prefix("--market=") {
                    market_path = Some(path.to_string());
                } else {
                    eprintln!("error: unknown flag '{other}'");
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                }
            }
        }
    }

    let _ = lorenz_core::telemetry::init_tracing();

    let market = match &market_path {
        Some(path) => match load_external_market(path) {
            Ok(m) => m,
            Err(msg) => {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
        },
        None => parse_market(SAMPLE_MARKET).expect("bundled sample market is valid JSON"),
    };

    let risk = default_risk();
    let costs = default_costs();

    let report = run_backtest(&market, &risk, &costs);

    if json {
        let out = JsonReport::new(market.snapshots.len(), &report);
        let rendered =
            serde_json::to_string_pretty(&out).expect("backtest report serializes to JSON");
        println!("{rendered}");
        return;
    }

    let source_label = match &market_path {
        Some(path) => path.as_str(),
        None => "sample market",
    };
    println!("Lorenz Protocol backtest ({source_label})");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_valid_market_string_parses_and_backtests() {
        // Exercises the string -> market -> report seam shared by the default
        // (bundled) and `--market <FILE>` code paths, without touching the
        // filesystem. A single pool-less snapshot is valid and yields a clean,
        // empty run.
        let json = r#"{
            "base_token": "SOL",
            "notional": 1000000,
            "snapshots": [{ "ts": 1, "pools": [] }]
        }"#;

        let market = parse_market(json).expect("small valid market parses");
        let report = run_backtest(&market, &default_risk(), &default_costs());

        assert_eq!(market.snapshots.len(), 1);
        assert_eq!(report.candidates_found, 0);
        assert_eq!(report.profitable_after_costs, 0);
    }
}
