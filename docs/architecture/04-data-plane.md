# 04 — Data Plane

The data plane is the deterministic hot path:
`ingest → detect → size → build → submit → execute`. This chapter covers the four
host crates that implement the analysis half of it: **`lorenz-amm`** (pricing),
**`lorenz-graph`** (detection), **`lorenz-dex`** (pool model + decoders), and
**`lorenz-stream`** (ingest abstraction). Building/submitting the transaction is
roadmap; executing is the on-chain program (chapter 06).

```mermaid
graph LR
    bytes["raw account bytes"] -->|decode| dex["lorenz-dex::Pool"]
    dex -->|"edges(probe)"| graph["lorenz-graph::ArbitrageGraph"]
    graph -->|find_arbitrage| cycle["Cycle (candidate)"]
    cycle -->|"quote hop-by-hop"| amm["lorenz-amm exact output"]
    amm --> profit["exact, size-aware profit"]
    stream["lorenz-stream::detect_stream"] -.drives.-> graph
```

A recurring theme: **the graph finds *candidates* using `f64` edge rates; exact,
size-aware profit is always recomputed with integer math** before anything is
acted on. The detector narrows the search space; it does not certify profit.

---

## `lorenz-amm` — constant-product math ✅

The smallest fully-real, fully-tested unit of the system. Given reserves and a
fee it returns exactly how much comes out of a swap, in integer arithmetic that
matches what the chain computes at settlement.

### `CpmmReserves`

```
CpmmReserves { reserve_in: u128, reserve_out: u128, fee: Bps }
```

One direction of a constant-product (`x·y=k`) pool.

### `amount_out(amount_in: u128) -> Option<u128>`

Uniswap-v2-style formula with the fee applied to the input:

```text
in_after_fee = amount_in * (10_000 - fee_bps) / 10_000
out          = reserve_out * in_after_fee / (reserve_in + in_after_fee)
```

- All multiplications are `checked_*` against `u128` overflow (returns `None`).
- Degenerate input (`reserve_in == 0 || reserve_out == 0 || amount_in == 0`)
  returns `Some(0)` rather than a misleading panic or `None`.
- Integer floor division means the realized output is exactly the on-chain value.

### Analytics helpers (f64, never settlement)

- `spot_price() -> f64` — marginal price `reserve_out / reserve_in`, ignoring fee
  and impact. Analytics only.
- `effective_rate(amount_in) -> Option<f64>` — `out/in` actually realized for a
  size, including fee + impact. This is what weights graph edges.

### Property tests (the invariants any correct AMM must obey)

`proptest` pins down, over wide random inputs:

| Property | What it guarantees |
| --- | --- |
| `output_bounded_by_reserve` | Output can never exceed the output reserve (`out < r_out`). |
| `monotonic_in_input` | More input never yields strictly less output. |
| `no_free_money_single_hop` | With symmetric reserves, `out ≤ in` for any positive fee/impact — no free money from one hop. |

Plus known-value tests (`1000` in on equal `1_000_000` reserves, zero fee →
`999`), a fee-reduces-output test, and an empty-reserves test.

This crate is invariant **O1** (integer settlement math) in [`../INVARIANTS.md`](../INVARIANTS.md).

---

## `lorenz-graph` — negative-cycle arbitrage detection ✅

Models the market as a directed graph: nodes are tokens, an edge `A → B` means
"you can swap A into B" at effective rate `r` (out per in). Chaining swaps
multiplies rates, so a cycle is profitable exactly when the product of its rates
exceeds 1.

### The transform that makes it tractable

Take `-ln(r)` as the edge weight. The product condition becomes additive: a
profitable cycle is a cycle whose summed weights are **negative**. That is the
classic Bellman-Ford negative-cycle problem.

### Types

| Type | Fields |
| --- | --- |
| `Edge` | `from: TokenId`, `to: TokenId`, `pool: PoolId`, `rate: f64` |
| `Cycle` | `edges: Vec<Edge>` (traversal order, closed loop), `product: f64` (`>1.0` ⇒ gross-profitable) |
| `ArbitrageGraph` | interned `tokens`, `index: HashMap<TokenId,usize>`, `edges` |

### `add_edge`

Interns the endpoints and pushes the edge — but **ignores edges with
non-positive or non-finite rate** (`rate <= 0.0 || !rate.is_finite()`), since
those represent "no path" and would break the `-ln(r)` transform.

### `find_arbitrage() -> Option<Cycle>`

