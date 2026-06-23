# Security Policy

> Lorenz Protocol is **experimental and unaudited**. Do not use it with real
> funds. The on-chain program has not been audited and is missing functionality
> (the flash-loan and swap-route CPIs are seams that return `NotImplemented`).
> See [docs/SECURITY.md](docs/SECURITY.md) for the threat model and
> [docs/INVARIANTS.md](docs/INVARIANTS.md) for the guarantees and where each is
> enforced.

## Supported versions

This is a `0.x` reference architecture under active development. Only the latest
`main` is supported; there are no security backports.

## Reporting a vulnerability

**Please do not open a public issue for security-sensitive reports.**

Preferred channel: open a **private GitHub Security Advisory** on the repository
(Security → Advisories → "Report a vulnerability"). This keeps the report
confidential until a fix is ready and avoids exposing users.

Please include:

- a description of the issue and the affected component (crate or the on-chain
  program);
- reproduction steps or a proof of concept;
- the potential impact (e.g. fund loss, invariant bypass, denial of service);
- any suggested remediation.

We will acknowledge the report, investigate, and coordinate a fix and disclosure
timeline with you. Because this software is not intended for use with real funds
in its current state, there is no bug-bounty program.

## Scope notes

- The trust-critical surface is the on-chain program (`programs/executor`) and
  its invariants (I1–I5). Findings that bypass the no-loss profit floor, the
  spend cap, the scoped withdrawal authority, or the fee ceiling are the highest
  priority.
- The off-chain engine is not trust-critical (the chain is the backstop), but
  reports of determinism breaks, integer-overflow/panic paths, or config-parsing
  issues are welcome.
- No private keys, mnemonics, or live endpoints are committed; config types never
  hold signing material. Reports of accidental secret exposure are in scope.
