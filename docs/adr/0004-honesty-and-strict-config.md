# ADR 0004 — Honesty as an engineering constraint

- Status: accepted
- Date: 2026-06

## Context

This project grew out of analysing a prior "arbitrage bot" repository that was
indefensible on review: it advertised an anti-MEV/Jito layer and an API that
were never wired into the running binary, quoted fabricated success rates and
revenue, claimed "open-source verifiable / no custody" in contradiction with the
code, and silently ignored whole config sections (`[jito]`,
`[kamino_flashloan]`) because the config struct never read them.

A reviewer — human or AI — does not get impressed by claims; it reads the code
and finds the contradictions.

## Decision

Adopt honesty as a hard constraint, enforced where possible by tooling:

1. No fabricated metrics or claims. Every README statement is verifiable from
   code/chain or is explicitly labelled roadmap.
2. Unimplemented paths return an explicit `NotImplemented` error, never a fake
   success.
3. Config uses `deny_unknown_fields` so a mistyped or unsupported section fails
   loudly instead of being silently dropped (see INVARIANTS O2).

## Consequences

- The repository is defensible *because it is accurate*, which is precisely the
  property that survives close review.
- Some features read as "not done yet"; this is intended and visible.
