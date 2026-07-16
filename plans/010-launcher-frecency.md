# Plan 010: Launcher intelligence — frecency ranking and query memory

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: plan 005 must be DONE (providers,
> `ActionSpec` with stable `id`, `src/search.rs`). Verify:
> `grep -rn "pub id: &'static str" src/model.rs` matches. Missing = STOP.

## Status

- **State**: DONE
- **Priority**: P3
- **Effort**: M
- **Risk**: LOW (additive ranking layer; launcher works identically with an
  empty store)
- **Depends on**: plans/005-provider-model-async-probe-search.md
- **Category**: direction
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

Mission (PRODUCT.md): "easy entry point... without complexity." After a week
of use, the actions a user actually runs (their upgrade combo, their
compose commands) should surface first — before typing anything. This is the
pattern that makes launchers feel telepathic (fff research: empty query =
frecency-ranked; "combo" query memory boosts the file you picked last time
for the same query). `ActionSpec.id` was designed as the frecency key
(plan 005 maintenance note records this intent).

## Current state (after 005)

- `ActionSpec { id: &'static str, ... }` — stable ids like
  `"docker.clean-all"`.
- Launcher: empty query renders full grouped list in provider order;
  non-empty query renders nucleo-ranked flat hits (`src/search.rs::search`).
- No persistence of any kind in holla yet (ops.log from 008 is the only
  file holla writes, and it may not be landed yet — independent).

### Design (decided)

1. **Store**: JSON file `~/.cache/holla/frecency.json`
   (`$XDG_CACHE_HOME` respected), schema-versioned (`"v":1`):
   `{ "actions": { "<action-id>": { "uses": [epoch_secs,...] } }, "queries": { "<query>": "<action-id>" } }`.
   Keep last ≤20 timestamps per action; prune actions unseen for 90 days on
   save. No new deps — hand-rolled serde? NO: add `serde = { features = ["derive"] }` +
   `serde_json` (ubiquitous; check `cargo tree` — likely already
   transitive). Corrupt/missing file → empty store (never crash, never
   block).
2. **Score** (fff-derived, simplified): for each use at age `d` days,
   `e^(-0.0693 * d)` (10-day half-life), summed, then
   `sqrt`-diminished above 10 uses. Pure function
   `frecency_score(uses: &[u64], now: u64) -> f64` — unit-test with fixed
   timestamps.
3. **Ranking integration**:
   - Empty query: a "Recent" pseudo-group renders FIRST (top 5 actions by
     frecency, only those with score > threshold), then the normal grouped
     list. Separator label "Recent".
   - Non-empty query: final rank = nucleo score + bounded frecency boost
     (`base * min(frecency,100)/100 * 0.25` — boost never exceeds 25%, so
     text relevance stays dominant), plus query-memory: if
     `queries[query] == action.id`, add a fixed top-boost (fff "combo").
4. **Recording**: on action activation (post-confirmation for destructive),
   append timestamp + set `queries[current_query]` when query non-empty;
   save asynchronously (spawn_blocking; failures logged to stderr, never
   surfaced as UI errors).
5. **Privacy/off switch**: `HOLLA_NO_HISTORY=1` env var disables load+save.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Gates | `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` | all exit 0 |

## Scope

**In scope**: `src/frecency.rs` (new), `src/search.rs` (boost hook),
launcher event loop (record + Recent group projection), `Cargo.toml`
(serde/serde_json if not transitive).

**Out of scope**: any other persistence; ops.log; disk analyzer;
per-PROJECT frecency (global only, keep it simple); UI for viewing/clearing
history beyond the env var (follow-up if asked).

## Git workflow

Branch `advisor/010-frecency`; Conventional Commits + DCO (`git commit -s`);
no push/PR without operator instruction.

## Steps

### Step 1: Store + score (pure core)

`src/frecency.rs`: load/save/record/score per design 1–2. Tests: score
monotonicity (recent > old), half-life math at fixed points, prune logic,
corrupt-file tolerance (feed garbage bytes → empty store), version field
present on save.

**Verify**: `cargo nextest run --all-features` → new tests pass.

### Step 2: Ranking integration

Design 3 in `src/search.rs` (boost is a parameter — pure, testable) and the
launcher projection (Recent group). Tests: boost bounded at 25%; query
memory wins ties; empty store → identical ranking to pre-plan behavior
(regression guard: existing search tests unchanged and green).

**Verify**: existing 005 search tests still pass unmodified; new tests pass.

### Step 3: Recording + env off-switch

Design 4–5. Test: `HOLLA_NO_HISTORY=1` → no file created (fixture HOME).

**Verify**: full gates; manual: run an action twice, relaunch → it appears
under "Recent".

## Test plan

≥10 new tests: score math (4), store robustness (3), boost/rank (3+).
Colocated modules per repo convention.

## Done criteria

- [x] Empty frecency store → launcher behavior byte-identical to plan 005
      (existing tests unmodified, green).
- [x] Boost capped (test-enforced); text relevance dominant.
- [x] Corrupt store never crashes or blocks startup (test-enforced).
- [x] `HOLLA_NO_HISTORY=1` honored (test-enforced).
- [x] Four gates exit 0; `plans/README.md` row updated.

## STOP conditions

- 005 not landed. — Store write path wants to block the UI thread. — You
  reach for LMDB/sqlite (fff uses heed; holla's dozens-of-actions scale
  needs a JSON file, not a database).

## Maintenance notes

- Fake-cache PTY smoke at `/tmp/holla-plan010.2t83lS`: activated
  `disk.scan-custom` from query `custom`; relaunch rendered `Recent` first
  with that action selected; activating the Recent copy appended a second
  use while preserving query memory. Both nested prompt cancellations
  restored the terminal. Store writes use a lock, pending-event merge, and
  atomic replacement so concurrent holla processes do not lose uses.
- Completion gates: fmt, clippy `-D warnings`, 166 tests passed with 2
  intentional manual tests skipped, and build succeeded against TermRock
  v0.11.0; the consumer is currently pinned to `371ff94`.

- Schema versioned; bump `"v"` on change and migrate-or-discard old files.
- If per-project ranking is ever wanted, key = (project root hash,
  action id) — additive change to the same store.
