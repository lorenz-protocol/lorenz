# 07 — Backtester (`lorenz-backtest`)

A deterministic replay / backtest harness. Feed it a sequence of market snapshots
(pool reserves at points in time) and, for each snapshot, it does exactly what the
live engine would do in its **analysis** step:

1. build the arbitrage graph,
2. find a candidate cycle (`lorenz-graph`),
3. size it and recompute the **exact** output hop-by-hop (`lorenz-amm` via `lorenz-dex`),
4. subtract the full cost model,
5. emit an auditable `TradeRecord`.

The cost model is shared with the live config (`lorenz_core::config`) **on purpose**:
simulated economics use the same numbers as production accounting, so a backtest
cannot quietly be more optimistic than reality (invariant O4).

## Data model (on-disk / on-wire form)

These types are `Serialize`/`Deserialize` and define the JSON market format.

```rust
struct PoolSnapshot {           // a single pool row
    id: String, dex: Dex,
    token_a: String, token_b: String,
    reserve_a: u128, reserve_b: u128, fee_bps: u32,
}
struct MarketSnapshot { ts: u64, pools: Vec<PoolSnapshot> }   // all pools at a time
struct Market {
    base_token: String,   // denominates borrowing and profit
    notional: u64,        // borrowed per attempt, base units
    snapshots: Vec<MarketSnapshot>,
}
```

> **Naming caution:** `lorenz-backtest::PoolSnapshot` is a *single pool* row, whereas
> `lorenz-stream::PoolSnapshot` is *a slot + many pools*. The backtester's
> `MarketSnapshot` is the analogue of the stream's `PoolSnapshot`. They are not
> interchangeable types.

`PoolSnapshot::to_pool()` converts a row into a `lorenz_dex::CpmmPool`. (The
backtester models constant-product pools; the JSON `dex` field is free to name a
CLMM venue, but the row is priced as CPMM from its `reserve_a/reserve_b`.)

## The cost model

```rust
struct CostModel { cfg: CostConfig }

fn total_cost(&self, notional: u128, hops: usize) -> u128 {
    flash    = notional * flash_loan_fee_bps      / 10_000;
    slippage = notional * slippage_per_hop_bps * hops / 10_000;
    fixed    = priority_fee_lamports + jito_tip_lamports;
    flash + slippage + fixed
}
```

So cost scales with notional (flash + slippage), with the number of hops
(slippage), and adds a flat per-tx network component. Tests confirm a 3-hop cost
exceeds a 2-hop cost for the same notional.

## The run loop

```rust
fn run_backtest(market: &Market, risk: &RiskConfig, costs: &CostConfig) -> BacktestReport
```

Setup: `base = market.base_token`; `notional = min(market.notional,
risk.max_position)` (the risk ceiling is honored even in simulation).

For each `MarketSnapshot`:

1. Build a fresh `ArbitrageGraph` and a `HashMap<PoolId, CpmmPool>`; for each pool
   add `pool.edges(notional)` (probe size = the actual notional) and insert the
   pool.
2. `graph.find_arbitrage()`; if none, skip. Otherwise `candidates_found += 1`.
3. `simulate_cycle(...)` walks the cycle hop-by-hop with **exact** `quote` math:
   - It first **rotates** the cycle so it starts (and ends) at the base token —
     otherwise profit can't be denominated in the loaned asset.
   - It quotes each hop in turn, threading the output of one hop into the next; if
     any hop quotes `0` (or a pool is missing) the trade is dropped.
   - Returns the `Vec<Hop>` route and the final output amount.
4. `gross_profit = final_out - notional` (signed `i128`);
   `cost = total_cost(notional, route.len())`;
   `net_profit = gross_profit - cost`.
5. `submitted = net_profit >= risk.min_profit`. If submitted, increment
   `profitable_after_costs` and add to `total_net_profit`.
6. Push a `TradeRecord { ts, route, notional, gross_profit, net_profit,
   submitted }`.

### `BacktestReport`

```rust
struct BacktestReport {
    records: Vec<TradeRecord>,
    candidates_found: usize,
    profitable_after_costs: usize,
    total_net_profit: i128,
}
```

`parse_market(json) -> Result<Market, serde_json::Error>` loads a market.

## The runnable binary

```sh
cargo run -p lorenz-backtest
```

`src/main.rs` embeds the sample market with `include_str!("../data/sample_market.json")`,
so the printed numbers are **reproducible by anyone who clones the repo** — no
live RPC, no hidden state. It initializes tracing, parses the bundled market, runs
with a fixed `RiskConfig`/`CostConfig`, then prints the snapshot count, candidate
count, profitable count, total net profit, and one line per `TradeRecord`
(`ts route gross net submitted`).

### The bundled sample market (`data/sample_market.json`)

- `base_token = "SOL"`, `notional = 1_000_000_000`.
- A `SOL → USDC → BONK → SOL` triangle across `raydium_amm`, `meteora_dlmm`, and
  `whirlpool` pools (the `dex` labels are illustrative; rows are priced as CPMM).
- **Snapshot 1** (`ts 1700000000`): the `bonk_sol` pool is mispriced
  (`reserve 1e12 / 1.05e12`) and fees are a tiny `1 bps` → a genuine cyclic
  mispricing the detector finds and prices as profitable.
- **Snapshot 2** (`ts 1700000060`): the mispricing is removed (`1e12 / 1e12`) and
  fees rise to `25 bps` → demonstrates the no-arb / costed-out case.

This makes the binary a self-contained demonstration that the detect → size →
cost pipeline works end-to-end on a known input.

Continue to [08 — End-to-End Flows](08-end-to-end-flows.md).
