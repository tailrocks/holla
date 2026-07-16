# Plan 008: Disk-usage analyzer screen — drill-down tree, multi-select, safe cleanup

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: verify done-criteria greps of plans 005, 006
> and confirm plan 007's features exist in the pinned termrock rev (check
> `TreeNode` has `trailing`, `Selection`/checked-set API, `Progress` widget).
> Missing prerequisite = STOP.

## Status

- **Priority**: P2
- **Effort**: L
- **Risk**: HIGH (this plan introduces deletion of user files — every
  safety invariant from PRODUCT.md applies; review accordingly)
- **Depends on**: plans/005-provider-model-async-probe-search.md,
  plans/006-disk-scan-engine.md, plans/007-termrock-extensions.md (steps 2–4)
- **Category**: direction / feature
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

This is the flagship feature (PRODUCT.md flow #2): "you run out of disk
space and have no idea what you can delete." The engine (006) computes; this
plan shows: a size-sorted drill-down tree (largest first), progressive
results while scanning, folded noise dirs, multi-select with expected
reclaimed space, and a deletion flow that is confirmation-gated, validated,
Trash-first, and logged. Product references: dua's mark-then-delete UX
(MIT), Mole's analyze TUI and safety architecture (GPL-3.0 — **ideas only,
never code**).

## Current state

After the dependency plans land:

- `src/du/` exposes exactly (from plan 006):
  `scan(ScanOptions) -> ScanHandle { events: mpsc::Receiver<ScanEvent>,
  cancel: Arc<AtomicBool>, tree: Arc<RwLock<ScanTree>> }`, with
  `ScanTree::sorted_children(id, SortKey::OnDisk)`, per-node
  `on_disk/apparent/entry_count/state`, error ledger via
  `ScanEvent::DirErrored` and `Finished { inaccessible, .. }`.
- Launcher (005): providers contribute `GroupSpec`s; `ActionSpec.run` can
  drive any future, including a full-screen sub-UI; destructive actions
  confirm via ChoiceDialog; task events render via the 004 runner.
- TermRock (007): Tree/List `trailing: Option<Line>` metadata cells,
  checked-set multi-select with `Space` + `CheckToggled(Id)` outcome,
  `Progress` widget, ChoiceDialog/ModalStack (`interaction/modal.rs`:
  `open/open_sub/pop`, Esc walks back one step — verified v0.8.0).
- TermRock Tree (verified v0.8.0, `widgets/tree.rs`): caller-flattened
  nodes `TreeNode { id, label, depth, branch, expanded, enabled, status }`;
  `TreeState::handle_key` gives Up/Down/Home/End/PageUp/PageDown,
  Left = collapse-or-parent, Right = expand, Enter = `Activated(Id)`;
  `TreeOutcome::Toggle(Id)` on disclosure click; `TreeNodeStatus::Loading`
  renders a loading suffix (`tree.rs:150-205`).

### Design decisions (already made)

1. **Entry point**: a new `DiskProvider` (in `src/providers/disk.rs`)
   always present in the registry (no detection needed), group "Disk
   usage", actions: `disk.scan-home` ("Analyze home folder"),
   `disk.scan-here` ("Analyze current folder"), `disk.scan-custom`
   ("Analyze a path…" — TextInput dialog for the path). All `Danger::Safe`
   (scanning reads only). Each opens the analyzer screen.
2. **Analyzer screen** (`src/tui/analyzer.rs`, new): own poll/draw loop on
   the shared Session pattern (003). Layout: StatusBar header (root path,
   scanned bytes, inaccessible count, scan state) / Tree body / selection
   summary line ("3 items selected — 12.4 GB reclaimable") / HintBar.
   While scanning: `Progress` (indeterminate + bytes-seen counter) in the
   header area; tree fills progressively (poll `ScanHandle.events` each
   frame, drain-loop, then re-project visible nodes from the tree
   snapshot).
