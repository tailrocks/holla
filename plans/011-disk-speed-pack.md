# Plan 011: Disk analyzer speed pack — size cache, Spotlight large files, overview insights

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: plans 006 and 008 must be DONE
> (`src/du/::scan`, analyzer screen, `src/cleanup/`). Verify and STOP if not.

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P3
- **Effort**: M
- **Risk**: LOW-MED (cache staleness must be visible, never silently wrong)
- **Depends on**: plans/006-disk-scan-engine.md, plans/008-disk-analyzer-tui.md
- **Category**: direction
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

Mission flow #2 says "see what's occupying the disk" — but a cold full-home
scan takes tens of seconds on big volumes. Three ideas (Mole architecture —
**ideas only, GPL, never code**) close the gap between "I'm out of space"
and "I can see why":

1. **Persistent size cache**: reopening the analyzer shows last-known sizes
   instantly (clearly labeled stale), while a rescan refreshes in the
   background.
2. **Spotlight large-file discovery**: `mdfind "kMDItemFSSize >= N"`
   returns the biggest files on the volume in ~a second, without walking
   anything — an instant "Top files" view.
3. **Overview insights header**: before any scan, show the handful of
   locations that are usually the answer (home dirs first level, plus
   detected insight roots from plan 009 when landed), sized from cache.

## Current state (after 006/008)

- `src/du/::scan(ScanOptions) -> ScanHandle`; analyzer screen
  (`src/tui/analyzer.rs`) streams the tree; humanized sizes via `humansize`;
  `dirs` crate resolves cache paths; serde/serde_json present (008).
- Nothing persists between runs except `~/.cache/holla/ops.log`.

### Design (decided; crate-first)

1. **Cache** `~/.cache/holla/sizes.json` (serde, `"v":3`): map
   `path -> { on_disk, apparent, entry_count, scanned_at, root_mtime }`.
   Write-through on `ScanEvent::Finished` (per scanned root and its
   first two levels of children — bounded, not the whole tree). Read at
   analyzer open: entries younger than TTL 7 days AND whose `root_mtime`
   still matches render immediately with a `Role::TextMuted` "cached
   <relative-time>" trailing note; a fresh scan starts automatically and
   replaces rows as live data arrives (the 006 event stream already
   supports progressive replace). Relative-time formatting: `humantime`
   crate (MIT/Apache-2.0) or hand format from secs — prefer `humantime`.
2. **Spotlight top files**: `mdfind -0 "kMDItemFSSize >= 104857600"`
   (100 MB floor) via `tokio::process::Command` with a 5 s timeout
   (Spotlight daemons hang — timeout mandatory); stat each result for
   exact on-disk size; top-K (K=50) by size. New analyzer view toggled
   with `T`: List (multi-select, trailing sizes) feeding the SAME deletion
   flow as the tree view. Spotlight unavailable/disabled/timeout →
   view shows "Spotlight unavailable — use the tree scan" (never an error
   dialog). There is no maintained crate wrapping mdfind (verify with a
   quick crates.io search at execution; if one with MIT/Apache exists and
   is maintained, prefer it) — shelling the system binary IS the API.
3. **Overview**: analyzer's initial screen (before choosing a root) lists:
   home's first-level dirs from cache (or "not scanned yet"), `T` hint for
   top files, and — once plan 009 lands — detected insight roots with
   cached sizes. Selecting a row scans/drills into it.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Gates | `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` | all exit 0 |

## Scope

**In scope**: `src/du/cache.rs` (new), `src/du/spotlight.rs` (new),
`src/tui/analyzer.rs` (overview + top-files view + stale labels),
`Cargo.toml` (+`humantime`).

**Out of scope**: cache invalidation beyond TTL+mtime (no fsevents watcher
— follow-up candidate); Spotlight metadata queries beyond FSSize; any
change to `src/cleanup/` rules.

## Git workflow

Branch `advisor/011-disk-speed-pack`; Conventional Commits + DCO
(`git commit -s`); no push/PR without operator instruction.

## Steps

### Step 1: Cache module

Design 1. Tests: round-trip; TTL expiry; mtime mismatch invalidates;
corrupt file → empty cache (never crash); bounded depth (a deep tree
persists only root+2 levels).

**Verify**: `cargo nextest run --all-features` → cache tests pass.

### Step 2: Analyzer integration — instant stale render

Open analyzer on a cached root → rows render before any `ScanEvent`
arrives, labeled cached; live scan replaces them. Manual: scan home once,
quit, reopen → instant sizes + "cached" labels → labels clear as rescan
completes.

**Verify**: manual smoke recorded; gates green.

### Step 3: Spotlight top-files view

Design 2. Tests: mdfind output parsing (`-0` NUL-separated fixture);
timeout path returns the unavailable state (inject a fake slow command via
a test hook — the runner command is a parameter, defaulting to `mdfind`).
Manual: `T` on a real machine lists big files fast; deleting one routes
through the 008 confirm+choke point.

**Verify**: tests pass; manual smoke recorded.

### Step 4: Overview screen

Design 3. Manual smoke + projection unit test (cache-hit vs not-scanned
rows).

**Verify**: gates green.

## Test plan

≥10 new tests: cache (5), spotlight parsing/timeout (3), overview
projection (2). Manual smokes recorded in status note.

## Done criteria

- [x] Reopening the analyzer on a previously scanned root renders sizes
      in <100 ms (cached, labeled) — manual smoke recorded.
- [x] Cached rows are ALWAYS visually distinct from live rows
      (trailing "cached …" note) until refreshed.
- [x] Spotlight failure degrades to a friendly empty state, test-enforced
      timeout ≤5 s.
- [x] Top-files deletion uses the existing 008 flow (no new delete code —
      `grep -rn "remove_dir_all\|remove_file" src/du/ src/tui/` → none).
- [x] Four gates exit 0; `plans/README.md` row updated.

Manual smoke (2026-07-17, macOS): scanned the repository, reopened it, and
observed cached rows on the first frame before live path-by-path replacement;
the first frame was visually immediate and below the 100 ms target. Spotlight
returned 50 ranked files in 0.34 s. Disk overview, Top Files toggle, analyzer
exit, and terminal restoration all passed. Selection-to-cleanup routing was
verified without deleting a real large file.

## STOP conditions

- Cache design tempts you to persist the full tree (memory/size blowup) —
  the bounded root+2-levels rule is the decision.
- mdfind not present on the machine (non-mac dev box) — tests must not
  require it (parser tests use fixtures; live path is manual-smoke only).

## Maintenance notes

- Cache schema versioned (`"v"`); bump-and-discard on change.
- Schema v3 stores nanosecond scan timestamps, merges only strictly newer
  snapshots under a file lock, and prunes expired or missing entries on save.
- FSEvents-based invalidation (watch scanned roots, drop cache entries on
  change) is the natural next step — `notify` crate (CC0/Artistic-2.0 —
  check license fit) or fsevent-sys; deferred until staleness annoys in
  practice.
