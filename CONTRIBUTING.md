# Contributing to Lorenz Protocol

Thanks for your interest. Lorenz Protocol is an experimental, open-source
reference architecture for deterministic, non-custodial on-chain arbitrage on
Solana. Contributions are welcome — provided they uphold the project's defining
constraint: **honesty**.

## The one non-negotiable rule: honesty

This project exists partly as a reaction to "arbitrage bot" repositories that
advertise unwired features and fabricated metrics. Every claim in this repo must
be verifiable from the code or the chain, or be explicitly labelled roadmap. See
[ADR 0004](docs/adr/0004-honesty-and-strict-config.md).

Concretely, a contribution must not:

- add a performance/profitability number that isn't reproducible from the code;
- present a stub as working — unimplemented paths return an explicit
  `NotImplemented` error (`ExecutorError::NotImplemented`, `DecodeError::NotImplemented`,
  `StreamError::NotImplemented`), never a faked success;
- silently widen a safety limit or weaken an invariant (see
  [docs/INVARIANTS.md](docs/INVARIANTS.md)).

If something is a roadmap item, label it as one (🔭). If it's a typed
integration seam, keep it that way (🟡) until it's genuinely wired.

### Adding a seam honestly

A seam is unfinished work wired up the honest way. The pattern:

1. Define a real, compiling type with the right signature — not a placeholder
   that the compiler ignores.
2. Have it return an explicit `NotImplemented` error
   (`ExecutorError::NotImplemented`, `DecodeError::NotImplemented`,
   `StreamError::NotImplemented`), never a fabricated success or a silent no-op.
3. Mark it with a doc comment identifying it as a seam (🟡) so it shows up as
   unfinished, not as a working feature.
4. Add a test asserting the path returns `NotImplemented`, so the seam stays a
   seam until someone genuinely wires it.

See [docs/architecture/10-roadmap-and-seams.md](docs/architecture/10-roadmap-and-seams.md)
for the catalogue of current seams and the conventions around them.

## Design principles to respect

- **Determinism in the data plane.** No floating-point money: settlement amounts
  are integer (`u64`/`u128`). `f64` is allowed only for analytics/edge-weights,
  never for amounts that settle. (`lorenz-amm` property tests pin this down.)
- **No AI on the hot path.** The control plane (`lorenz-agent`) decides *limits*
  and *parameters* and may only ever **tighten** them; it never signs hot-path
  transactions. An LLM may only return one action from a closed, clamped set.
- **The chain is the backstop.** Safety guarantees live in `programs/executor`
  and the type system, not in promises. Don't move a guarantee out of the runtime.
- **Strict config.** `#[serde(deny_unknown_fields)]` everywhere — a typo must fail
  loudly, not be silently ignored.

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and the deep-dive in
[docs/architecture/](docs/architecture/README.md) before a substantial change.

## Development setup

The off-chain workspace needs no Solana toolchain:

```sh
cargo test --workspace          # full off-chain suite
cargo run -p lorenz-backtest    # deterministic backtest on the bundled market
cargo fmt --all -- --check      # formatting (CI enforces)
cargo clippy --workspace --all-targets   # CI runs with -D warnings
```

The on-chain program is built separately (it's excluded from the host workspace):

```sh
cd programs/executor && cargo test --lib   # pure-logic unit tests
anchor build                                # requires anchor-cli + cargo build-sbf
```

The browser engine (WASM):

```sh
wasm-pack build crates/lorenz-wasm --target web --out-dir pkg
node crates/lorenz-wasm/smoke_test.mjs      # verifies it matches the native engine
```

## Pull requests

1. Keep changes focused; one logical change per PR.
2. Add or update tests. New AMM/graph/decoder logic should come with unit or
   property tests; new invariants belong in `docs/INVARIANTS.md` with a test that
   verifies them.
3. `cargo fmt`, `cargo clippy` (no warnings), and `cargo test --workspace` must
   pass. CI enforces all three plus a backtest smoke run.
4. Update docs when behaviour changes. If you change the on-chain program,
   update `docs/INVARIANTS.md` and note the audit implications.
5. Fill in the PR template.

## Anonymity / privacy

This is a pseudonymous project. Please do not introduce personally-identifying
information (emails, real names, private endpoints, keys) into the code, commits,
or docs. Never commit secrets — `.gitignore` already excludes `*.key`,
`*.keypair.json`, `id.json`, and `.env*`, and config types deliberately never
hold signing material.

## Reporting bugs / requesting features

Open an issue using the templates. For security-sensitive reports, follow
[SECURITY.md](SECURITY.md) instead of opening a public issue.

## License

By contributing, you agree your contributions are licensed under the project's
[MIT License](LICENSE).
