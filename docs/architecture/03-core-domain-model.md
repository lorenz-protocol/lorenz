# 03 — Core Domain Model (`lorenz-core`)

`lorenz-core` is the shared vocabulary of the whole platform. Its design rule, from
the crate docs:

> This crate must stay free of any I/O, RPC client or on-chain dependency. It is
> the deterministic vocabulary that both the data plane and the control plane
> agree on, which is what makes traces replayable.

Module map (`crates/lorenz-core/src/`):

| Module | Contents |
| --- | --- |
| `types` | Domain newtypes and the `Dex` enum |
| `config` | Typed, strict `EngineConfig` and sub-configs |
| `error` | The single library error type and `Result` alias |
| `telemetry` | `tracing` bootstrap + `Hop` / `TradeRecord` records |
| `lib` | Re-exports `error::{Error, Result}` |

## Newtypes (`types.rs`)

The crate uses newtypes instead of bare `u64`/`String` so the compiler prevents
mixing, say, a token amount with a basis-point value. Every quantity has a name
and a unit.

| Type | Underlying | Meaning & notes |
| --- | --- | --- |
| `TokenId(String)` | opaque string | A token mint. Kept as a string so the crate needs no Solana SDK; data-plane crates parse it into a real `Pubkey` at the edge. `From<&str>`, `Display`. |
| `PoolId(String)` | opaque string | A liquidity pool / pair account id. `From<&str>`, `Display`. |
| `Amount(u64)` | `u64` | A raw token amount in **base units** (lamports for SOL, smallest unit for SPL). Always integer — no floating-point money. Has `Amount::ZERO` and `as_u64()`. |
| `Bps(u32)` | `u32` | Basis points (1 bp = 0.01%). `DENOMINATOR = 10_000`. `as_fraction() -> f64` is for analytics/logging only, **never** for settlement. |

### The `Dex` enum

Identifies which DEX a pool belongs to (`#[serde(rename_all = "snake_case")]`):

```
RaydiumAmm | RaydiumClmm | RaydiumCpmm | MeteoraDlmm | MeteoraDamm
| Whirlpool | PumpAmm | Solfi | Vertigo
```

This list mirrors the integration targets; see chapter 04 / `lorenz-dex` for the
**decoder status** of each (only Raydium AMM v4, Raydium CPMM, and Whirlpool are
decoded today; the rest are `NotImplemented`).

### Why newtypes matter here

`Amount` and `Bps` both wrap integers but are different units; the type system
refuses to add a fee (`Bps`) to a balance (`Amount`). `Bps::as_fraction()` exists
but is documented as analytics-only, reinforcing the determinism principle — the
floating-point path is quarantined away from settlement math.

## Typed, strict config (`config.rs`)

The configuration model uses `#[serde(deny_unknown_fields)]` on every struct, so
a typo or unsupported section **fails loudly** instead of being silently ignored.
This is a direct lesson recorded in [ADR 0004](../adr/0004-honesty-and-strict-config.md):
a prior project silently dropped whole config sections (`[jito]`,
`[kamino_flashloan]`) because the struct never read them.

```
EngineConfig
├── rpc:   RpcConfig
├── risk:  RiskConfig
└── costs: CostConfig
```

### `RpcConfig`

| Field | Type | Notes |
| --- | --- | --- |
| `url` | `String` | Read endpoint (account/stream subscriptions). |
| `geyser_url` | `Option<String>` | Optional Yellowstone/Geyser gRPC endpoint (`#[serde(default)]`). |

> **No signing material lives in config types.** Private keys/mnemonics are
> loaded at the process edge and never serialized. This is both a design rule and
> a security property (see [`../SECURITY.md`](../SECURITY.md)).

### `RiskConfig` — the hard ceilings

| Field | Type | Meaning |
| --- | --- | --- |
| `max_position` | `u64` | Maximum notional per arbitrage, in base units of the loaned asset. The agent may tighten but never exceed this. |
| `min_profit` | `u64` | Minimum net profit (after costs) required to fire, in base units. |
| `max_consecutive_losses` | `u32` | Consecutive failed/loss trades before the kill-switch trips. |

These are the values the control plane treats as ceilings and the on-chain
program mirrors (spend cap, profit floor).

### `CostConfig` — the shared cost model inputs

| Field | Type | Meaning |
| --- | --- | --- |
| `flash_loan_fee` | `Bps` | Flash-loan provider fee. |
| `priority_fee_lamports` | `u64` | Priority fee per transaction. |
| `jito_tip_lamports` | `u64` | Jito bundle tip. |
| `slippage_per_hop` | `Bps` | Assumed extra slippage per hop on top of pool math. |

`CostConfig` is consumed by **both** the backtester (`lorenz-backtest::CostModel`)
and is intended for the live engine, so simulated economics use the same numbers
as production accounting (invariant O4).

### Parsing

`EngineConfig::from_toml(&str) -> Result<EngineConfig>` parses TOML and returns a
descriptive `Error::Config` on failure rather than panicking. Tests confirm a
valid sample parses and that an unknown `[jito]` section is rejected.

## Error type (`error.rs`)

A single `thiserror`-derived enum for the deterministic core, with
`pub type Result<T> = std::result::Result<T, Error>`:

| Variant | Message |
| --- | --- |
| `Config(String)` | `configuration error: …` |
| `InvalidPool(String)` | `invalid pool state: …` |
| `Overflow(&'static str)` | `arithmetic overflow in …` |
| `NoCycle` | `no arbitrage cycle found` |
| `RiskViolation(String)` | `risk limit violated: …` |

Note that `lorenz-dex` and `lorenz-stream` define their **own** local error types
(`DecodeError`, `StreamError`) rather than overloading this one, keeping each
boundary's failure modes precise.

## Telemetry & records (`telemetry.rs`)

This module is the observability bootstrap plus the structured records that make
every run replayable and auditable.

- `init_tracing() -> Result<(), SetGlobalDefaultError>` — initializes a
  structured `tracing` subscriber driven by `RUST_LOG` (defaults to `info`).
  Idempotent-friendly: callers in tests can ignore the error if a global
  subscriber already exists.

- **`Hop`** — one hop of an arbitrage route: `pool: PoolId`, `token_in`,
  `token_out`, `amount_in: Amount`, `amount_out: Amount`.

- **`TradeRecord`** — an immutable record of a decision the engine made,
  persisted append-only so any run can be reconstructed:

  | Field | Type | Meaning |
  | --- | --- | --- |
  | `ts` | `u64` | Unix timestamp (seconds). |
  | `route` | `Vec<Hop>` | The hops walked. |
  | `notional` | `Amount` | Notional borrowed for the cycle. |
  | `gross_profit` | `i128` | Gross output minus notional, before fees (signed). |
  | `net_profit` | `i128` | Net profit after the full cost model (signed). |
  | `submitted` | `bool` | Whether the engine actually submitted (vs simulated/skipped). |

`TradeRecord` is the **up-channel** payload of the data/control contract: it is
what the backtester emits and what a live telemetry pipeline would feed to the
control plane.

Continue to [04 — Data Plane](04-data-plane.md).
