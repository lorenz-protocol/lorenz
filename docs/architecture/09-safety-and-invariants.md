# 09 — Safety & Invariants

Lorenz's safety story is that its guarantees are **properties of the runtime and the
type system, checkable by reading the code** — not promises in a README. This
chapter is the architectural companion to the canonical
[`../INVARIANTS.md`](../INVARIANTS.md) and [`../SECURITY.md`](../SECURITY.md): it
explains *why* each guarantee holds and *where* to verify it.

There are two tiers:

- **On-chain invariants (I1–I5)** — trust-critical. They hold at runtime
  regardless of the off-chain bot's behaviour. The chain refuses violations.
- **Off-chain invariants (O1–O5)** — keep the engine honest and reproducible.
  Not trust-critical (the chain is the backstop), but they are what make the
  system auditable and the backtest trustworthy.

---

## On-chain invariants

| ID | Guarantee | Enforced by | Verified by |
| --- | --- | --- | --- |
| **I1** | **Atomic-or-revert.** The whole arbitrage runs in one instruction; if any step or the profit check fails, the tx reverts and the only cost is fees — never the notional. | Single-instruction design of `execute_arbitrage` + Solana tx atomicity (and Kamino's same-tx repay requirement once wired). | Program structure; the borrow/repay pairing the chain enforces. |
| **I2** | **No loss.** Settlement requires `balance_after ≥ balance_before + min_profit`, `min_profit ≥ 0`. | `profit = balance_after.checked_sub(balance_before)` + `require!(profit >= min_profit)`. A loss underflows the `checked_sub`. | `math::clears_profit_floor` unit tests; `protocol_fee` tests. |
| **I3** | **Bounded spend.** `notional ≤ vault.spend_cap`. | `require!(notional <= vault.spend_cap, SpendCapExceeded)`. | Read of `execute_arbitrage`. |
| **I4** | **Scoped authority.** The bot may invoke `execute_arbitrage` but can never withdraw; only the owner can `withdraw`. | Separate `ExecuteArbitrage` (signed by `bot_authority`) and `Withdraw` (signed by `owner`, `has_one = owner`) account contexts. | The two distinct `#[derive(Accounts)]` structs. |
| **I5** | **Transparent, capped fee.** Protocol fee is exactly `fee_bps` of realized profit, `fee_bps ≤ 1000` (10%), paid on-chain to a fixed account recorded at init. | `require!(fee_bps <= MAX_FEE_BPS)` at init + the fixed-destination fee transfer. | `protocol_fee` tests (fee is a fraction; never exceeds profit). |

All five live in `programs/executor/src/lib.rs`; see [06](06-onchain-program.md)
for the code walk.

### Why these compose into "no rug, no loss"

- A **losing trade cannot settle** (I2) — the worst case of a bad attempt is the
  fees/tip (I1), never the borrowed notional.
- The **bot cannot drain funds** (I4) — withdrawal authority never leaves the
  owner; the delegate's only power is to trigger a profit-bearing round trip.
- **Position size and fee are bounded on-chain** (I3, I5) — not by the bot's good
  behaviour.

This is the basis of the non-custodial model in
[ADR 0002](../adr/0002-non-custodial-vault.md): users keep ownership; the bot gets
a scoped delegate; the downside is provable on-chain.

---

## Off-chain invariants

| ID | Guarantee | Where enforced | Verified by |
| --- | --- | --- | --- |
| **O1** | **Integer settlement math.** Output amounts are integer; `f64` is used only for analytics/edge-weights, never for settling amounts. | `lorenz-amm` (`amount_out` is all `u128`/`checked_*`); `f64` confined to `spot_price`/`effective_rate`/graph weights. | `lorenz-amm` property tests: `output_bounded_by_reserve`, `monotonic_in_input`, `no_free_money_single_hop`. |
| **O2** | **Strict config.** Unknown config keys are rejected, so a typo cannot silently disable a feature. | `lorenz_core::config` `#[serde(deny_unknown_fields)]`. | `unknown_key_is_rejected` test. Motivated by [ADR 0004](../adr/0004-honesty-and-strict-config.md). |
| **O3** | **Agent can only tighten; LLM output is clamped.** The control plane can lower limits but never raise them above the configured ceiling; an LLM returns one action from a closed set and every value is clamped before applying. | `RuleBasedRiskManager::tighten_cap` / `effective_cap`; `HeuristicTuner` ceiling clamps; `Orchestrator::apply` clamps each action. | `agent_cannot_widen_cap_beyond_ceiling`, tuner `never_exceeds_ceiling`, orchestrator `agent_cap_request_is_clamped_to_ceiling` / `agent_params_are_clamped_to_ceiling`. |
| **O4** | **Shared cost model.** Backtester and live engine use the same cost model and config types, so simulated economics cannot be quietly more optimistic than production. | Both consume `lorenz_core::config::CostConfig`; `lorenz-backtest::CostModel` is derived from it. | Type sharing; backtest tests. |
| **O5** | **Auditable decisions.** Every control-plane action is recorded append-only with a rationale. | `DecisionLedger` has no mutate/delete API; `Orchestrator` records on every branch. | `ledger_is_append_only_and_ordered`; orchestrator `every_action_is_recorded_in_order`. |

---

## Threat model (from `../SECURITY.md`)

| Threat | Mitigation | Invariant |
| --- | --- | --- |
| Settling a losing trade | On-chain profit floor; tx reverts | I1, I2 |
| Bot drains user funds | Scoped delegate; withdrawal owner-only | I4 |
| Oversized position | On-chain spend cap | I3 |
| Hidden/expanding fees | Fixed, capped, on-chain fee | I5 |
| Runaway automation | Control plane can only tighten; kill-switch | O3 |
| Misconfiguration | Strict typed config rejects unknown keys | O2 |

### Key handling

- No private keys, mnemonics, or live endpoints are committed; `.gitignore`
  excludes `*.key`, `*.keypair.json`, `id.json`, `.env*`.
- Config types in `lorenz-core` deliberately never hold signing material; signing
  happens at the process edge.

---

## What safety does **not** yet cover (honest limits)

These are real and must not be glossed over:

- The on-chain program is **unaudited** and cannot land a live trade yet (the
  flash-loan and route CPIs are seams). I1's "bounded to fees" property depends on
  the Kamino same-tx repay enforcement that is not yet wired.
- `declare_id!` uses the Anchor **example** program id; a deployment must replace
  it (chapter 06).
- The single-tick CLMM math is `f64` and accurate only within the current tick;
  tick-crossing integer math is roadmap. The detector tolerates this (candidates
  only), but a live executor would need exact CLMM math.
- The off-chain invariants protect *honesty and reproducibility*, not funds — the
  chain remains the only trust boundary that matters with real capital.

Continue to [10 — Roadmap & Seams](10-roadmap-and-seams.md).
