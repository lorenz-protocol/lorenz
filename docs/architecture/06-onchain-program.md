# 06 — On-chain Program (`programs/executor`)

The on-chain executor is **the single source of the platform's hard safety
guarantees**. They are enforced by the Solana runtime, not by the off-chain bot,
which is what makes the system auditable: a reviewer can read one file and the
chain will refuse anything that violates it.

- Source: `programs/executor/src/lib.rs` (Anchor program `lorenz_executor`).
- Built with the Solana/Anchor toolchain (`anchor build`), **excluded** from the
  host workspace (chapter 02).
- `declare_id!("E5Mj85eZNW9s2fhceFT56sZFQezXiVNJ7QKZv5vp1RxA")`.

> **Honest note on the program id:** `E5Mj85e…p1RxA` is a fresh placeholder id
> with **no published keypair** — it is not a deployable program. Before
> deploying, a deployer must generate their own keypair and sync it (`anchor keys
> sync`), then update `declare_id!` and `Anchor.toml` to the resulting address.
> This is consistent with the repo's "no faked production state" posture, but
> worth flagging explicitly.

> **Build status (honest):** the program compiles as a normal Anchor program, but
> the flash-loan borrow/repay CPI (`mod flash_loan`) and the multi-DEX swap route
> (`mod route`) are **not implemented** — they return `ExecutorError::NotImplemented`.
> So `execute_arbitrage` cannot land a live trade yet. The surrounding guard, fee,
> and accounting logic is complete and reviewable; the unimplemented parts are not
> stubbed to fake success. See [ADR 0003](../adr/0003-custom-onchain-program.md).

## Constants

- `MAX_FEE_BPS: u16 = 1_000` — hard ceiling: protocol fee ≤ 10%.
- `flash_loan::KAMINO_LENDING_PROGRAM_ID = "KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD"`
  — the documented integration target (mainnet Kamino Lending).

## State: the `Vault` account

A per-owner PDA at seeds `[b"vault", owner]`.

| Field | Type | Purpose |
| --- | --- | --- |
| `owner` | `Pubkey` | The only key that may `withdraw`. |
| `bump` | `u8` | PDA bump. |
| `base_mint` | `Pubkey` | The denominating asset. |
| `vault_token_account` | `Pubkey` | Where the loaned/realized balance lives. |
| `protocol_fee_account` | `Pubkey` | Fixed destination for the protocol fee. |
| `spend_cap` | `u64` | Hard per-arb notional ceiling (I3). |
| `fee_bps` | `u16` | Protocol fee on realized profit (≤ `MAX_FEE_BPS`). |
| `cumulative_profit` | `u64` | Running owner profit (post-fee), `saturating_add`. |

`Vault::SPACE = 8 + 32 + 1 + 32 + 32 + 32 + 8 + 2 + 8` (Anchor discriminator +
fields). Parameters are immutable after creation except via explicit owner-signed
instructions (not yet exposed), keeping the trust surface small.

## Instructions

### `initialize_vault(spend_cap, fee_bps)`

Creates the vault PDA owned by the signer. `require!(fee_bps <= MAX_FEE_BPS,
FeeTooHigh)` enforces the fee ceiling at creation; records the token / fee /
mint accounts and zeroes `cumulative_profit`.

Accounts (`InitializeVault`): `owner` (signer, payer), the `init` `vault` PDA,
`base_mint` (unchecked, validated via the token account), `vault_token_account`
(`token::authority = vault`), `protocol_fee_account`, plus token/system programs.

### `execute_arbitrage(notional, min_profit)`

The atomic cycle. Signed by the **`bot_authority`** delegate (not the owner).

```text
require!(notional <= vault.spend_cap, SpendCapExceeded);     // I3
balance_before = vault_token_account.amount;                 // I2 baseline

flash_loan::borrow(&ctx, notional)?;     // ROADMAP -> NotImplemented
route::execute_route(&ctx, notional)?;   // ROADMAP -> NotImplemented
flash_loan::repay(&ctx, notional)?;      // ROADMAP -> NotImplemented

vault_token_account.reload()?;
balance_after = vault_token_account.amount;

profit = balance_after.checked_sub(balance_before)
            .ok_or(ArbNotProfitable)?;                       // a loss underflows
require!(profit >= min_profit, ArbNotProfitable);            // I2

fee = math::protocol_fee(profit, vault.fee_bps);             // I5
if fee > 0 { token::transfer(... vault -> protocol_fee_account, fee) }  // PDA-signed
vault.cumulative_profit = saturating_add(profit - fee);
emit!(ArbitrageExecuted { vault, notional, profit, fee });
```

