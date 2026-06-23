//! WebAssembly facade for the Lorenz deterministic arbitrage engine.
//!
//! This crate is *glue only*: it contains no engine logic. Every exported
//! function parses string/JSON input, calls the real, tested engine crates
//! (`lorenz-amm`, `lorenz-graph`, `lorenz-dex`, `lorenz-backtest`), and returns
//! a `String` (JSON for structured results). The engine itself stays a pure
//! Rust workspace with zero `wasm-bindgen` dependency.
//!
//! String-in / string-out JSON is used deliberately:
//! - `u128` / `i128` cannot be passed safely across the JS<->wasm boundary, so
//!   large quantities travel as decimal strings.
//! - It is the most portable choice across every wasm target (web, nodejs,
//!   bundler) and needs nothing beyond `serde_json` on the JS side.
//!
//! Everything here is a *simulation / replay* of the real algorithms. There is
//! no live trading, no RPC, no signing material — just deterministic math the
//! browser can reproduce bit-for-bit against the native binary.

use lorenz_amm::CpmmReserves;
use lorenz_backtest::{run_backtest as engine_run_backtest, Market, PoolSnapshot};
use lorenz_core::config::{CostConfig, RiskConfig};
use lorenz_core::types::Bps;
use lorenz_graph::{ArbitrageGraph, Cycle, Edge};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Install the panic hook so Rust panics surface as readable messages in the
/// browser console instead of an opaque `unreachable`. Idempotent; safe to call
/// more than once.
#[wasm_bindgen]
pub fn start() {
    console_error_panic_hook::set_once();
}

// ---------------------------------------------------------------------------
// 2. AMM playground
// ---------------------------------------------------------------------------

fn parse_u128(label: &str, s: &str) -> Result<u128, JsValue> {
    s.trim()
        .parse::<u128>()
        .map_err(|e| JsValue::from_str(&format!("invalid u128 for `{label}`: {e}")))
}

/// Exact constant-product output for a swap, computed with the same integer
/// math the chain would use at settlement.
///
/// `reserve_in`, `reserve_out`, `amount_in` are decimal `u128` strings (JS
/// cannot safely carry `u128`). Returns the output amount as a decimal string.
#[wasm_bindgen]
pub fn amount_out(
    reserve_in: String,
    reserve_out: String,
    fee_bps: u32,
    amount_in: String,
) -> Result<String, JsValue> {
    let r_in = parse_u128("reserve_in", &reserve_in)?;
    let r_out = parse_u128("reserve_out", &reserve_out)?;
    let amt = parse_u128("amount_in", &amount_in)?;
    let reserves = CpmmReserves::new(r_in, r_out, Bps(fee_bps));
    let out = reserves
        .amount_out(amt)
        .ok_or_else(|| JsValue::from_str("amount_out overflowed"))?;
    Ok(out.to_string())
}

/// Marginal spot price (out per in), ignoring fees and impact. Display only.
#[wasm_bindgen]
pub fn spot_price(reserve_in: String, reserve_out: String) -> Result<f64, JsValue> {
    let r_in = parse_u128("reserve_in", &reserve_in)?;
    let r_out = parse_u128("reserve_out", &reserve_out)?;
    Ok(CpmmReserves::new(r_in, r_out, Bps(0)).spot_price())
}

/// Effective realized rate (out/in) for `amount_in`, including fee and price
/// impact. Display only.
#[wasm_bindgen]
pub fn effective_rate(
    reserve_in: String,
    reserve_out: String,
    fee_bps: u32,
    amount_in: String,
) -> Result<f64, JsValue> {
    let r_in = parse_u128("reserve_in", &reserve_in)?;
    let r_out = parse_u128("reserve_out", &reserve_out)?;
    let amt = parse_u128("amount_in", &amount_in)?;
    let reserves = CpmmReserves::new(r_in, r_out, Bps(fee_bps));
    reserves
        .effective_rate(amt)
        .ok_or_else(|| JsValue::from_str("effective_rate overflowed"))
}

// ---------------------------------------------------------------------------
// 3. Graph / detection
// ---------------------------------------------------------------------------

/// `lorenz_graph::Edge` is not serde-derivable in the engine crate (keeping it
/// pure), so we mirror its wire shape here for (de)serialization.
#[derive(Debug, Serialize, Deserialize)]
struct EdgeWire {
    from: String,
    to: String,
    pool: String,
    rate: f64,
}

impl From<&Edge> for EdgeWire {
    fn from(e: &Edge) -> Self {
        EdgeWire {
            from: e.from.0.clone(),
            to: e.to.0.clone(),
            pool: e.pool.0.clone(),
            rate: e.rate,
        }
    }
}

impl EdgeWire {
    fn to_edge(&self) -> Edge {
        Edge {
            from: self.from.as_str().into(),
            to: self.to.as_str().into(),
            pool: self.pool.as_str().into(),
            rate: self.rate,
        }
    }
}

