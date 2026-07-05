//! Runnable backtest over a market (the bundled sample by default).
//!
//! ```sh
//! cargo run -p lorenz-backtest                          # bundled sample, human output
//! cargo run -p lorenz-backtest -- --json                # bundled sample, JSON output
//! cargo run -p lorenz-backtest -- --market market.json  # replay an external market
//! cargo run -p lorenz-backtest -- --config engine.toml  # load risk/cost params from TOML
//! cargo run -p lorenz-backtest -- --top 5               # human output: 5 best records
//! ```
//!
//! `--top <N>` only affects the human output, where it prints just the `N`
//! records with the highest net profit (with a header noting it is the top `N`
//! of `M`). `--json` always emits the full report — every record plus the
//! aggregate analytics — regardless of `--top`.
//!
//! Without `--market` everything is deterministic and self-contained: the
//! sample market is embedded, so the numbers printed are reproducible by anyone
//! who clones the repo. With `--market <FILE>` the same deterministic pipeline
//! replays an external market JSON file. No live RPC, no hidden state.
//!
//! Without `--config` the built-in [`default_risk`]/[`default_costs`] values are
//! used. With `--config <FILE>` the `[risk]` and `[costs]` sections of an
//! [`EngineConfig`] TOML file override them. Note that `EngineConfig` also
//! requires an `[rpc]` section for schema reasons; the backtester never reads it,
//! but the file must still include a (dummy) `[rpc]` section with a `url` to
//! parse successfully.

use lorenz_backtest::{parse_market, run_backtest, top_by_net_profit, BacktestReport, Market};
use lorenz_core::config::{CostConfig, EngineConfig, RiskConfig};
use lorenz_core::telemetry::TradeRecord;
use lorenz_core::types::Bps;
use serde::Serialize;
use std::collections::BTreeMap;

const SAMPLE_MARKET: &str = include_str!("../data/sample_market.json");

const USAGE: &str = "\
Usage: lorenz-backtest [OPTIONS]

Replays a market (the bundled sample by default) and reports arbitrage economics.

Options:
  --market <FILE>  Replay an external market JSON file instead of the bundled sample
  --config <FILE>  Load risk/cost params from an EngineConfig TOML file (see note below)
  --top <N>        Human output only: print just the N highest-net-profit records
  --json           Emit the full result as a single JSON document (no human output)
  -h, --help       Print this help and exit

The --config file is an EngineConfig TOML with [risk] and [costs] sections. The
schema also requires an [rpc] section (with a `url`); the backtester never uses
it, but the file must still include a dummy [rpc] section to parse.";

/// Machine-readable view of a backtest run.
///
/// Borrows the aggregated [`BacktestReport`] and adds `snapshots_replayed` so
/// the JSON carries exactly the same figures the human output reports, plus the
/// aggregate analytics (`total_gross_profit`, best/worst net, hop histogram and
/// submission rate). Field order is fixed and the one map (`hop_histogram`) is a
/// `BTreeMap`, which serializes keys in ascending order, so the document is
/// deterministic. Always carries the full record set regardless of `--top`.
#[derive(Debug, Serialize)]
struct JsonReport<'a> {
    snapshots_replayed: usize,
    candidates_found: usize,
    profitable_after_costs: usize,
    submission_rate: f64,
    total_net_profit: i128,
    total_gross_profit: i128,
    best_net_profit: i128,
    worst_net_profit: i128,
    hop_histogram: &'a BTreeMap<usize, usize>,
    records: &'a [TradeRecord],
}

