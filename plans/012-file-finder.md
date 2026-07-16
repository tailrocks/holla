# Plan 012: File & folder finder — fuzzy find anything, fast (fff-core spike + integrate)

> **Executor instructions**: Follow this plan step by step. Step 1 is a
> research/spike gate — its outcome decides the implementation path; record
> the decision in this file under "Spike verdict" before coding. Run every
> verification command. On any STOP condition, stop and report. When done,
> update the status row in `plans/README.md`.
>
> **Drift check (run first)**: plan 005 must be DONE (launcher, providers,
> nucleo search, TextInput pattern). Verify and STOP if not.

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P3
- **Effort**: L
- **Risk**: MED (external SDK integration; scanning scope on real homes)
- **Depends on**: plans/005-provider-model-async-probe-search.md
  (plan 008's analyzer helps but is not required)
- **Category**: direction
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

The operator's brief names this explicitly: "using, for example, FFF for
finding specific file folder on the system." A launcher that can find
actions but not files stops one step short — "where IS that project /
config / huge download" is the same daily question as "what can I run."
And the operator directive is crate-first: fff's core is a published,
MIT-licensed, actively maintained Rust file-search SDK (typo-resistant
fuzzy path matching, frecency, background watcher, macOS QoS-pinned
scanning — it powers pickers in opencode and nushell). Prefer embedding it
over building our own walker+matcher.

## Current state (after 005)

- Launcher: TextInput + List + nucleo-matcher over ACTIONS only.
- `which`, tokio, termrock in the tree; no file indexing anywhere.
- fff facts (researched 2026-07-16): repo `dmtrKovalenko/fff`, MIT,
  v0.9.x, very active; Rust core crate (`fff-core`) + C/Python/Node
  bindings; matcher = `neo_frizbee` (SIMD Smith-Waterman); walker =
  `ignore` crate (default feature); frecency via LMDB (`heed`), 10-day
  half-life; macOS: QoS pinning + P-core-sized search pool
  (`sysctl hw.perflevel0.physicalcpu`); no tokio in core (rayon pools).

### Design intent

New launcher group "Find" (always present): `find.files` ("Find a file or
folder…") opens a finder screen: TermRock TextInput + List (+ optional
preview pane later), results ranked as-you-type across a configurable root
set (default: `$HOME`, respecting ignore rules), Enter on a result →
actions submenu: reveal in Finder (`open -R`), open (`open`), copy path
(OSC 52 via termrock's osc module — check `termrock::osc` for the
clipboard-write helper), analyze size (jump into 008's analyzer at that
path, when landed).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Gates | `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` | all exit 0 |

## Scope

**In scope**: Step-1 spike doc (this file's "Spike verdict" section),
`src/providers/find.rs`, `src/tui/finder.rs`, `src/find/` (index wrapper),
`Cargo.toml` (+fff-core or fallback deps).

**Out of scope**: content grep (fff can; holla doesn't need it yet —
follow-up); background watcher/daemon mode (index per-session first);
replacing plan 005's ACTION search machinery (nucleo stays for actions —
do not unify prematurely).

## Git workflow

Branch `advisor/012-file-finder`; Conventional Commits + DCO
(`git commit -s`); no push/PR without operator instruction.

## Steps

### Step 1: SPIKE — evaluate fff-core as a dependency (timebox: half a day)

Answer with evidence, write results into "Spike verdict" below:

1. **Availability**: is the core published on crates.io (check names
   `fff-core`, `fff`), at what version, or is it git-dependency only?
   License of the exact crate (MIT expected — verify its Cargo.toml).
2. **API fit**: can a consumer (a) build an in-memory index of a root set,
   (b) query with scores + match indices as-you-type from a synchronous UI
   loop, (c) get results without the Neovim/frecency-DB parts, (d) disable
   or redirect the LMDB frecency store (holla may not want a second
   history DB — check if it's optional)?
3. **Footprint**: dependency tree delta (`cargo tree` in a scratch
   project), compile-time impact, binary-size delta. Note anything heavy
   (git2 is in fff's tree for git-status scoring — is it feature-gated?).
4. **Platform**: builds clean on macOS with holla's toolchain (1.96)?
5. **Fallback design** (if any answer disqualifies): `ignore` crate
   parallel walker (`WalkBuilder::build_parallel`, already fff's own
   default walker; MIT/Unlicense) streaming paths into an in-memory
   `Vec`, matched with the EXISTING `nucleo-matcher` dep from 005 —
   i.e. the fallback adds ONE dep (`ignore`) and reuses everything else.

**Decision rule** (operator preference: external over own): adopt fff-core
UNLESS (license ≠ MIT-compatible) OR (frecency-DB unavoidable) OR
(dep-tree delta grossly disproportionate — think >150 new crates) OR
(API cannot serve a sync TUI loop). Otherwise fall back to
`ignore` + `nucleo-matcher`.

**Verify**: "Spike verdict" section below is filled in with the four
answers + chosen path, committed
(`docs: record fff-core spike verdict` — plan file edit is the deliverable).

### Spike verdict

- Availability/license (verified 2026-07-17): `fff-core` is not published on
  crates.io; `fff` `0.3.1` is an unrelated finite-fields crate. The official
  repository does contain `crates/fff-core`, whose package name is
  `fff-search`, version `0.9.6`, MIT, Rust 2024 edition. Holla therefore uses
  the operator-requested repository directly, initially pinned to exact `main`
  SHA `42f38ff66e6c62475678f05ee60c3a311e341884`.
- API fit: current upstream has synchronous `fuzzy_search_mixed`, a
  shared-state background scanner, score data, and file match-byte offsets. A
  default `SharedFrecency` leaves LMDB uninitialized, so no second history DB
  is created; `heed` remains compiled. Holla disables watching and content
  indexing. One picker accepts one root, so Holla sequences safe roots and
  exposes each completed picker while later roots scan. Directory highlight
  offsets are projected with the existing `nucleo-matcher`.
- Footprint: an empty Rust 1.97.1 scratch binary plus exact `fff-search`
  resolved 188 packages versus one baseline package: +187 lock packages and
  151 additional unique normal dependency-tree nodes. `heed` and vendored
  `git2` are unconditional. A cold release build took 42.21 s on this macOS
  machine. Referencing `FilePicker::new` grew the stripped-by-default scratch
  release binary from 430,880 B to 563,840 B (+132,960 B, 30.9%). This exceeds
  the plan's explicit >150-crate rejection threshold.
- Platform: exact `fff-search 0.9.7-nightly.fce72fa` compiled successfully on
  macOS arm64 with the project's current exact Rust 1.97.1 toolchain; no
  platform failure drove the decision.
- **Chosen path at adoption**: exact Git `fff-search` at
  `42f38ff66e6c62475678f05ee60c3a311e341884`. The measured crates.io release
  exceeds the original footprint threshold, but the operator explicitly
  overrode that gate and requested the official Git source when the core crate
  was not published. Current upstream also closes the earlier match-offset API
  gap. Holla accepts the heavy unconditional dependencies, keeps FFF frecency
  uninitialized, and pins the full SHA for reproducibility.

### Step 2: Index wrapper

`src/find/mod.rs`: `FileIndex::build(roots, cancel) -> …` +
`query(&str, limit) -> Vec<FileHit { path, score, indices }>` wrapping the
chosen backend. Indexing runs off-thread (spawn_blocking / its own pool),
streams readiness (count indexed) via the 005 `StdSubscription` pattern;
querying is synchronous per keystroke against whatever is indexed so far
(partial-index results are fine — label "indexing… N files").
Respect ignore rules; skip `~/Library/Mobile Documents*` (dataless
hazard — same rule as 006).

Tests: fixture tree → hits found with expected ranking basics
(filename match beats path match — if backend exposes it), cancel stops
indexing, hidden/ignored files excluded by default.

**Verify**: `cargo nextest run --all-features` green.

### Step 3: Finder screen + provider

Design-intent UX. Result row: filename styled, parent path
`Role::TextMuted`, match indices highlighted `Role::Accent` (reuse 005's
highlight helper). Actions submenu via ChoiceDialog. `find.files` action
registered `Danger::Safe`.

**Verify**: manual: type 3–4 chars of a known filename under `$HOME` →
appears near top while index still building; reveal-in-Finder works; Esc
backs out cleanly. Gates green.

### Step 4: Analyze-size handoff (only if 008 landed)

Result action "Analyze size" opens the 008 analyzer rooted at the hit's
directory. Skip silently (hide the action) if analyzer module absent.

**Verify**: gates green; manual handoff smoke if applicable.

## Test plan

≥8 new tests: index wrapper (5+ incl. cancel + ignore rules), provider
registration (1), highlight projection (2). UI: manual smoke per policy.

## Done criteria

- [x] Spike verdict recorded with evidence and a defensible choice per the
      decision rule.
- [x] Typing in the finder returns ranked hits while indexing continues
      (manual smoke recorded, no UI freeze).
- [x] `~/Library/Mobile Documents` never traversed (test or code-cited
      skip rule).
- [x] All new deps' licenses recorded in the spike verdict
      (MIT/Apache/BSD/MPL only).
- [x] Four gates exit 0; `plans/README.md` row updated.

### Completion evidence (2026-07-17)

- Exact FFF Git SHA `42f38ff66e6c62475678f05ee60c3a311e341884`;
  exact TermRock SHA `f802fcc48c4361ea477c5021b52a121f180d4b4d` after
  applying migrations 0019 and 0020.
- `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo nextest run --all-features` (198 passed), and `cargo build` passed;
  formatting check passed.
- PTY smoke: opened `find.files`, queried `Cargo.toml` while the indexed-file
  counter continued rising, received 100 ranked results, opened/cancelled the
  action dialog, cleared the query, and exited with terminal restoration.

### Dependency migration history

- 2026-07-17: `42f38ff66e6c62475678f05ee60c3a311e341884` →
  `31be2242234df9eb44851f3a59bf007e96986a44` (`fff-search` 0.10.0). Upstream
  publishes no migration guide. The release adds public watcher subscriptions;
  Holla keeps `watch = false`, and its existing picker/search API is unchanged.
  Repin accepted only after finder tests and the full project gates passed.
- 2026-07-17: `31be2242234df9eb44851f3a59bf007e96986a44` →
  `b14c31d137e108b7c520d0d9e0b0017a1a88141d`. The delta changes only FFF MCP
  release artifacts; Holla's linked crates are source-identical. Mac and pinned
  Debian gates passed before the repin was accepted.

## STOP conditions

- Spike answers conflict with the decision rule in a way it doesn't cover.
- Chosen backend can't be cancelled/dropped cleanly (runaway indexing
  threads after screen close).
- You start writing a custom directory walker or fuzzy scorer — both exist
  as approved crates; that's the directive violation this plan exists to
  prevent.

## Maintenance notes

- If fff-core is adopted: pin the version; its release cadence is fast —
  review its CHANGELOG on bumps. Its optional zlob (Zig) walker feature
  stays OFF (build-chain cost).
- Content grep (fff's bigram index) and a persistent background index are
  the natural follow-ups if the finder gets daily use.
- Frecency for FILE hits should reuse plan 010's store patterns (one
  history policy), not fff's LMDB — revisit when both landed.
