# Plan 006: Build the disk-usage scan engine (streaming, cancellable, APFS-correct)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat ad8a0f1..HEAD -- src/du/ Cargo.toml`
> `src/du/` must not exist yet. If it does, STOP (someone started this).

## Status

- **Priority**: P2
- **Effort**: L
- **Risk**: MED (filesystem semantics; wrong size math destroys user trust;
  the engine feeds a deletion UI)
- **Depends on**: plans/002-test-baseline-and-bugfixes.md (test conventions).
  Independent of the UI plans — can proceed in parallel with 003–005.
- **Category**: direction / feature
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

PRODUCT.md flow #2: the disk-usage control panel — see what occupies space,
largest first, drill down, select, clean up with confirmation. This plan
builds the ENGINE only (no UI): a parallel filesystem scanner that streams
size-aggregated results into an arena tree, supports cancellation, and gets
macOS/APFS semantics right. Plan 008 puts a TermRock UI on top; plan 009
reuses the sizing machinery for cleanup insights. Engine and UI are split so
the engine is testable headless in CI.

## Current state

- No disk-scanning code exists anywhere in `src/` (the closest thing is
  `find`-based cleanup in `src/commands/gradle.rs`/`idea.rs`).
- Dependencies today: tokio (full), clap, anyhow, thiserror, which,
  tokio-stream, futures, termrock (after 003). `thiserror = "2.0"` is
  available for engine error types.
- Test convention (from 002): colocated `#[cfg(test)]` modules,
  `cargo nextest run --all-features`.

### Research base (verified 2026-07-16; sources in plans/README.md notes)

Design references: dua-cli (MIT — jwalk-based streaming into a tree, "mark
then delete" UX), pdu (Apache-2.0 — rayon recursion, fastest in its own
benchmark suite), dust (Apache-2.0), diskus (MIT/Apache-2.0). **pdu is the
only real library crate but returns a completed tree — no streaming, no
progressive UI — so we build our own core** (its `DataTree` design is still
the model). GPL tools (ncdu, gdu concepts fine; **macdirstat GPL-3.0 — ideas
only, never code**; same rule as Mole).

macOS facts the engine must honor:

1. **On-disk size** = `st_blocks * 512` (`std::os::unix::fs::MetadataExt::blocks()`),
   not `st_size`. Track BOTH (`on_disk`, `apparent`); UI defaults to
   on-disk. Sparse/compressed/dataless files make `st_size` a lie.
2. **Hardlinks**: dedup via `(st_dev, st_ino)` set, only consulted when
   `st_nlink > 1` (cheap filter). Concurrent: `dashmap` or a sharded
   `Mutex<HashSet>`.
3. **Firmlinks**: scanning `/` double-counts (e.g. `/Users` and
   `/System/Volumes/Data/Users` are the same storage). When the scan root is
   `/`, skip `/System/Volumes/Data` (the firmlink table lives at
   `/usr/share/firmlinks`; hardcoding the skip of `/System/Volumes/Data` is
   sufficient and simpler).
