# Invariants

These are the guarantees Lorenz enforces. Each lists *what* it guarantees, *where*
it is enforced, and *how* it is verified. The point of writing them down is that
a reviewer can check each one against the code.

## On-chain invariants (`programs/executor/src/lib.rs`)

These hold at runtime regardless of the off-chain bot's behaviour. This is the
core of the auditability story: the chain refuses violations.

### I1 — Atomic-or-revert
The full arbitrage (borrow, swap route, repay, profit check, fee) runs inside a
single instruction. If any step or the profit check fails, the whole
transaction reverts. The maximum loss on a failed attempt is therefore the
network/priority fees and any Jito tip — never the notional.
- Enforced by: single-instruction design of `execute_arbitrage` + Solana
  transaction atomicity.

### I2 — No loss (profit floor)
Settlement requires `balance_after >= balance_before + min_profit`, with
`min_profit >= 0`.
- Enforced by: the `profit = balance_after.checked_sub(balance_before)` /
  `require!(profit >= min_profit, ...)` block in `execute_arbitrage`.

### I3 — Bounded spend
`notional <= vault.spend_cap`.
- Enforced by: `require!(notional <= vault.spend_cap, SpendCapExceeded)`.

### I4 — Scoped authority
The bot authority can invoke `execute_arbitrage` but can never withdraw funds.
Only the vault owner can call `withdraw`.
- Enforced by: separate `ExecuteArbitrage` (signed by `bot_authority`) and
  `Withdraw` (signed by `owner`, with `has_one = owner`) account contexts.

### I5 — Transparent, capped fee
The protocol fee is exactly `fee_bps` of realized profit, `fee_bps <= 1000`
(10% hard ceiling), paid on-chain to a fixed account recorded at vault init.
- Enforced by: `require!(fee_bps <= MAX_FEE_BPS, ...)` at init and the fee
  transfer in `execute_arbitrage`.

## Off-chain invariants (`crates/`)

These keep the engine honest and reproducible even though they are not
trust-critical (the chain is the backstop).

### O1 — Integer settlement math
Output amounts are computed in integer arithmetic; floating point is used only
for analytics/edge-weights, never for amounts that settle.
- Verified by: `lorenz-amm` property tests (`output_bounded_by_reserve`,
  `monotonic_in_input`, `no_free_money_single_hop`).

### O2 — Strict config
Unknown configuration keys are rejected, so a typo cannot silently disable a
feature.
- Verified by: `lorenz_core::config` `deny_unknown_fields` + the
  `unknown_key_is_rejected` test. (This is a direct fix for a real failure mode
  observed in a prior project; see ADR 0004.)

### O3 — Agent can only tighten, and LLM output is clamped
The control plane can lower limits but never raise them above the configured
ceiling. An LLM may only return one action from a closed set, and the
orchestrator clamps every value before applying it: a hallucinated request to
widen a cap or raise a fee is clamped to the ceiling, not honored.
- Verified by: `RuleBasedRiskManager::tighten_cap` clamping
  (`agent_cannot_widen_cap_beyond_ceiling`), `HeuristicTuner` ceiling tests, and
  the orchestrator tests (`agent_cap_request_is_clamped_to_ceiling`,
  `agent_params_are_clamped_to_ceiling`).

### O4 — Shared cost model
The backtester and the live engine use the same cost model and config types, so
simulated economics cannot be quietly more optimistic than production.
- Verified by: both consuming `lorenz_core::config::CostConfig`.

### O5 — Auditable decisions
Every control-plane action is recorded append-only with a rationale.
- Verified by: `DecisionLedger` has no mutate/delete API; `ledger` tests.