impl<'a> JsonReport<'a> {
    fn new(snapshots_replayed: usize, report: &'a BacktestReport) -> Self {
        Self {
            snapshots_replayed,
            candidates_found: report.candidates_found,
            profitable_after_costs: report.profitable_after_costs,
            submission_rate: report.submission_rate(),
            total_net_profit: report.total_net_profit,
            total_gross_profit: report.total_gross_profit,
            best_net_profit: report.best_net_profit,
            worst_net_profit: report.worst_net_profit,
            hop_histogram: &report.hop_histogram,
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

/// Read and parse an [`EngineConfig`] TOML file supplying risk/cost overrides.
///
/// Only `.risk` and `.costs` are consumed by the backtester, but the schema
/// (`deny_unknown_fields`) also requires an `[rpc]` section, so the file must
/// still include a (dummy) one. Returns a human-readable error rather than
/// panicking so the caller can report it on stderr and exit non-zero.
fn load_engine_config(path: &str) -> Result<EngineConfig, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read config file '{path}': {e}"))?;
    EngineConfig::from_toml(&contents)
        .map_err(|e| format!("failed to parse config file '{path}': {e}"))
}

/// Parse the `--top` count, exiting with usage on a malformed value. Kept as a
/// helper so the two-token (`--top N`) and `--top=N` forms share one parser.
fn parse_top_arg(raw: &str) -> usize {
    match raw.parse::<usize>() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("error: --top expects a non-negative integer, got '{raw}'");
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}

fn main() {
    let mut json = false;
    let mut market_path: Option<String> = None;
    let mut config_path: Option<String> = None;
    let mut top: Option<usize> = None;

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
            "--config" => {
                let Some(path) = args.next() else {
                    eprintln!("error: --config requires a <FILE> path argument");
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                };
                config_path = Some(path);
            }
            "--top" => {
                let Some(raw) = args.next() else {
                    eprintln!("error: --top requires an <N> count argument");
                    eprintln!("{USAGE}");
                    std::process::exit(2);
                };
                top = Some(parse_top_arg(&raw));
            }
            other => {
                if let Some(path) = other.strip_prefix("--market=") {
                    market_path = Some(path.to_string());
                } else if let Some(path) = other.strip_prefix("--config=") {
                    config_path = Some(path.to_string());
                } else if let Some(raw) = other.strip_prefix("--top=") {
                    top = Some(parse_top_arg(raw));
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

    let (risk, costs) = match &config_path {
        Some(path) => match load_engine_config(path) {
            Ok(cfg) => (cfg.risk, cfg.costs),
            Err(msg) => {
                eprintln!("error: {msg}");
                std::process::exit(1);
            }
        },
        None => (default_risk(), default_costs()),
    };

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
    println!(
        "  total gross profit : {} base units",
        report.total_gross_profit
    );
    println!("  best / worst net   : {} / {}", report.best_net_profit, report.worst_net_profit);
    println!("  submission rate    : {:.3}", report.submission_rate());
    println!();

    // `--top N` narrows the human output to the N highest-net records (stable
    // tie-break by original order); the full set is printed otherwise. `--json`
    // is unaffected and always emits every record.
    let total = report.records.len();
    let indices: Vec<usize> = match top {
        Some(n) => {
            println!("  showing top {n} of {total} records (by net profit)");
            top_by_net_profit(&report.records, n)
        }
        None => (0..total).collect(),
    };
    for &i in &indices {
        let rec = &report.records[i];
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

    #[test]
    fn engine_config_toml_yields_expected_risk_and_costs() {
        // A valid EngineConfig TOML supplies the risk/costs used by --config.
        // The [rpc] section is required by the schema even though the
        // backtester never reads it (see load_engine_config). No filesystem
        // access: we exercise the parse seam directly.
        let toml = r#"
            [rpc]
            url = "https://example.invalid"

            [risk]
            max_position = 1_000_000
            min_profit = 42
            max_consecutive_losses = 3

            [costs]
            flash_loan_fee = 7
            priority_fee_lamports = 25_000
            jito_tip_lamports = 5_000
            slippage_per_hop = 4
        "#;

        let cfg = EngineConfig::from_toml(toml).expect("valid engine config parses");

        assert_eq!(cfg.risk.max_position, 1_000_000);
        assert_eq!(cfg.risk.min_profit, 42);
        assert_eq!(cfg.risk.max_consecutive_losses, 3);
        assert_eq!(cfg.costs.flash_loan_fee, Bps(7));
        assert_eq!(cfg.costs.priority_fee_lamports, 25_000);
        assert_eq!(cfg.costs.jito_tip_lamports, 5_000);
        assert_eq!(cfg.costs.slippage_per_hop, Bps(4));
    }
}
