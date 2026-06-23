// Smoke test: load the nodejs wasm package, run the REAL engine in wasm, and
// assert the numbers match the native `cargo run -p lorenz-backtest` output.
//
//   node crates/lorenz-wasm/smoke_test.mjs
//
// The nodejs package is CommonJS, so we bridge it with createRequire.
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));

const wasm = require(join(here, "pkg-node", "lorenz_wasm.js"));
wasm.start();

const marketJson = readFileSync(
  join(here, "..", "lorenz-backtest", "data", "sample_market.json"),
  "utf8",
);

// Same RiskConfig / CostConfig the native binary uses (crates/lorenz-backtest/src/main.rs).
const riskJson = JSON.stringify({
  max_position: 5_000_000_000,
  min_profit: 1,
  max_consecutive_losses: 5,
});
const costsJson = JSON.stringify({
  flash_loan_fee: 9, // Bps(9)
  priority_fee_lamports: 50_000,
  jito_tip_lamports: 10_000,
  slippage_per_hop: 5, // Bps(5)
});

let failures = 0;
function check(name, actual, expected) {
  const ok = actual === expected;
  if (!ok) failures++;
  console.log(`  ${ok ? "PASS" : "FAIL"} ${name}: got ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`);
}

console.log("== amount_out unit check ==");
check("amount_out(1000000,1000000,0,1000)", wasm.amount_out("1000000", "1000000", 0, "1000"), "999");

console.log("== run_backtest (sample market) ==");
const reportJson = wasm.run_backtest(marketJson, riskJson, costsJson);
const report = JSON.parse(reportJson);

// i128 fields may arrive as JS number or string depending on serde_json; normalize.
const totalNet = typeof report.total_net_profit === "string"
  ? Number(report.total_net_profit)
  : report.total_net_profit;

check("candidates_found", report.candidates_found, 1);
check("profitable_after_costs", report.profitable_after_costs, 1);
check("total_net_profit", totalNet, 44086018);

// Reconstruct the route of the first record the same way main.rs does.
// TokenId(pub String) serializes as a bare JSON string.
const rec = report.records[0];
const tokens = rec.route.map((h) => h.token_in);
tokens.push(rec.route[rec.route.length - 1].token_out);
const routeStr = tokens.join("->");
check("first record route", routeStr, "SOL->USDC->BONK->SOL");

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED" : `${failures} CHECK(S) FAILED`}`);
process.exit(failures === 0 ? 0 : 1);
