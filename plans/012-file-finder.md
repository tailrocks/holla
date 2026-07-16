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

_(filled by executor)_

- Availability/license:
- API fit:
- Footprint:
- Platform:
- **Chosen path**: fff-core | ignore+nucleo fallback — because:

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

- [ ] Spike verdict recorded with evidence and a defensible choice per the
      decision rule.
- [ ] Typing in the finder returns ranked hits while indexing continues
      (manual smoke recorded, no UI freeze).
- [ ] `~/Library/Mobile Documents` never traversed (test or code-cited
      skip rule).
- [ ] All new deps' licenses recorded in the spike verdict
      (MIT/Apache/BSD/MPL only).
- [ ] Four gates exit 0; `plans/README.md` row updated.

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