1. Compute weights `-rate.ln()` for every edge.
2. Initialize all distances to `0.0`. This is equivalent to attaching a virtual
   source with zero-weight edges to every node, so a negative cycle **anywhere**
   in the graph is detectable regardless of connectivity.
3. Relax all edges `n` times (`n = token count`), tracking `pred_edge[v]` and the
   last node updated. An epsilon of `1e-12` guards the float comparison. If a
   round relaxes nothing, the graph has converged with no negative cycle → early
   return `None`.
4. A node still relaxing after `n` rounds sits on (or reaches) a negative cycle.
   Walk predecessors `n` times to step *into* the cycle proper.
5. Reconstruct the cycle by following predecessor edges back to the start, with a
   safety valve (`len > n+1 ⇒ None`) against pathological reconstruction.

Returns the cycle with its rate `product`.

### Honest caveat (stated in the crate)

Edge rates are taken at a **single quote size**, so this finds *candidate*
cycles. Exact, size-aware profit is recomputed by the executor/backtester against
real pool math (`lorenz-amm`) before anything is acted on. This module narrows the
search space; it does not certify profit.

Tests cover: balanced rates (product 1) ⇒ none; a triangular arb
(`1·1·1.1 = 1.1`) found and reconstructed as a closed loop; non-positive rates
ignored; empty graph ⇒ none.

---

## `lorenz-dex` — pool model, quoting, decoders ✅ / 🔭

This crate is the convergence point of the data plane. It does three things:

1. Defines the **`Pool`** model and the **`Quoter`** trait.
2. Prices both constant-product (`CpmmPool`) and single-tick concentrated-liquidity
   (`ClmmPool`) pools.
3. Decodes **raw on-chain account bytes** into priceable pool snapshots
   (`decoder` module).

### The `Quoter` trait and `CpmmPool`

```rust
trait Quoter {
    fn quote(&self, token_in: &TokenId, amount_in: u128) -> Option<u128>;
    fn dex(&self) -> Dex;
    fn pool_id(&self) -> &PoolId;
}
```

`CpmmPool { id, dex, token_a, token_b, reserve_a, reserve_b, fee }` orients its
reserves for whichever side is being spent (`reserves_for`) and delegates to
`lorenz-amm`. Its `edges(probe_size) -> Vec<Edge>` emits **both** directed edges
(A→B and B→A) at a probe size, so the detector can consider swapping either way.

### `ClmmPool` — single-tick concentrated liquidity ✅ (f64)

A CLMM pool stores a current `sqrt_price` (Q64.64 fixed point) and active
`liquidity`, not vault reserves. Within the current tick the math is exact and
simple. Let `s = sqrt_price`, `L = liquidity`:

```text
token0 (A) in, dx:  1/s' = 1/s + dx/L   ;  out_B = L * (s - s')   (price down)
token1 (B) in, dy:  s'   = s  + dy/L    ;  out_A = L * (1/s - 1/s') (price up)
```

with the fee applied to the input first (`dx = amount_in * (1 - fee_fraction)`).
`Q64 = 2^64` converts the fixed-point `sqrt_price_x64` to a real.

> **Honest scope:** this is the *single-tick* swap — accurate only while the trade
> does not cross into a neighbouring initialized tick (where `L` changes). That is
> exactly the right tool for the detector (it only needs effective rates to surface
> candidates). The math here uses `f64`; full Q64.64 integer math with tick
> crossing is **roadmap**. The executor/backtester recompute exact output before
> acting.

### The unifying `Pool` enum

```rust
enum Pool { Cpmm(CpmmPool), Clmm(ClmmPool) }
```

`Pool` forwards `edges`, `quote`, `id`, `dex` to the inner variant, so the
streaming and detection pipeline treats both kinds uniformly.

### The `decoder` module — raw bytes → pool snapshot

This is where the deterministic core meets untrusted external data, behind traits
so the live engine, replay, and tests share one definition of "what a pool is".

**How reserves actually work (CPMM):** the pool account does *not* store reserves
— it stores the two vault token-account addresses; the reserves are the SPL
balances in those vaults. Decoding is therefore two steps:

1. `PoolAccountDecoder::decode_accounts(bytes) -> PoolAccounts` reads the pool
   account (mints, vaults, fee).
2. The caller fetches the two vault token accounts, reads each balance with
   `spl::token_account_amount`, and calls `PoolAccounts::assemble(id, reserve_a,
   reserve_b) -> CpmmPool`.