/// Wire form of a detected cycle.
#[derive(Debug, Serialize, Deserialize)]
struct CycleWire {
    edges: Vec<EdgeWire>,
    product: f64,
}

impl From<&Cycle> for CycleWire {
    fn from(c: &Cycle) -> Self {
        CycleWire {
            edges: c.edges.iter().map(EdgeWire::from).collect(),
            product: c.product,
        }
    }
}

fn cycle_to_json(cycle: Option<Cycle>) -> Result<String, JsValue> {
    match cycle {
        Some(c) => serde_json::to_string(&CycleWire::from(&c))
            .map_err(|e| JsValue::from_str(&e.to_string())),
        None => Ok("null".to_string()),
    }
}

/// Run negative-cycle arbitrage detection over a raw edge list.
///
/// Input: JSON array of `{from, to, pool, rate}`. Returns JSON of the detected
/// `Cycle` (`{edges:[...], product:...}`) or JSON `null` if none.
#[wasm_bindgen]
pub fn find_arbitrage(edges_json: &str) -> Result<String, JsValue> {
    let wires: Vec<EdgeWire> = serde_json::from_str(edges_json)
        .map_err(|e| JsValue::from_str(&format!("invalid edges JSON: {e}")))?;
    let mut graph = ArbitrageGraph::new();
    for w in &wires {
        graph.add_edge(w.to_edge());
    }
    cycle_to_json(graph.find_arbitrage())
}

// ---------------------------------------------------------------------------
// 4. Screener
// ---------------------------------------------------------------------------

/// Screen a set of CPMM pool snapshots for a profitable cycle at a given probe
/// size.
///
/// Input: JSON array of pool snapshots `{id, dex, token_a, token_b, reserve_a,
/// reserve_b, fee_bps}` (the `lorenz-backtest::PoolSnapshot` shape).
/// `probe_size` is a decimal `u128` string. Builds the pools, collects their
/// directed edges at the probe size, runs detection, and returns the `Cycle`
/// JSON or `null`.
#[wasm_bindgen]
pub fn screen(pools_json: &str, probe_size: String) -> Result<String, JsValue> {
    let probe = parse_u128("probe_size", &probe_size)?;
    let snapshots: Vec<PoolSnapshot> = serde_json::from_str(pools_json)
        .map_err(|e| JsValue::from_str(&format!("invalid pools JSON: {e}")))?;
    let mut graph = ArbitrageGraph::new();
    for ps in &snapshots {
        let pool = ps.to_pool();
        for edge in pool.edges(probe) {
            graph.add_edge(edge);
        }
    }
    cycle_to_json(graph.find_arbitrage())
}

// ---------------------------------------------------------------------------
// 5. Backtester (flagship)
// ---------------------------------------------------------------------------

/// Run the deterministic backtest/replay over a market.
///
/// Inputs are JSON strings for a `Market`, a `RiskConfig` and a `CostConfig`.
/// Returns the `BacktestReport` as JSON (`records`, `candidates_found`,
/// `profitable_after_costs`, `total_net_profit`). `i128` fields serialize as
/// JSON numbers.
#[wasm_bindgen]
pub fn run_backtest(
    market_json: &str,
    risk_json: &str,
    costs_json: &str,
) -> Result<String, JsValue> {
    let market: Market = serde_json::from_str(market_json)
        .map_err(|e| JsValue::from_str(&format!("invalid market JSON: {e}")))?;
    let risk: RiskConfig = serde_json::from_str(risk_json)
        .map_err(|e| JsValue::from_str(&format!("invalid risk JSON: {e}")))?;
    let costs: CostConfig = serde_json::from_str(costs_json)
        .map_err(|e| JsValue::from_str(&format!("invalid costs JSON: {e}")))?;

    let report = engine_run_backtest(&market, &risk, &costs);
    serde_json::to_string(&report).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_out_known_value() {
        let out = amount_out("1000000".into(), "1000000".into(), 0, "1000".into()).unwrap();
        assert_eq!(out, "999");
    }

    #[test]
    fn find_arbitrage_triangular() {
        let edges = r#"[
            {"from":"A","to":"B","pool":"p1","rate":1.0},
            {"from":"B","to":"C","pool":"p2","rate":1.0},
            {"from":"C","to":"A","pool":"p3","rate":1.1}
        ]"#;
        let out = find_arbitrage(edges).unwrap();
        assert!(out.contains("\"product\""));
        assert_ne!(out, "null");
    }

    #[test]
    fn find_arbitrage_none() {
        let edges = r#"[
            {"from":"A","to":"B","pool":"p1","rate":2.0},
            {"from":"B","to":"A","pool":"p1","rate":0.5}
        ]"#;
        assert_eq!(find_arbitrage(edges).unwrap(), "null");
    }
}
