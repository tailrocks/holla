# Plan completion audit

Plans 001–015 were re-audited against the repository on 2026-07-17. Every
written requirement and done criterion is implemented. The achieved plan files
were then removed; this ledger preserves the verification evidence.

| Plan | Verified implementation |
|---|---|
| 001 | `PRODUCT.md` contains mission, platform scope, domain vocabulary, flows, safety invariants, references, and roadmap; `README.md` links it. |
| 002 | Probe/menu characterization exists; child repositories are the single source of truth; Oh My Zsh uses its directory/script; Git commands use argv; compose logs are bounded. |
| 003 | Launcher lifecycle, list, theme roles, keymap hints, and terminal restoration use TermRock. No direct raw-mode or alternate-screen ownership remains in the menu. |
| 004 | Runner streams stdout/stderr, supports sequential and parallel execution, tail scrolling, cancellation escalation, process-group teardown, and completion summaries. Linux now adopts and reaps orphan descendants through `PR_SET_CHILD_SUBREAPER`. |
| 005 | Stable Provider/Group/Action model, danger levels, destructive confirmation, post-first-paint provider streaming, fuzzy search, Unicode-safe highlights, and tests exist. |
| 006 | UI-independent streaming disk engine uses jwalk/rayon, exact allocated-byte accounting, hardlink deduplication, cancellation, platform seams, and deterministic fixture tests. |
| 007 | TermRock supplies multi-select, trailing metadata, Progress, and LogPane with product-neutral APIs. Latest upstream gate passed with 306 tests, doctests, feature matrix, docs, license/advisory checks, package verification, and deterministic previews. |
| 008 | Analyzer provides progressive tree navigation, folding, multi-selection, shared confirmation/report flow, Trash default, permanent opt-in, dry-run, validated deletion, and operation logging. Production deletion exists only in `src/cleanup/`. |
| 009 | macOS cleanup registry, safety taxonomy, sizing, process guards, review-first behavior, age gates, never-touch rules, artifact discovery, and legacy-cleaner migration are implemented and tested. |
| 010 | Versioned frecency/query-memory store, bounded boost, Recent projection, corruption tolerance, concurrent merge behavior, pruning, and `HOLLA_NO_HISTORY` are implemented and tested. |
| 011 | Bounded size cache, visibly stale first frame, live replacement, Spotlight top files with timeout/fallback, overview, and shared deletion flow are implemented and tested. |
| 012 | Finder uses pinned FFF indexing, concurrent search, bounded ranked results, exclusions, Unicode-safe highlights, reveal/open actions, and analyzer handoff. FFF pin matched upstream `main` during closure. |
| 013 | Validated argv-only user actions, mandatory danger, project trust hashes, launcher trust flow, versioned JSON list output, headless run exit contracts, and doctor command are implemented and tested. |
| 014 | Node, Just, Make, Taskfile, Cargo, Brew services, and Git hygiene providers are registered with pure parsers, absence behavior, caps, argv commands, conservative danger, cache, and tests. |
| 015 | Linux design/matrix is documented; platform-specific surfaces are cfg-gated; fixed-image Linux build/tests cover providers, runner, disk, Trash, and macOS absence. The closure suite passed 246 unit tests and 8 CLI tests with 2 explicit manual-only tests ignored. |

## Closure gates

- Rust `1.97.1`; format, Clippy with all targets/features and warnings denied,
  nextest, and build pass on macOS.
- Linux verification uses
  `rust:1.97.1-bookworm@sha256:77fac8b98f9f46062bb680b6d25d5bcaabfc400143952ebc572e924bcbedc3fa`.
- TermRock is pinned by full revision to the live upstream `main` checked at
  closure; `Cargo.lock` resolves the same revision.
- FFF is pinned by full revision to the live upstream `main` checked at closure.
- Every crates.io direct dependency uses an exact manifest version.
- GitHub Actions and runner images use fixed versions/commit SHAs; no
  `*-latest` labels remain.
- Renovate validates successfully and tracks exact crate versions, lockfile
  maintenance, and TermRock/FFF `main` digests through the `git-refs`
  datasource.

Items explicitly marked out of scope or deferred by the completed plans are
future product work, not incomplete acceptance criteria. Linux parity follow-ups
remain in `docs/linux-port.md`; broader future scope remains in `PRODUCT.md`.
