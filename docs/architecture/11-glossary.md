# 11 — Glossary

Terms used throughout the Lorenz codebase and these docs. Where a term maps to a
type or module, the location is given.

## Domain & finance

- **Arbitrage** — exploiting a price discrepancy across venues for a risk-free
  (here, atomic) profit. Lorenz targets *cyclic* arbitrage: a loop of swaps that
  returns more of the starting token than it spent.
- **Atomic execution** — the whole arbitrage runs in a single transaction that
  either fully succeeds or fully reverts (invariant I1). No partial state.
- **Base units** — the smallest integer unit of a token (lamports for SOL,
  smallest SPL unit otherwise). `Amount(u64)`. No floating-point value ever
  settles.
- **Basis point (bp)** — 0.01%. `Bps(u32)`, with `DENOMINATOR = 10_000`. Fees,
  thresholds, and slippage are expressed in bps.
- **Notional** — the borrowed size of one arbitrage attempt, denominated in the
  base/loaned asset. Bounded by `spend_cap` on-chain (I3) and `max_position`
  off-chain.
- **Min profit / profit floor** — the minimum net (after-cost) profit required to
  fire. On-chain settlement requires `balance_after ≥ balance_before + min_profit`
  (I2).
- **Realized profit** — `balance_after - balance_before` measured on the vault
  token account after the round trip; the base for the protocol fee (I5).

## Solana / on-chain

- **AMM (Automated Market Maker)** — a pool that prices swaps from an on-chain
  formula instead of an order book.
- **CPMM (Constant-Product MM)** — `x · y = k` pools (Uniswap-v2 style). Reserves
  live in two SPL vault accounts. Implemented in `lorenz-amm` / `lorenz-dex::CpmmPool`.
- **CLMM (Concentrated-Liquidity MM)** — Uniswap-v3 / Orca-Whirlpool style: a pool
  with a current `sqrt_price` and active `liquidity` over tick ranges.
  `lorenz-dex::ClmmPool` implements the *single-tick* case; tick-crossing is roadmap.
- **sqrt_price (Q64.64)** — a CLMM's current price stored as `sqrt(price)` in
  Q64.64 fixed point (`sqrt_price_x64`); `Q64 = 2^64` converts to a real.
- **Tick / tick array** — discrete price points where CLMM liquidity can change.
  Crossing a tick changes active `L`; the single-tick model is valid only within
  one tick.
- **SPL token account** — a 165-byte account holding a token balance; the `amount`
  is a little-endian `u64` at offset 64. Read by `decoder::spl::token_account_amount`.
- **Vault** — for a CPMM pool, the token account holding one side's reserves. For
  Lorenz, also the per-user PDA (`Vault` account) that holds trading balance and
  enforces the on-chain invariants.
- **PDA (Program-Derived Address)** — a deterministic, key-less account owned by a
  program. Lorenz's vault is a PDA at seeds `[b"vault", owner]`.
- **CPI (Cross-Program Invocation)** — one program calling another. The flash-loan
  borrow/repay and per-hop swaps are CPIs (currently seams).
- **Flash loan** — an uncollateralized loan that must be borrowed and repaid
  (plus fee) within the same transaction, or the whole transaction reverts. Lorenz
  targets Kamino Lending (`KLend2g3cP87…gmjD`). This same-tx rule is what bounds
  the downside to fees.
- **Geyser / Yellowstone** — Solana's plugin/gRPC interface for *pushed* account
  and slot updates (vs polling RPC). The live ingest transport seam
  (`lorenz-stream::geyser`).
- **Slot** — Solana's unit of block time; a `PoolSnapshot` is tagged with the slot
  it represents.
- **Priority fee** — extra lamports per compute unit to prioritize a transaction.
  A tuner parameter (`priority_fee_lamports`).
- **Jito / Jito tip** — Jito enables bundle submission (atomic groups of txs) to
  block builders; the *tip* (`jito_tip_lamports`) bids for inclusion. A tuner
  parameter and cost-model input.
- **Land rate** — fraction of submitted bundles/txs that actually land on-chain.
  Drives the `HeuristicTuner` (`EngineStats::land_rate`).
- **LUT (Address Lookup Table)** — a Solana mechanism to compress account
  addresses in versioned transactions; part of the roadmap tx builder.
- **Anchor** — the Rust framework for Solana programs used by `programs/executor`
  (`#[program]`, `#[derive(Accounts)]`, `declare_id!`, 8-byte account
  discriminators).

## Lorenz architecture

- **Data plane** — the deterministic hot path
  (`ingest → detect → size → build → submit → execute`). No AI, no float money.
- **Control plane** — the off-hot-path layer (`lorenz-agent`) that decides limits
  and parameters; can only *tighten*; never signs hot-path txs.
- **Seam** — a typed, documented integration point that compiles and returns
  `NotImplemented`, so surrounding logic stays reviewable (🟡).
- **Roadmap** — scoped but not yet in code (🔭).
- **Detector** — `lorenz-graph::ArbitrageGraph`; finds candidate profitable cycles
  via negative-cycle Bellman-Ford on `-ln(rate)` edge weights.
- **Edge / Cycle** — a directed swap with an effective `rate`; a closed loop whose
  rate `product > 1` is gross-profitable.
- **Probe size** — the single input size at which edge rates are computed for the
  graph; cycles found are *candidates* re-priced exactly afterward.
- **Cost model** — flash-loan fee + per-hop slippage + priority fee + Jito tip;
  shared by backtest and live (`CostConfig` / `lorenz-backtest::CostModel`).
- **Kill-switch** — the risk manager's hard stop; trips after
  `max_consecutive_losses` and blocks all trades until reset.
- **Decision ledger** — append-only record of every control-plane action with a
  rationale (`DecisionLedger`); no mutate/delete API.
- **Orchestrator** — the layer that turns one LLM `AgentAction` into a clamped,
  ledger-recorded state change (`lorenz-agent::Orchestrator`).
- **`AgentAction`** — the closed set an LLM may return: `Hold`, `TightenCap`,
  `AdjustParams`, `TripKillSwitch`, `Note`.
- **`TradeRecord` / `Hop`** — the auditable telemetry records (one trade, one hop)
  that form the data→control up-channel.
- **Replay source** — a deterministic `PoolSnapshotSource` over recorded snapshots
  (`lorenz-stream::replay::ReplaySource`); the working, reproducible driver.

## Invariant IDs (quick reference)

- **I1–I5** — on-chain guarantees (atomic-or-revert, no loss, bounded spend,
  scoped authority, transparent capped fee). See [09](09-safety-and-invariants.md).
- **O1–O5** — off-chain guarantees (integer settlement, strict config, agent-only-
  tightens + LLM clamp, shared cost model, auditable decisions).

---

Back to the [index](README.md).