4. **Dataless (iCloud) files — MUST NOT materialize**: enumerating dataless
   directories can trigger downloads. On every scan worker thread, call
   `setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
   IOPOL_SCOPE_THREAD, IOPOL_MATERIALIZE_DATALESS_FILES_OFF)` via `libc`;
   dataless access then fails (commonly `EDEADLK`) instead of downloading —
   record the entry with its stat-visible size and move on. Also skip
   descending into `~/Library/Mobile Documents*` by default (matches Mole's
   hard-learned behavior; it's iCloud Drive).
5. **Permissions**: without Full Disk Access, TCC-protected paths return
   EPERM (`~/Library/Mail`, Safari data, Time Machine internals). Record
   per-directory errors in a ledger (path + errno class), keep scanning.
   The UI (008) shows "N inaccessible" + an FDA hint.
6. **Threading**: on Apple Silicon, IO does not scale past performance
   cores (dua-cli defaults to P-core count; fff pins QoS). Default worker
   count = `sysctl hw.perflevel0.logicalcpu` when available, else
   `std::thread::available_parallelism()`. Optionally pin QoS
   (`pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0)`) on
   workers — measured 2-4× wins in fff; guard behind `#[cfg(target_os =
   "macos")]`.
7. Walker: **`jwalk` 0.8 (MIT) on a custom `rayon` pool** — operator
   directive: prefer maintained external crates over hand-rolling. jwalk is
   proven for exactly this job (dua-cli streams its interactive tree from
   jwalk); it gives per-directory parallelism with streamed results. The
   macOS per-thread requirements (dataless iopolicy + QoS pin) attach at
   pool construction: build `rayon::ThreadPoolBuilder::new()
   .num_threads(workers).start_handler(|_| platform::init_scan_thread())`
   and hand it to jwalk via `Parallelism::RayonExistingPool`. Do NOT follow
   symlinks (`follow_links(false)`, default). Custom per-entry stat work
   happens in jwalk's `process_read_dir` callback. If jwalk 0.8's API lacks
   `RayonExistingPool` or an equivalent hook for the pinned pool, fall back
   to `Parallelism::RayonNewPool(workers)` + calling
   `platform::init_scan_thread()` at the top of `process_read_dir`
   (idempotent per-thread via `thread_local!` guard) — note which variant
   shipped. Linux later gets `getdents64`/`statx` tuning inside the same
   callback seam.

### Design (implement exactly)

New module `src/du/` (engine only, no UI imports — enforce: nothing in
`src/du/` may import `termrock` or `ratatui`):

```rust
// src/du/mod.rs
pub struct ScanOptions {
    pub root: PathBuf,
    pub follow_hidden: bool,          // default true (dotfiles count)
    pub skip_paths: Vec<PathBuf>,     // firmlink + Mobile Documents defaults
    pub workers: usize,               // default per fact 6
}

pub struct NodeId(pub u32);           // index into arena

pub struct Node {
    pub parent: Option<NodeId>,
    pub name: OsString,
    pub on_disk: u64,                 // cumulative, updated incrementally
    pub apparent: u64,
    pub entry_count: u64,             // cumulative files under node
    pub is_dir: bool,
    pub state: NodeState,             // Scanning | Done | Errored(ErrKind)
    pub children: Vec<NodeId>,
}

pub struct ScanTree { /* arena: Vec<Node>, root: NodeId */ }
// sorted_children(&self, id, by: SortKey) -> Vec<NodeId>  (SortKey::OnDisk desc default)

pub enum ScanEvent {
    DirAdded { id: NodeId, parent: NodeId, name: OsString },
    SizesUpdated,                     // coalesced; UI re-reads tree snapshot
    DirErrored { id: NodeId, kind: ErrKind },
    Progress { dirs_done: u64, bytes_seen: u64 },
    Finished { duration: Duration, inaccessible: u64 },
}

pub struct ScanHandle {
    pub events: std::sync::mpsc::Receiver<ScanEvent>,
    pub cancel: Arc<AtomicBool>,      // workers check per directory unit
    pub tree: Arc<RwLock<ScanTree>>,  // UI reads snapshots; workers write
}

pub fn scan(options: ScanOptions) -> ScanHandle;
```

Event coalescing: workers update the tree under the lock and send
`SizesUpdated` at most every ~100 ms (a `last_emit` atomic timestamp), so
the channel never floods. Cumulative sizes propagate to ancestors on each
directory completion (walk parent chain adding deltas — arena makes this
cheap).

Platform seam: `src/du/platform.rs` owns `init_scan_thread()` (iopolicy +
QoS, macOS-only, no-op elsewhere) and `default_workers()` (perf-core
sysctl with `available_parallelism()` fallback). Entry metadata is
extracted in the jwalk `process_read_dir` callback into
`RawEntry { name, file_type, on_disk, apparent, nlink, dev, ino, is_dataless }`.

New dependencies (crate-first per operator directive): `jwalk = "0.8"`
(MIT), `rayon = "1"` (MIT/Apache-2.0), `libc = "0.2"` (iopolicy, qos,
sysctl — no maintained safe wrapper covers these three), `dashmap = "6"`
(MIT, hardlink set).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Tests | `cargo nextest run --all-features` | all pass |
| Bench sanity (manual) | `cargo run --release -- <once CLI hook exists in 008>` | n/a here — engine perf test is a unit test with a synthetic tree |

## Scope

**In scope**:
- `src/du/` (new: `mod.rs`, `walker.rs`, `tree.rs`, `platform.rs`,
  `hardlinks.rs` — split as makes sense)
- `src/main.rs` — only the `mod du;` declaration (via `src/lib.rs` if one
  is introduced; simplest: `mod du;` in main.rs)
- `Cargo.toml` (+libc, +dashmap, optionally +crossbeam-channel)

**Out of scope**:
- ANY UI (plan 008). Nothing in `src/du/` imports termrock/ratatui —
  done-criteria grep enforces it.
- Deletion of any kind (plan 008 owns the delete choke point).
- Pattern/mask grouping of results ("all node_modules") — plan 008/009
  layer that on top of the tree.
- Linux fast path (`getdents64`/`statx`) — the jwalk callback is the seam;
  do not implement it now.

## Git workflow

- Branch: `advisor/006-disk-scan-engine` from `main`.
- Conventional Commits, DCO sign-off. Suggested:
  1. `feat(du): arena scan tree and event model`
  2. `feat(du): parallel walker with cancellation and error ledger`
  3. `feat(du): macos correctness — blocks, hardlinks, firmlinks, dataless`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Tree + events (pure, no filesystem)

Implement `ScanTree` arena, `Node`, ancestor size propagation,
`sorted_children` (on-disk desc, ties by name). Unit tests: build a tree by
hand, assert propagation and sorting; empty dir; deep chain (1000 levels —
propagation must not be O(n²) pathological: walking the parent chain per
completed dir is fine, test it completes fast).

**Verify**: `cargo nextest run --all-features` → tree tests pass.

### Step 2: jwalk walker with cancellation

Wire jwalk per design fact 7: pinned rayon pool, `process_read_dir`
extracting `RawEntry`s and updating the tree, `cancel` checked per
directory unit (return early / skip children when set). Fixture tests build
temp trees via the `tempfile` crate (`tempfile = "3"`, MIT/Apache-2.0 —
dev-dependency; crate-first directive applies to test helpers too) —
create ~100 files of known sizes across nested dirs, scan, assert
`on_disk`/`apparent`/`entry_count` totals and `Finished` event. Cancellation
test: huge synthetic breadth (many empty dirs), set cancel after first
event, assert scan returns quickly and no `Finished` with full counts.

**Verify**: `cargo nextest run --all-features` → walker tests pass on macOS
AND in Linux CI (the ubuntu runner — sizes via st_blocks work there too).

### Step 3: Hardlink dedup

`(dev, ino)` in `DashMap<(u64,u64), ()>`, checked only when `nlink > 1`;
first sighting counts, later sightings add to `entry_count` but not sizes.
Test: create a file, `std::fs::hard_link` it elsewhere in the fixture,
assert total counts it once.

**Verify**: test passes.

### Step 4: macOS specifics behind `#[cfg(target_os = "macos")]`

- `platform::init_scan_thread()`: `setiopolicy_np` dataless-off (fact 4) +
  optional QoS pin (fact 6). Both via `libc` FFI — this is the ONE place
  `unsafe` is permitted; keep each call in a tiny documented wrapper in
  `src/du/platform.rs`. Wired into the rayon pool `start_handler` (or the
  thread-local fallback) from Step 2.
- Default `skip_paths`: `/System/Volumes/Data` (when root == `/`),
  `~/Library/Mobile Documents` prefix match.
- Worker count: sysctl `hw.perflevel0.logicalcpu` wrapper with
  `available_parallelism()` fallback.
- Error ledger: EPERM/EACCES → `ErrKind::PermissionDenied` (FDA hint
  material), EDEADLK on dataless → `ErrKind::Dataless` (recorded, not
  fatal).

Tests: sysctl wrapper returns ≥1 on macOS; skip_paths logic is a pure
function (`should_skip(path, &options) -> bool`) — test on all platforms.

**Verify**: `cargo nextest run --all-features` on macOS → pass;
`cargo clippy --all-targets -- -D warnings` → exit 0 (unsafe blocks
documented with `// SAFETY:` comments).

### Step 5: Live sanity run on a real directory

Temporary dev harness (test-only, `#[ignore]`-marked test named
`manual_scan_home_smoke`): scan `~/Library/Caches`, print top-10 by size to
stderr, assert it finishes and total > 0. Run manually:
`cargo nextest run --all-features -- --ignored manual_scan_home_smoke`
— compare its total against `du -sk ~/Library/Caches` (within ~5%
tolerance; du also counts blocks). Record both numbers in the status note.

**Verify**: numbers within tolerance; no hang; Ctrl-C not needed.

## Test plan

Steps embed it: tree math (4+), walker fixtures (4+), hardlinks (1),
platform pure functions (2+), ignored manual smoke (1). ≥12 new tests. Model
after `src/probe.rs` test module (002).

## Done criteria

- [ ] `grep -rn "termrock\|ratatui" src/du/` → no matches.
- [ ] `grep -rn "unsafe" src/du/ | grep -v platform.rs` → no matches;
      every unsafe in platform.rs has a `// SAFETY:` comment.
- [ ] Engine scan of a fixture tree matches known totals exactly;
      `~/Library/Caches` smoke within ~5% of `du -sk`.
- [ ] Cancellation returns promptly (test-proven).
- [ ] All four gates exit 0 (fmt, clippy, nextest, build).
- [ ] No UI plan blocked on API questions: `ScanHandle`
      {events, cancel, tree} is exactly as specified (008 codes against it).
- [ ] `plans/README.md` status row updated.

## STOP conditions

- jwalk 0.8 cannot express BOTH the pinned pool AND per-entry stat
  collection (neither `RayonExistingPool` nor the thread-local fallback
  works) — report the exact API friction; do not silently hand-roll a
  walker (that reverses an operator directive).
- `setiopolicy_np` or QoS constants are missing from the `libc` crate
  version — report (don't hand-define syscall numbers).
- Fixture tests behave differently on the CI ubuntu runner in ways that
  aren't st_blocks-vs-ext4 explainable.
- Any code path would DELETE or WRITE user files. This engine only reads.

## Maintenance notes

- Plan 008 consumes `ScanHandle` and renders the tree; plan 009 reuses
  `scan()` pointed at known cache roots for sizing insights. Keep the
  engine UI-free.
- The jwalk `process_read_dir` callback is where Linux `getdents64`/`statx`
  tuning and a future macOS `getattrlistbulk` fast path land — benchmark
  before adding either.
- Clone/CoW (APFS clonefile) sharing is intentionally NOT dedup'd (no
  public API); document in 008's UI copy ("sizes may overcount cloned
  files"). Purgeable space is likewise invisible — 008 shows the
  scanned-total vs volume-used delta honestly.
