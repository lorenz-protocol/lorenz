# On-chain programs

This directory holds the Solana programs that enforce Lorenz's hard safety
guarantees on-chain.

## `executor`

Atomic arbitrage executor. It enforces, at runtime:

- atomic-or-revert execution (no partial state, losses bounded to fees),
- a no-loss profit floor (`balance_after >= balance_before + min_profit`),
- a per-vault spend cap,
- owner-only withdrawal (the bot's delegated authority cannot withdraw),
- a transparent, capped protocol fee paid on realized profit.

See [`../docs/INVARIANTS.md`](../docs/INVARIANTS.md) for the full statement of
each invariant and where it is enforced in the code.

### Build status (honest)

The program compiles as a normal Anchor program, but two pieces are **not yet
implemented** and are isolated behind clearly-marked boundaries that currently
return `ExecutorError::NotImplemented`:

- the flash-loan borrow/repay CPI (`mod flash_loan`),
- the multi-DEX swap route (`mod route`).

This means `execute_arbitrage` cannot land a live trade yet. The surrounding
guard, fee and accounting logic is complete and reviewable. We deliberately do
not stub these to fake a success.

### Building

Requires the Solana + Anchor toolchain (`anchor-cli`, `cargo build-sbf`):

```sh
anchor build
```

The program is excluded from the host Cargo workspace so that the off-chain
`cargo test` stays fast and toolchain-free; it is built separately here.

### Before mainnet

This program must pass an external audit before being used with real capital.
It is unaudited and experimental.
