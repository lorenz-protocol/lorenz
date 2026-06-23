# lorenz-wasm

A thin [`wasm-bindgen`](https://rustwasm.github.io/wasm-bindgen/) façade that runs the
**real, deterministic Lorenz arbitrage engine** in the browser. It contains no
engine logic of its own — every function parses string/JSON input, calls the
tested engine crates (`lorenz-amm`, `lorenz-graph`, `lorenz-dex`,
`lorenz-backtest`), and returns a `String`. The engine crates stay pure Rust with
zero `wasm-bindgen` dependency; all glue lives here.

> **Simulation / replay only.** These are the genuine production algorithms, but
> the WASM build does no live trading, holds no keys, and makes no RPC calls. It
> replays bundled or user-supplied market data and reproduces the native
> binary's numbers bit-for-bit (the backtest of the bundled sample market yields
> `candidates_found = 1`, `profitable_after_costs = 1`,
> `total_net_profit = 44086018`, route `SOL->USDC->BONK->SOL`).

## Why string-in / string-out JSON

- `u128` / `i128` cannot cross the JS↔wasm boundary safely, so large quantities
  travel as **decimal strings**.
- JSON via `serde_json` is the most portable choice across every wasm target
  (web, nodejs, bundler) and needs nothing extra on the JS side.

## API surface

| Function | Signature | Returns |
| --- | --- | --- |
| `start()` | `() => void` | Installs `console_error_panic_hook` for readable panics. Idempotent. |
| `amount_out` | `(reserve_in: string, reserve_out: string, fee_bps: number, amount_in: string) => string` | Exact CPMM output amount, decimal string (integer math, matches settlement). |
| `spot_price` | `(reserve_in: string, reserve_out: string) => number` | Marginal price out/in (display only). |
| `effective_rate` | `(reserve_in: string, reserve_out: string, fee_bps: number, amount_in: string) => number` | Realized rate out/in incl. fee + impact (display only). |
| `find_arbitrage` | `(edges_json: string) => string` | JSON array of `{from,to,pool,rate}` → JSON `Cycle` `{edges,product}` or `null`. |
| `screen` | `(pools_json: string, probe_size: string) => string` | JSON array of `PoolSnapshot` `{id,dex,token_a,token_b,reserve_a,reserve_b,fee_bps}` → `Cycle` JSON or `null`. |
| `run_backtest` | `(market_json: string, risk_json: string, costs_json: string) => string` | `Market` + `RiskConfig` + `CostConfig` → `BacktestReport` JSON. |

All functions that take JSON throw a JS `Error` (mapped from `JsValue`) with a
descriptive message on malformed input.

### Function → demo mapping

| Function | Powers the demo |
| --- | --- |
| `amount_out` (+ `spot_price`, `effective_rate`) | **AMM playground** — slide reserves/fee, see exact output and price impact. |
| `find_arbitrage` | **Graph visualizer** — feed edges, highlight the detected negative cycle. |
| `screen` | **Arbitrage screener** — paste pool snapshots, surface a profitable cycle at a probe size. |
| `run_backtest` | **Interactive backtester** — replay a market with chosen risk/cost knobs and read the report. |

### JSON shapes

`RiskConfig`: `{ "max_position": u64, "min_profit": u64, "max_consecutive_losses": u32 }`

`CostConfig`: `{ "flash_loan_fee": u32, "priority_fee_lamports": u64, "jito_tip_lamports": u64, "slippage_per_hop": u32 }`
(`flash_loan_fee` / `slippage_per_hop` are basis points.)

`Market`: `{ "base_token": string, "notional": u64, "snapshots": [ { "ts": u64, "pools": [PoolSnapshot...] } ] }`

`BacktestReport` (output): `{ "records": [TradeRecord...], "candidates_found": number, "profitable_after_costs": number, "total_net_profit": number }`
(`total_net_profit` and the per-record `gross_profit`/`net_profit` are `i128`,
serialized as JSON numbers.)

## Building

Requires the wasm target and `wasm-pack`:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-pack   # one-time; may be slow
```

Build both packages from the workspace root:

```sh
# Node.js (CommonJS) — used by the smoke test.
wasm-pack build crates/lorenz-wasm --target nodejs --out-dir pkg-node

# Web (ES module) — for the website to import.
wasm-pack build crates/lorenz-wasm --target web --out-dir pkg
```

Each produces `lorenz_wasm.js`, `lorenz_wasm.d.ts`, and `lorenz_wasm_bg.wasm`.

To merely prove it compiles to wasm without `wasm-pack`:

```sh
cargo build -p lorenz-wasm --target wasm32-unknown-unknown
```

## Using from the web (`pkg`)

```js
import init, { start, run_backtest } from "./pkg/lorenz_wasm.js";
await init();          // load the .wasm
start();               // readable panics
const reportJson = run_backtest(marketJson, riskJson, costsJson);
console.log(JSON.parse(reportJson));
```

## Smoke test

Builds against `pkg-node`, runs the bundled sample market through `run_backtest`
with the exact risk/cost config the native binary uses, and asserts the numbers
match plus the `amount_out` unit value:

```sh
wasm-pack build crates/lorenz-wasm --target nodejs --out-dir pkg-node
node crates/lorenz-wasm/smoke_test.mjs
```

Expected: all checks `PASS` and `ALL CHECKS PASSED`.
