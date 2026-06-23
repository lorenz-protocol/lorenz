# ADR 0002 — Non-custodial vault with scoped delegate

- Status: accepted
- Date: 2026-06

## Context

We want to offer the system to other users (multi-tenant) without becoming a
custodian of their funds, which carries severe legal and trust implications and
is the pattern that makes many "arbitrage products" look like scams.

## Decision

Each user owns a vault PDA. The bot receives a **scoped delegate** that can only
invoke `execute_arbitrage`; it can never withdraw. Funds can move in exactly two
ways: a profit-bearing atomic round trip back into the vault, or an
owner-signed withdrawal.

Because arbitrage is funded by a flash loan, users do not need to deposit
trading capital — only a small buffer for fees/tips.

## Consequences

- We are not a custodian: withdrawal authority never leaves the owner.
- The user's downside is bounded and provable on-chain (see INVARIANTS I1-I4).
- Revenue is taken on-chain as a transparent fee on realized profit (I5), with
  no off-chain invoicing or trust.
- A compliance review is still required before public multi-tenant launch
  (tracked as Phase 4).
