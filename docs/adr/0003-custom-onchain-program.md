# ADR 0003 — Build a custom on-chain executor (phased)

- Status: accepted
- Date: 2026-06

## Context

The atomic arbitrage can either reuse an existing third-party on-chain program
or be a custom program we build and audit. Reuse is faster but means a
third-party dependency, an opaque fee, and no defensible/auditable core. A
custom program is the opposite trade-off.

## Decision

Commit to a **custom** program as the destination, but **phased**:

1. Validate the economic edge first (backtester + detector) before spending on
   development and audit.
2. Build the custom Anchor program with the invariants in INVARIANTS.md.
3. Audit externally before any mainnet use with real capital.

The flash-loan CPI and swap route are isolated behind boundaries that currently
return `NotImplemented`, so the guard/fee logic can be reviewed independently of
the integrations.

## Consequences

- Auditability and defensibility live in code we control.
- No hidden third-party fee.
- Higher cost and longer timeline; mitigated by phasing and by keeping the
  unimplemented parts explicit rather than faked.
