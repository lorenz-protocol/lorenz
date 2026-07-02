# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims
to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This is a `0.x` experimental reference architecture: the public API and on-chain
program are not yet stable.

## [Unreleased]

## [0.4.0] - 2026-07-02

### Added
- `lorenz-graph`: `find_disjoint_arbitrage(max_cycles)` — detects multiple
  edge-disjoint profitable cycles from a graph (the single-cycle `find_arbitrage`
  is unchanged).
- `lorenz-backtest`: a `--config <FILE>` flag to load risk and cost parameters
  from an `EngineConfig` TOML file (the file must include an `[rpc]` section,
  which the backtester does not use).
- CI: a non-gating `cargo audit` job that reports RustSec advisories as warnings
  without blocking merges or releases.

### Changed
- `lorenz-backtest`: each snapshot now detects and independently sizes several
  edge-disjoint arbitrage opportunities (up to 8) instead of a single one;
  `candidates_found` counts all of them.

## [0.3.0] - 2026-06-29

### Added
- `lorenz-backtest`: a `--market <FILE>` flag to replay an external market JSON
  file, falling back to the bundled sample when omitted.
- `lorenz-amm`: `optimal_cycle_size` and `maximize_unimodal` — net-profit-maximizing
  trade sizing for an arbitrage cycle via a unimodal (ternary narrowing +
  local-refine) search, with property tests against brute force.
- A "Discrepancy report" issue template and an "adding a seam honestly"
  CONTRIBUTING guide, operationalizing the project's honesty principle.

### Changed
- `lorenz-backtest`: each arbitrage trade is now sized at the net-profit-maximizing
  input (capped by the risk ceiling) instead of a fixed notional.
- Corrected the `lorenz-amm::amount_out` doc to match its actual degenerate-input
  behavior (`Some(0)`), and refreshed the README capabilities list.

### Fixed
- `lorenz-amm`: a fee-complement underflow when a pool's fee exceeded 100%;
  out-of-range fees now return `None` via the checked `Bps::fee_complement`.

## [0.2.0] - 2026-06-28

### Added
- `lorenz-backtest`: a `--json` flag (plus `--help`) on the runnable binary that
  emits the full deterministic backtest result as a single JSON document for
  downstream tooling. Default human-readable output is unchanged.
- `lorenz-amm`: exact-output (inverse) constant-product swap math
  `CpmmReserves::amount_in_for_exact_out` — the minimum input for a desired
  output, integer-exact and rounded so the pool is never short-changed, with
  property tests proving round-trip, minimality, and monotonicity.

## [0.1.0] - 2026-06-26

Initial public reference architecture.

### Added
- `lorenz-core`: shared domain newtypes, strict typed config (`deny_unknown_fields`),
  error type, and telemetry records.
- `lorenz-amm`: constant-product AMM math in integer arithmetic, with property tests.
- `lorenz-graph`: arbitrage detection as negative-cycle Bellman-Ford with cycle
  reconstruction.
- `lorenz-dex`: pool model, CPMM + single-tick CLMM quoting, and real account
  decoders (SPL token accounts, Raydium AMM v4 / CP-Swap, Orca Whirlpool);
  concentrated-liquidity venues that need tick math are honest `NotImplemented`.
- `lorenz-stream`: transport-agnostic pool-update streaming with a deterministic
  replay source; the Geyser/Yellowstone transport is a documented seam.
- `lorenz-backtest`: deterministic replay/backtester with a full cost model and a
  runnable binary over a bundled sample market.
- `lorenz-agent`: control-plane primitives — rule-based risk manager + kill-switch,
  decision ledger, parameter tuner, and an LLM orchestrator that clamps a closed
  action set against hard ceilings.
- `programs/executor`: on-chain atomic executor (Anchor) enforcing the safety
  invariants (atomic-or-revert, no-loss profit floor, spend cap, scoped authority,
  capped transparent fee). The flash-loan CPI and swap route are seams that
  return `NotImplemented`.
- `lorenz-wasm`: WebAssembly bindings exposing the deterministic engine
  (`amount_out`, `find_arbitrage`, `screen`, `run_backtest`) to the browser,
  verified against the native engine.
- Documentation: architecture, invariants, security, and architecture decision
  records.
- Open-source community files: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`,
  top-level `SECURITY.md`, issue/PR templates, and Dependabot config.

### Notes
- Experimental and unaudited. No profitability claims. MIT licensed.

[Unreleased]: https://github.com/lorenz-protocol/lorenz/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/lorenz-protocol/lorenz/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/lorenz-protocol/lorenz/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/lorenz-protocol/lorenz/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/lorenz-protocol/lorenz/releases/tag/v0.1.0
