# ADR 0001 — Separate data plane from control plane

- Status: accepted
- Date: 2026-06

## Context

Arbitrage on Solana is decided in microseconds. We also want an "agentic" layer
that reasons about strategy, risk and parameters. An LLM call takes seconds.
Putting any such reasoning on the execution path would make us lose every race.

## Decision

Split the system into two planes with a one-way control relationship:

- The **data plane** is deterministic, has no AI, and runs the detect -> size ->
  build -> submit loop.
- The **control plane** consumes telemetry and emits parameters and *tighter*
  limits. It never signs hot-path transactions.

## Consequences

- The agent layer can be slow, fallible, and even swapped for an LLM without
  endangering execution, because the safety properties live in the data plane
  and on-chain program, not in the agent.
- Determinism in the data plane makes every trade replayable from logs.
- The interface between planes is a small, typed contract (params + limits in,
  trade records out).
