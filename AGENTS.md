# AGENTS.md

This repo is `qpayd`: a small open-source Bitcoin and Lightning payment daemon.
Treat this file as the local operating guide for future coding agents.

## Product Direction

- Optimize for complete, correct, simple glueware that leans on proven Bitcoin,
  Lightning, database, HTTP, and serialization libraries.
- Bias toward production-ready behavior before calling work done. For payments,
  this means durability, idempotency, replay, signatures, clear status semantics,
  tested failure paths, and no broad secrets.
- Keep qpayd deployable anywhere. Do not make the daemon or README about a
  single host. Fast warm start matters, but provider-specific deployment notes
  should stay out of core usage docs unless explicitly requested.
- Support many stores. Store configuration may be local config now and remote DB
  config later; avoid designs that assume one store forever.
- Fiat pricing is important. Use configured third-party pricing sources, with
  Kraken as the default source unless the project changes direction.
- Bitcoin plus Lightning both matter. Keep on-chain and Lightning behavior
  explicit and testable.

## Workflow

- Use pull requests for future changes, even when they will be merged quickly.
  PRs are the audit trail and feed generated GitHub release notes.
- Keep releases tag-driven. `v*` tags run `.github/workflows/release.yml`, which
  verifies the build and creates a GitHub Release with generated notes.
- For release-note quality, write PR titles and summaries as user-visible change
  descriptions. Label internal-only changes `ignore-for-release` when useful.
- Do not commit unrelated local changes. In particular, `fly/` is an ignored
  local deployment workspace and may have its own repo state.
- The user is comfortable with frequent commits and pushes, but make them
  coherent. Prefer small PRs with a clear reason over large mixed changes.

## Engineering Standards

- Read the existing code before changing it. Follow local patterns unless there
  is a concrete reason to introduce a new one.
- Keep the daemon boring: no app code on payment infrastructure, no broad API
  keys in dependent apps, durable data, tested backups/migrations when relevant,
  and clear update policy.
- Prefer structured storage and APIs over stringly ad hoc behavior.
- Payment handling must be idempotent. Webhooks should be signed, persisted,
  retried, and replayable.
- Avoid inline network calls on checkout-critical paths when a durable queue is
  the correct shape.
- Add tests at the level of the risk. Payment state transitions and persistence
  behavior should have focused tests.
- Before pushing code changes, run:

```sh
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
```

- For release-sensitive changes, also run:

```sh
cargo build --release --locked
```

## Documentation Standards

- README is for how to use qpayd, not for dumping specs or caveats.
- If docs need a caveat because the software is incomplete, prefer fixing the
  software. If there is a real operational constraint, document the concrete
  action the user should take.
- Write for real people with busy wallets and limited Bitcoin descriptor
  context. Avoid assuming users understand derivation indexes, script types,
  account paths, or wallet internals unless the docs explain exactly what to do.
- Prefer end-to-end verification steps over abstract correctness checks. For
  example, tell users to create a small invoice, send a tiny payment, reconcile,
  and verify `settled`.
- Keep provider-specific deployment details out of generic usage docs unless the
  section is explicitly about that provider.

## Communication Style

- Be direct, concrete, and pragmatic. Avoid cheerleading and vague reassurance.
- The user wants questions answered before coding when they are still choosing a
  direction. Once a direction is set, keep moving through implementation,
  verification, PR, and merge when feasible.
- Challenge unclear or brittle payment architecture, but explain the technical
  reason and propose the cleaner path.
- Do not treat qpayd as a SaaS product unless explicitly asked. The public/private
  split is the operator's business; the software should stay open-source and
  generally useful.
