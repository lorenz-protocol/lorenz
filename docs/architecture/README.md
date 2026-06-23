# Lorenz Protocol Architecture Documentation

A thorough, code-grounded description of the Lorenz system **as it exists today**.
Every statement here is meant to be checkable against the source; where a piece
is unfinished it is labelled **roadmap / seam**, never described as working.

> Lorenz Protocol is an experimental, unaudited reference architecture for atomic,
> flash-loan-funded arbitrage on Solana. It does not trade live and makes no
> profitability claims. See [`../../DISCLAIMER.md`](../../DISCLAIMER.md).

## How this set relates to the existing docs

The repository root `docs/` already contains three high-level documents. This
`architecture/` folder is the **deep reference** that expands them:

| Existing doc | Relationship |
| --- | --- |
| [`../ARCHITECTURE.md`](../ARCHITECTURE.md) | Executive summary of the two-plane split. Expanded by chapters 01, 04, 05, 08. |
| [`../INVARIANTS.md`](../INVARIANTS.md) | Canonical list of guarantees. Cross-referenced and explained in chapter 09. |
| [`../SECURITY.md`](../SECURITY.md) | Threat model + disclosure. Cross-referenced in chapters 06 and 09. |
| [`../adr/`](../adr/) | The four decision records. Referenced throughout. |

## Reading guide

Start at **01** for the mental model, then read by interest:

| # | Chapter | What it covers |
| --- | --- | --- |
| 01 | [System Overview](01-system-overview.md) | What Lorenz is, the data/control plane model, design principles, status legend |
| 02 | [Crate Topology](02-crate-topology.md) | Workspace layout, dependency graph, why the on-chain program is excluded |
| 03 | [Core Domain Model](03-core-domain-model.md) | `lorenz-core`: newtypes, typed config, errors, telemetry records |
| 04 | [Data Plane](04-data-plane.md) | `lorenz-amm`, `lorenz-graph`, `lorenz-dex`, `lorenz-stream` — detect → size pipeline |
| 05 | [Control Plane](05-control-plane.md) | `lorenz-agent`: risk manager, tuner, ledger, LLM boundary, orchestrator |
| 06 | [On-chain Program](06-onchain-program.md) | `programs/executor`: instructions, accounts, the five on-chain invariants |
| 07 | [Backtester](07-backtester.md) | `lorenz-backtest`: cost model, replay loop, the runnable binary |
| 08 | [End-to-End Flows](08-end-to-end-flows.md) | Sequence diagrams for detection, backtest, and a (roadmap) live trade |
| 09 | [Safety & Invariants](09-safety-and-invariants.md) | Where each guarantee is enforced and how it is verified |
| 10 | [Roadmap & Seams](10-roadmap-and-seams.md) | Every `NotImplemented` boundary and what a production build adds |
| 11 | [Glossary](11-glossary.md) | Terms: notional, CPMM/CLMM, Geyser, flash loan, Jito tip, … |

## The system in one paragraph

A deterministic **data plane** ingests pool state, models the market as a token
graph, detects profitable cycles (negative-cycle Bellman-Ford), and prices them
with exact integer AMM math. An off-hot-path **control plane** consumes telemetry
and may only *tighten* limits — an LLM, if present, can return only one action
from a closed, clamped set. The final backstop is a custom **on-chain program**
that refuses to settle any trade that loses money or exceeds its spend cap.
Safety is a property of the runtime and the type system, not a promise in a
README.

## Status legend

Used consistently across all chapters:

- ✅ **Implemented + tested** — real code with unit/property tests.
- 🟡 **Seam** — a typed, documented integration point that compiles and returns
  an explicit `NotImplemented`, so the surrounding logic is reviewable.
- 🔭 **Roadmap** — described and scoped, not present in code yet.

## Conventions

- File paths are relative to the repository root (`lorenz/`).
- Symbols are written as `module::Type` and map to the crate of the same prefix
  (`risk::RuleBasedRiskManager` lives in `crates/lorenz-agent/src/risk.rs`).
- "Base units" always means the integer smallest unit of a token (lamports for
  SOL). No floating-point value ever settles.