3. **Tree projection**: node label = dir/file name; `trailing` = humanized
   on-disk size + percentage of parent; children sorted largest-first
   (engine's `sorted_children`); collapsed by default except the root's
   first level. Expanding a still-`Scanning` node shows
   `TreeNodeStatus::Loading`. Depth is caller-flattened: project only
   expanded paths (standard flatten walk).
4. **Folding (masks)**: a projection-layer rule (NOT engine): directories
   whose name ∈ FOLD_SET (`node_modules`, `.git`, `target`, `build`,
   `dist`, `.venv`, `venv`, `__pycache__`, `DerivedData`, `Pods`,
   `.gradle`, `.next`, `.turbo`, `.cache`) render as a single non-expandable
   row labeled e.g. `node_modules (folded)` — still selectable for
   deletion as a unit. `f` toggles folding globally. Keep FOLD_SET a
   `const` in the analyzer, exported for tests.
5. **Selection & reclaim estimate**: Space toggles (007 multi-select);
   summary sums `on_disk` of checked nodes, EXCLUDING descendants of
   already-checked ancestors (pure function `effective_selection(tree,
   checked) -> (Vec<NodeId>, u64)` — dedup nested selections; test this
   hard).
6. **Deletion flow** (the safety-critical part — mirrors Mole's
   architecture as ideas, coded fresh):
   - New module `src/cleanup/mod.rs`: THE single deletion choke point.

     ```rust
     pub enum DeleteMode { Trash, Permanent }
     pub struct DeletePlan { pub items: Vec<PathBuf>, pub mode: DeleteMode, pub dry_run: bool }
     pub struct DeleteReport { pub removed: Vec<(PathBuf, u64)>, pub failed: Vec<(PathBuf, String)>, pub skipped: Vec<(PathBuf, String)> }
     pub fn execute(plan: &DeletePlan) -> DeleteReport;      // sync, called from spawn_blocking
     pub fn validate(path: &Path) -> Result<(), Rejection>;  // pure, heavily tested
     ```

   - `validate` rules (allow-then-deny, all pure): must be absolute; must
     not contain `..` components; must not be a bare protected root —
     `/`, `/bin`, `/sbin`, `/usr` (except under `/usr/local`), `/etc`,
     `/System`, `/Library` (bare), `/Applications` (bare), `/Users` (bare),
     `$HOME` itself, `/var/db`; symlinks: delete the LINK only, never
     resolve-and-delete-target; reject empty path components (guards the
     `"$dir/$name"` empty-var collapse class).
   - `Trash` mode: rename into `~/.Trash/<name>` (uniquify on collision:
     `name 2`, `name 3`, …). If rename fails with EXDEV (other volume),
     report as `skipped` with reason "cross-volume — use permanent mode"
     (do NOT silently copy+delete). No Finder/AppleScript dependency.
   - `Permanent` mode: `std::fs::remove_dir_all`/`remove_file` — only
     reachable via an extra toggle in the confirm dialog.
   - `dry_run` short-circuits INSIDE `execute` (structural, per PRODUCT.md),
     returning what WOULD happen.
   - Operation log: append JSON-lines to
     `~/.cache/holla/ops.log` (`$XDG_CACHE_HOME` respected): timestamp,
     mode, path, size, outcome. Fail-open (log error → still proceed, note
     in report).
   - UI flow: Backspace/`d` on selection → ChoiceDialog: body lists first
     10 items + "…and N more", total reclaimable, mode line
     ("Move to Trash" default; `p` inside dialog toggles Permanent — dialog
     body re-renders with a `Role::Danger` warning); actions
     `[Cancel (focused), Delete]`. On confirm: run via `spawn_blocking`,
     progress via the 004 `TaskEvent` seam or a simple modal progress,
     then a MessageDialog report (removed/failed/skipped counts, freed
     bytes) and tree refresh of affected parents (re-scan those subtrees or
     subtract sizes — subtracting is acceptable; mark nodes stale).
7. **Navigation**: Enter/→ descend (expand), ← collapse/up (Tree owns
   this), `r` rescan root, `s` toggle sort on-disk/apparent, `q`/Esc
   leave analyzer (cancel scan via `ScanHandle.cancel`, confirm if a
   delete is running). Esc inside dialogs walks back one step
   (ModalStack semantics).
8. **Honesty copy**: footer/status notes "sizes are on-disk; APFS clones
   may overcount; purgeable space not included" and, when
   `inaccessible > 0`, "N items unreadable — grant Full Disk Access for a
   complete picture".

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Tests | `cargo nextest run --all-features` | all pass |
| Build | `cargo build` | exit 0 |

## Scope

**In scope**:
- `src/providers/disk.rs` (new), registry registration
- `src/tui/analyzer.rs` (new screen)
- `src/cleanup/mod.rs` (+ `validate.rs`, `trash.rs` as needed — new)
- `Cargo.toml` — ideally NO new deps (trash via rename; JSON lines via
  hand-rolled `format!` with escaping, or `serde_json` if already
  transitively present — check `cargo tree`; if adding, prefer
  `serde_json` explicitly, it's ubiquitous)
- Re-pin termrock rev to include 007's features

**Out of scope**:
- Engine internals (`src/du/` — consume `ScanHandle` as-is; needed engine
  changes = STOP and report, they belong in a 006 follow-up)
- Cleanup category insights (DerivedData, brew, … — plan 009 builds these
  ON the `cleanup::execute` choke point)
- `sudo` anything. Analyzer runs as the user; system paths show as
  inaccessible.
- Windows/Linux trash conventions (Linux port later).

## Git workflow

- Branch: `advisor/008-disk-analyzer` from `main`.
- Conventional Commits, DCO sign-off. Keep the deletion choke point its own
  commit (`feat(cleanup): validated deletion choke point with trash mode`)
  so it can be reviewed in isolation. Do NOT push or open a PR unless the
  operator instructed it.

## Steps

### Step 1: `cleanup::validate` + `execute` (pure core first, TDD)

Write the validator tests BEFORE the implementation — adversarial corpus
(Mole keeps 79 fuzz paths as an idea; write our own ~30): `/`, `$HOME`,
`~/.Trash` itself, `/usr/local/foo` (allowed), `/usr/bin/x` (denied),
relative paths, paths with `..`, empty components (`/Users//`), symlink
cases, non-existent paths (validate passes; execute reports failed),
unicode names, names with newlines. Then `execute` with tempdir fixtures:
trash-rename works + uniquifies; permanent removes; dry_run touches
nothing (assert files still exist); report accounting exact; ops.log line
appended.

**Verify**: `cargo nextest run --all-features` → ≥20 new cleanup tests pass.

### Step 2: DiskProvider + analyzer screen skeleton (read-only)

Provider actions (decision 1) open `analyzer::run(root)` — Session-pattern
loop, drain `ScanHandle.events`, project + render Tree with trailing sizes,
Progress while scanning, navigation per decision 7. NO deletion keys yet.

**Verify**: `cargo run` → Disk usage group appears; "Analyze current
folder" streams a growing tree, largest-first; drill-down + back work;
`q` cancels scan promptly (engine cancel test already proves the
mechanism); no UI freeze on large dirs.

### Step 3: Folding + selection summary

Decisions 4–5. Pure functions `fold_row(...)` and `effective_selection`
with unit tests (nested checked dirs dedup; folded node selectable;
`f` toggle reprojects).

**Verify**: tests pass; manual: select a parent and a child → summary
counts parent only.

### Step 4: Deletion flow

Decision 6 UI wiring onto the Step-1 core. Danger styling from
`Role::Danger`/`Role::Warning` only.

**Verify** (in a THROWAWAY fixture dir, never on real data):
`mkdir -p /tmp/holla-smoke/{a,b}/big && dd if=/dev/zero of=/tmp/holla-smoke/a/big/f bs=1m count=50 2>/dev/null`
→ analyze `/tmp/holla-smoke`, select `a`, delete (Trash) → confirm dialog
shows ~50 MB; after: `a` gone from `/tmp/holla-smoke`, present in
`~/.Trash`, report dialog correct, `~/.cache/holla/ops.log` has the line;
repeat with dry-run toggle if exposed (validate nothing moved); permanent
mode on `b` → gone entirely. Esc at the dialog → nothing deleted.

### Step 5: Full gate + safety review pass

**Verify**: all four gates exit 0. Then self-review the diff against the
checklist: (1) every filesystem delete call in the whole repo lives in
`src/cleanup/` — `grep -rn "remove_dir_all\|remove_file\|fs::rename" src/ | grep -v "src/cleanup/\|src/du/"`
→ no deletion outside the choke point (`du` has none; `commands/{gradle,
idea}.rs` legacy `find -exec rm` TaskDefs are pre-existing and migrate to
the choke point in plan 009 — leave them, note them); (2) no deletion path
skips `validate`; (3) confirm dialog cannot default-activate Delete
(Cancel is focused initial).

## Test plan

- `cleanup` validator adversarial corpus (~30 cases) + execute fixtures
  (≥8) — the heart of this plan.
- `effective_selection` (≥4: disjoint, nested, folded, empty).
- Projection: fold rules, sort order, loading status mapping (≥4).
- Manual smoke script of Step 4 recorded in the status note.
- Screen loop: manual only (consistent with 003/004 policy).

## Done criteria

- [ ] Deletion exists ONLY in `src/cleanup/` (grep above) and every path
      goes through `validate`.
- [ ] Trash is the default mode; permanent requires the explicit in-dialog
      toggle; dry-run short-circuits inside `execute`.
- [ ] Confirm dialog: Cancel focused by default; Esc cancels; body shows
      item count + reclaimable bytes.
- [ ] Analyzer streams progressively and cancels cleanly (manual smoke
      recorded).
- [ ] `~/.cache/holla/ops.log` records every outcome in the smoke run.
- [ ] All four gates exit 0; new tests ≥ 45 total across cleanup/selection/
      projection.
- [ ] `plans/README.md` status row updated (include termrock re-pin SHA).

## STOP conditions

- Plan 007 features absent from the newest termrock rev.
- You are tempted to add ANY delete/rename outside `src/cleanup/`.
- `ScanHandle`'s API (006) doesn't fit the screen's needs — report the gap;
  don't fork engine logic into the UI.
- Trash rename semantics on APFS behave unexpectedly in smoke (e.g.
  cross-volume `~/.Trash`) — report with the exact errno.
- Any test in the validator corpus fails and the fix would WEAKEN a rule.

## Maintenance notes

- Plan 009 routes ALL its cleanup actions through `cleanup::execute` —
  including migrating the legacy `find -exec rm` TaskDefs in
  `src/commands/{gradle,idea}.rs`. Reviewers: any future PR calling
  `std::fs::remove_*` outside `src/cleanup/` is a rejection.
- The ops.log format is append-only JSON-lines; if it ever needs schema
  changes, version the line (`"v":1` field from day one).
- Deferred honestly: `mdfind`-based instant large-file discovery
  (Spotlight) and size caching between runs (Mole ideas) — good follow-ups
  after real-world latency feedback.