Because the borrow/route/repay calls currently return `NotImplemented`, the
instruction reverts early today; the guard arithmetic (I2/I3/I5) around them is
nonetheless fully present and unit-testable.

Accounts (`ExecuteArbitrage`): `bot_authority` (signer), the `mut` `vault` PDA,
`vault_token_account` (`address = vault.vault_token_account`, `token::authority =
vault`), `protocol_fee_account` (`address = vault.protocol_fee_account`), the
token program, and **`remaining_accounts`** which encode the DEX route (consumed
by the roadmap `route::execute_route`).

### `withdraw(amount)`

Owner-only. The fee/PDA transfer is signed by the vault PDA
(`seeds = [b"vault", owner, bump]`). Accounts (`Withdraw`): `owner` (signer),
the `vault` PDA with **`has_one = owner`**, `vault_token_account`, an arbitrary
`destination` token account, the token program.

## Pure arithmetic: `mod math` ✅

Kept free of Anchor types so it is unit-testable with a plain `cargo test` in the
crate — this is what CI exercises.

- `protocol_fee(profit: u64, fee_bps: u16) -> u64` = `profit * fee_bps / 10_000`,
  computed via a `u128` intermediate to avoid overflow; the result fits in `u64`
  because it is a fraction of `profit`.
- `clears_profit_floor(before, after, min_profit) -> bool` — the I2 predicate; a
  loss (`after < before`) underflows to `false`.

Tests: fee is a correct fraction (`1%` of `1_000_000` = `10_000`); fee never
exceeds profit even at the 10% ceiling on `u64::MAX/2`; the floor predicate
accepts a clearing trade and rejects both a short and a loss.

## The roadmap seams 🟡

### `mod flash_loan` (Kamino Lending CPI)

`borrow` / `repay` return `NotImplemented`. The integration shape is documented:
Kamino exposes `flash_borrow_reserve_liquidity` / `flash_repay_reserve_liquidity`;
a flash-loan transaction must contain a matching borrow/repay pair and the runtime
enforces repayment + fee within the same transaction or the whole transaction
reverts — **which is exactly what bounds the downside to fees (I1).** The required
accounts (lending market, reserve, reserve liquidity supply, the vault's
destination token account, the instructions sysvar used to validate pairing) are
passed via `remaining_accounts`.

### `mod route` (multi-DEX swap CPIs)

`execute_route` returns `NotImplemented`. Roadmap: walk the route encoded in
`remaining_accounts`, issuing a swap CPI per hop (Raydium, Meteora, Whirlpool, …),
mirroring the off-chain `lorenz-dex` integrations.

## Events and errors

`#[event] ArbitrageExecuted { vault, notional, profit, fee }`.

`ExecutorError`: `FeeTooHigh`, `SpendCapExceeded`, `ArbNotProfitable`,
`MathOverflow`, `NotImplemented`.

## How the five invariants map here

| Invariant | Enforced by |
| --- | --- |
| **I1** Atomic-or-revert | Single-instruction design + Solana tx atomicity (and Kamino's same-tx repay requirement once wired). |
| **I2** No loss | `profit = checked_sub(...)` + `require!(profit >= min_profit)`. |
| **I3** Bounded spend | `require!(notional <= vault.spend_cap)`. |
| **I4** Scoped authority | Separate `ExecuteArbitrage` (signed by `bot_authority`) and `Withdraw` (signed by `owner`, `has_one = owner`) contexts — the delegate can never reach `withdraw`. |
| **I5** Transparent capped fee | `require!(fee_bps <= MAX_FEE_BPS)` at init + the fixed-destination fee transfer. |

Full statements and verification notes: [09 — Safety & Invariants](09-safety-and-invariants.md)
and [`../INVARIANTS.md`](../INVARIANTS.md).

## Before mainnet

The program is **unaudited and experimental** and must pass an external audit
before any use with real capital (see `programs/README.md` and
[`../SECURITY.md`](../SECURITY.md)).

Continue to [07 — Backtester](07-backtester.md).
