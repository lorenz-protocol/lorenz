# Security

## Status

This software is **experimental and unaudited**. Do not use it with real funds.
The on-chain program has not been audited and is missing functionality (see
`programs/README.md`).

## Threat model (summary)

| Threat | Mitigation | Where |
| --- | --- | --- |
| Settling a losing trade | On-chain profit floor; tx reverts | INVARIANTS I1, I2 |
| Bot drains user funds | Scoped delegate; withdrawal is owner-only | INVARIANTS I4 |
| Oversized position | On-chain spend cap | INVARIANTS I3 |
| Hidden/expanding fees | Fixed, capped, on-chain fee | INVARIANTS I5 |
| Runaway automation | Control plane can only tighten limits; kill-switch | INVARIANTS O3 |
| Misconfiguration | Strict typed config rejects unknown keys | INVARIANTS O2 |

## Key handling

- No private keys, mnemonics or live endpoints are committed. `.gitignore`
  excludes `*.key`, `*.keypair.json`, `id.json`, `.env*`.
- Config types in `lorenz-core` deliberately never hold signing material; signing
  happens at the process edge.

## Reporting a vulnerability

Please open a private security advisory on the repository rather than a public
issue. Include a description, affected component, and reproduction steps.
