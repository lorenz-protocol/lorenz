<!-- Thanks for contributing to Lorenz Protocol. Keep PRs focused: one logical change. -->

## What & why

<!-- What does this change, and why? Link any related issue (#123). -->

## Type of change

- [ ] Bug fix
- [ ] New feature / wired-up seam
- [ ] Refactor (no behaviour change)
- [ ] Docs only
- [ ] On-chain program change (⚠️ affects invariants / audit surface)

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets` is clean (CI uses `-D warnings`)
- [ ] `cargo test --workspace` passes; I added/updated tests for this change
- [ ] `cargo run -p lorenz-backtest` still runs (if the data plane changed)
- [ ] If I touched the on-chain program: `cd programs/executor && cargo test --lib` passes, and I updated `docs/INVARIANTS.md`
- [ ] If I touched the WASM API: `wasm-pack build crates/lorenz-wasm` + the smoke test pass

## Honesty & safety

- [ ] No fabricated metrics or unverifiable claims; unimplemented paths return `NotImplemented`, not fake success
- [ ] No safety limit was silently widened and no invariant was weakened
- [ ] No PII, secrets, real names, private endpoints, or keys are introduced

## Notes for reviewers

<!-- Anything reviewers should focus on, trade-offs, or follow-ups. -->