Low-level readers are all bounds-checked (`read_pubkey`, `read_u16`, `read_u128`)
and return `DecodeError::TooShort { expected, got }` on truncation.

#### Implemented decoders (real offsets, fixture-tested) ✅

| Decoder | Kind | Offsets | Fee |
| --- | --- | --- | --- |
| `spl::token_account_amount` | SPL token account | 165-byte account, `amount` u64 at offset **64**, mint at 0 | — |
| `RaydiumAmmV4Decoder` | CPMM | coin_vault 336, pc_vault 368, coin_mint 400, pc_mint 432 | default **25 bps** |
| `RaydiumCpmmDecoder` | CPMM | token0_vault 72, token1_vault 104, token0_mint 168, token1_mint 200 | default **25 bps** (real fee lives in the separate `amm_config` account; caller refines) |
| `WhirlpoolDecoder` | CLMM | (incl. 8-byte Anchor discriminator) fee_rate 45 (u16), liquidity 49 (u128), sqrt_price 65 (u128), mint_a 101, mint_b 181; len 653 | `fee_rate / 100` (Whirlpool fee is hundredths of a bp → bps) |

`PoolAccounts` carries `{ dex, mint_a, mint_b, vault_a, vault_b, fee }`;
`ClmmPoolAccounts` carries `{ dex, mint_a, mint_b, sqrt_price_x64, liquidity, fee }`
and assembles directly (no vault fetch needed).

#### Roadmap decoders (explicit `NotImplemented`) 🔭

Generated by the `roadmap_decoder!` / `roadmap_clmm_decoder!` macros — real types
that openly report `DecodeError::NotImplemented(dex)` rather than misrepresenting
non-CPMM math as constant-product:

- CPMM-shaped seams: `MeteoraDammDecoder`, `PumpAmmDecoder`, `SolfiDecoder`,
  `VertigoDecoder`.
- CLMM seams: `RaydiumClmmDecoder`, `MeteoraDlmmDecoder` (need sqrt-price + tick
  arrays / bin layouts).

`b58(&RawPubkey) -> String` base58-encodes a raw 32-byte key into a `TokenId`
string, keeping the crate dependency-light (no Solana SDK).

---

## `lorenz-stream` — transport-agnostic ingest ✅ / 🟡

Turns a sequence of pool-state updates into detected candidates, independently of
where the bytes come from.

### Core abstraction

```rust
struct PoolSnapshot { slot: u64, pools: Vec<Pool> }   // all watched pools at a slot

trait PoolSnapshotSource {
    fn next_snapshot(&mut self) -> Option<PoolSnapshot>;
}
```

> **Naming note:** this `PoolSnapshot` (runtime: a slot + many pools) is distinct
> from `lorenz-backtest::PoolSnapshot` (a single serializable pool row). See chapter
> 07.

### `detect_stream` — the shared analysis step

```rust
fn detect_stream<S: PoolSnapshotSource, F: FnMut(u64, Cycle)>(
    source: S, probe_size: u128, on_cycle: F
) -> usize
```

For every snapshot it: builds a fresh `ArbitrageGraph`, adds `pool.edges(probe_size)`
for each pool, runs `find_arbitrage`, and invokes `on_cycle(slot, cycle)` on a
hit, returning the count detected. **This is the exact analysis step the live
engine runs**, just fed from an abstract source.

### Two sources

- **`replay::ReplaySource`** ✅ — a real, deterministic source over recorded
  snapshots (a `VecDeque` popped front-to-back). Used in tests and conceptually by
  the backtester; genuinely drives the detector end-to-end.
- **`geyser::GeyserSource`** 🟡 — the production transport **seam** for a live
  Yellowstone/Geyser gRPC subscription. `GeyserConfig { endpoint, x_token,
  commitment }`; `connect()` returns `StreamError::NotImplemented(...)`. The
  module documents the exact wiring (connect → subscribe to pool + vault accounts
  → decode with `lorenz-dex` → assemble → emit snapshot). Only the gRPC transport is
  environment-specific; the decode/assemble steps already exist and are tested.

`StreamError` has `NotImplemented(&'static str)` and `Transport(String)` variants.

The heavyweight `yellowstone-grpc-client` (and its `solana-sdk` transitive) is
**deliberately kept out** of the core workspace so the deterministic crates stay
light and fast to build; it belongs in a deployment build.

Continue to [05 — Control Plane](05-control-plane.md).
