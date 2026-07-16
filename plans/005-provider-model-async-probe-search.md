# Plan 005: Provider/Action domain model, non-blocking probe, fuzzy launcher search, danger confirmations

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat ad8a0f1..HEAD -- src/`
> This plan assumes 002 (probe field renames, tests), 003 (termrock menu),
> and 004 (streaming runner) have landed. Verify their done-criteria greps
> pass before starting; if not, STOP.

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P1
- **Effort**: L
- **Risk**: MED
- **Depends on**: plans/003-termrock-migration-menu.md, plans/004-task-runner-streaming.md
- **Category**: direction / tech-debt
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

This plan delivers the launcher half of the product mission (PRODUCT.md
"Primary user flows" #1) and removes the structural blockers to it:

1. **Startup blocks on probing.** `src/main.rs:14` calls `Probe::run()`
   before any UI exists; `src/probe.rs:37-63` does 8 blocking `which` calls,
   4 file checks, a directory scan, and — worst — spawns
   `mise tasks ls` synchronously (`src/probe.rs:85-92`), which can take
   hundreds of ms. Mission says: menu paints instantly, results stream in.
2. **No search.** Typing "dock" must surface Docker; "cleanup" must surface
   every cleanup action across groups. Today there is no text input at all.
3. **Destructive actions run with zero confirmation.** In the menu loop,
   Enter directly invokes the handler — `docker: clean everything`
   (force-removes all images/volumes), `gradle: clean all`, `idea: clean`
   delete data with no dialog. This violates the product invariant
   "Everything destructive requires explicit confirmation."
4. **Actions are anonymous closures.** `src/tui/menu.rs:21-32`:

   ```rust
   pub struct Action {
       pub label: String,
       pub description: String,
       pub preview: String,
       pub handler: Box<dyn Fn() -> ActionFuture>,
   }
   pub struct Group { pub title: String, pub icon: &'static str, pub actions: Vec<Action> }
   ```

   No ids, no keywords, no danger level, no provider identity — nothing for
   search, confirmation policy, or the future Linux port to hang onto.
   `Menu::build` (`menu.rs:39-274`) is one 235-line function mixing all
   providers.

## Current state

- After 002: `Probe` has `child_git_repos`, `omz_dir: Option<PathBuf>`;
  parsing is tested. After 003: menu renders as one flattened TermRock
  `List` with `RowRole::Separator` group headers, `ActionId = (usize,
  usize)`, `Theme::tailrocks_phosphor()`, static keymap + HintBar. After
  004: `run_tasks`/`run_parallel_tasks` stream and are cancellable.
- Existing menu content tests (002) assert group titles/labels/previews —
  they are the behavioral contract for the refactor.
- Toolchain: Rust 1.96, edition 2024, tokio "full" already a dependency.

### TermRock pieces (verified at rev `da54a03…`)

- **TextInput** (`widgets/text_input.rs:43-157`): `TextInputState::new("")
  .with_allow_empty(true)`; `handle_key(key) -> TextInputOutcome { Ignored,
  Changed, Submitted(String), Cancelled }`; value via `state.value()`.
  Renders via `TextInput { label, placeholder, validation, theme }`.
  This is the search box; matching logic is explicitly consumer-owned.
- **List** — same as 003; filtered results are just a re-projected
  `Vec<ListRow>`; IDs are stable across filtering by design.
- **ChoiceDialog** (`widgets/dialog.rs:214-339`): `Dialog { title, body:
  Text, style, theme, emphasis }` + `actions: &[Action<Id>]` +
  `ChoiceDialogState::handle_key` → `Outcome { Ignored, Changed,
  Activated(Id), Cancelled }` (Esc cancels, Enter activates, arrows/Tab
  traverse — verified `dialog.rs:238-246`). Render over the list with
  `Backdrop` then the dialog in a centered rect
  (`termrock::centered_rect` is exported at `lib.rs:25-30`).
- **ModalOutcome** (`lib.rs:38-46`) if a modal state machine helps.

### Fuzzy matcher decision (researched 2026-07-16)

Use **`nucleo-matcher` 0.3.x** (MPL-2.0 — file-level copyleft, compatible
with Apache-2.0 linking; helix-editor project, actively maintained; ~6×
faster than the dead `fuzzy-matcher`/skim). holla matches dozens–hundreds of
short strings per keystroke, so raw speed is irrelevant — chosen for API
quality and maintenance. Alternative considered: `neo_frizbee` (used by fff;
SIMD, faster still) — rejected for now: younger fork, no need at this scale.
Match spans from `nucleo_matcher::Matcher::fuzzy_indices` drive highlight
styling. Do NOT pull the high-level `nucleo` crate (its background worker
is for millions of items; overkill here).

### Design decisions (already made)

1. **Domain model** in a new `src/model.rs`:

   ```rust
   pub struct ActionSpec {
       pub id: &'static str,            // stable, e.g. "docker.clean-all"
       pub label: String,
       pub description: String,
       pub preview: String,             // command preview lines
       pub keywords: &'static [&'static str], // extra search terms, e.g. ["cleanup", "prune"]
       pub danger: Danger,
       pub run: Box<dyn Fn() -> ActionFuture + Send>,
   }
   pub enum Danger { Safe, Mutating, Destructive }
   pub struct GroupSpec { pub id: &'static str, pub title: String, pub actions: Vec<ActionSpec> }
   ```

2. **Provider trait** in a new `src/providers/mod.rs`:

   ```rust
   pub trait Provider: Send {
       fn id(&self) -> &'static str;
       /// Cheap sync detection + group construction. Runs on a worker thread.
       fn scan(&self) -> Option<GroupSpec>;
   }
   ```

   One file per provider under `src/providers/`: `current_folder.rs`
   (mise tasks, git, gradle, compose, idea — from `menu.rs:43-147`),
   `repos.rs` (multi-repo actions, `menu.rs:149-190`), `system.rs`
   (upgrades, `menu.rs:193-240`), `docker.rs` (`menu.rs:241-255`),
   `gradle.rs` (`menu.rs:256-263`). `Menu::build` is deleted; its content
   moves verbatim into providers (the 002 tests keep asserting the same
   labels/previews — update them to call the provider registry instead).
3. **Non-blocking probe**: `main.rs` starts the UI immediately with an empty
   group list and a "scanning…" status slot. A `std::thread::spawn` worker
   pool (or `tokio::task::spawn_blocking`) runs each provider's `scan()`;
   results stream over `std::sync::mpsc` → `StdSubscription` (the exact
   pattern of 004) as `ScanEvent::Group(GroupSpec)` +
   `ScanEvent::Finished`. Groups append in a fixed provider order
   (sort by provider registry index, not arrival). `which` lookups happen
   inside provider `scan()`, off the UI thread. The `Probe` struct becomes
   an internal helper for providers that share detection (keep its tests).
4. **Search UX** (fff-informed): search field is always active (no mode
   toggle) — printable chars go to `TextInputState`, navigation keys go to
   the list. Empty query → full grouped list (separators visible). Non-empty
   query → flat ranked rows (no separators), each row's match indices
   highlighted with `Role::Accent`; group title matches (e.g. "dock")
   surface ALL of that group's actions (haystack per action =
   `group title + label + keywords + description`). Rank by nucleo score
   desc, stable by (group, action) order. Esc clears query first; second
   Esc quits. Enter activates selected.
5. **Confirmation**: activating a `Danger::Destructive` action opens
   ChoiceDialog — body = action description + preview lines + warning
   styled with `Role::Warning`; actions `[Cancel (focused), Run]`. Only
   `Outcome::Activated("run")` executes. `Danger::Mutating` runs directly
   (upgrades etc.); `Safe` obviously direct. Assign initial danger levels:
   - Destructive: `docker.clean-all`, `docker.stop-all` (removes
     containers), `gradle.clean-all`, `idea.clean`, `git.push-all-remotes`?
     → No: pushes are Mutating. Destructive = deletes local data.
   - Mutating: all upgrades, git pull/push, compose up/down, mise tasks,
     gradle build/test.
   - Safe: git status, compose logs.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Tests | `cargo nextest run --all-features` | all pass |
| Build | `cargo build` | exit 0 |

## Scope

**In scope**:
- `Cargo.toml` (+`nucleo-matcher = "0.3"`)
- `src/model.rs`, `src/providers/*.rs` (new)
- `src/search.rs` (new — matching/ranking, pure functions)
- `src/tui/menu.rs` (event loop: search field, filtered projection,
  confirmation dialog; rows now project from `GroupSpec`)
- `src/main.rs` (immediate UI start + scan channel)
- `src/probe.rs` (demote to shared detection helpers used by providers)
- Existing tests (update constructors/imports; assertions unchanged)

**Out of scope**:
- `src/tui/app.rs` (runner is done; call it, don't change it)
- `src/commands/*` implementations (handlers move, bodies unchanged)
- Disk analyzer / cleanup providers (006, 008, 009)
- Frecency ranking, query history (roadmap "Later")
- Any TermRock repo change — filter highlighting is consumer-side span
  styling; if you find yourself needing a new widget, note it for 007, STOP
  only if blocked.

## Git workflow

- Branch: `advisor/005-provider-model` from `main` (after 004 merged).
- Conventional Commits, DCO sign-off. Suggested sequence:
  1. `refactor: introduce ActionSpec/GroupSpec/Provider model`
  2. `refactor: split Menu::build into providers`
  3. `feat: stream provider scans into the menu without blocking startup`
  4. `feat: fuzzy search across groups and actions` 
  5. `feat: confirmation dialog for destructive actions`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Introduce the model + providers (pure refactor, UI unchanged)

Create `src/model.rs` and `src/providers/` as in design decisions 1–2. Move
each `Menu::build` section into its provider, assigning `id`, `keywords`,
`danger` per the table in decision 5. A thin
`pub fn all_providers() -> Vec<Box<dyn Provider>>` registry fixes ordering:
current_folder, repos, system, docker, gradle. Rewire the menu to build its
rows from `Vec<GroupSpec>` produced by running all providers synchronously
(temporary — async lands in Step 3). Update 002/003 tests to construct
groups via providers with an injected `Probe`; label/title/preview
assertions stay identical.

Note: `scan()` returning the group WITH its actions keeps provider logic in
one place. Providers needing async detection later (Docker daemon liveness)
can block inside `scan()` — it already runs on a worker thread.

**Verify**: `cargo nextest run --all-features` → all pass (same
assertions); `grep -n "Menu::build" src/` → no matches.

### Step 2: Danger levels + confirmation dialog

Implement decision 5 in the menu event loop: a
`Option<PendingConfirm { action_id }>` modal state; while `Some`, keys route
to `ChoiceDialogState::handle_key`; render `Backdrop` + `ChoiceDialog` in
`centered_rect`. Add keymap hint. Tests: pure-function test that
`needs_confirmation(&ActionSpec)` is true exactly for `Destructive`; content
test that `docker.clean-all` is `Destructive` (protects against silent
downgrades in review).

**Verify**: `cargo nextest run` green; manual: select
`docker: clean everything` → dialog appears; Esc cancels (nothing runs);
Enter on "Run" executes.

### Step 3: Non-blocking scan

Implement decision 3. First paint must not wait on any provider:
`main.rs` builds the terminal + empty state, then spawns scans. StatusBar
right slot shows `scanning…` until `Finished`. Groups insert in registry
order as they arrive (re-sort the group vec by provider index on each
insert).

Add a startup latency guard-test where feasible: unit-test that
`ScanEvent` ordering logic inserts out-of-order arrivals correctly
(pure function over a Vec). UI latency itself: manual check.

**Verify**: `cargo run` in a dir with a slow `mise tasks ls` (or wrap: any
repo with mise) → menu frame appears instantly, mise actions appear later
without input lag. Tests pass.

### Step 4: Fuzzy search

Add `nucleo-matcher = "0.3"`. Create `src/search.rs`:

```rust
pub struct SearchHit { pub group: usize, pub action: usize, pub score: u32, pub indices: Vec<u32> }
pub fn search(groups: &[GroupSpec], query: &str) -> Vec<SearchHit>
```

Haystack per action per decision 4; use
`Matcher::new(Config::DEFAULT)` + `Pattern::parse(query,
CaseMatching::Ignore, Normalization::Smart)` (check nucleo-matcher 0.3 docs
for exact names — if the API differs, adapt; the contract is
score+indices). Menu loop: `TextInputOutcome::Changed` → recompute hits →
re-project rows (flat when query non-empty); selection resets to top hit;
Esc-clears-then-quits per decision 4. Highlight matched label chars via
span styling with `Role::Accent`.

Tests (pure, in `src/search.rs`): "dock" matches group "Docker" surfacing
all docker actions ranked; "cleanup" matches actions whose keywords include
"cleanup" across ≥2 groups; empty query returns empty hit list; ranking
deterministic.

**Verify**: `cargo nextest run` green; manual: type `dock` → docker actions
top; type `clean` → docker clean, gradle clean, idea clean all visible.

### Step 5: Full gate + smoke

**Verify**:
`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` → all exit 0.
Manual checklist: instant first paint; groups stream in; search narrows and
highlights; destructive confirm; cancel path; runner still streams (004
regression check).

## Test plan

- Provider content tests: ported 002 assertions + per-provider gating
  (probe field → group present/absent) — ≥10 cases.
- `search.rs` pure tests — ≥5 cases (above).
- Danger/confirmation policy tests — 2 cases.
- Scan-ordering pure test — 1 case.
- Pattern: colocated `#[cfg(test)]`, model after `src/probe.rs` tests.

## Done criteria

- [x] `grep -rn "Menu::build" src/` → no matches; providers own content.
- [x] `grep -n "Probe::run()" src/main.rs` → no match before terminal
      setup (no blocking scan pre-paint).
- [x] Every `ActionSpec` has a stable `id`; `grep -rn 'id: ""' src/` → none.
- [x] `docker.clean-all` is `Danger::Destructive` and test-protected.
- [x] All four gates exit 0; total test count ≥ 30.
- [x] Manual smoke checklist recorded in the status row note.
- [x] `plans/README.md` status row updated.

## STOP conditions

- 002/003/004 done-criteria greps fail at start (dependency not landed).
- nucleo-matcher 0.3 API diverges so far from score+indices that you'd
  vendor matching logic — report options instead.
- The always-active search field conflicts with List navigation keys in a
  way the keymap can't cleanly arbitrate (e.g. `j/k` vs typing) — decide:
  printable chars → input, arrows/Enter/Esc/PgUp/PgDn → list, `j/k` NOT
  list-bound while search is enabled; if that still breaks, STOP and report.
- Any need to modify `src/tui/app.rs` beyond imports.

## Maintenance notes

- The provider registry is the Linux-port seam: platform-specific providers
  get `#[cfg(target_os)]` registration, nothing else changes.
- Plan 008 adds a "Disk usage" provider whose action opens a sub-screen
  (not a TaskDef) — the `run` closure type already returns a future, which
  can drive any UI; keep that flexibility.
- Plan 009 adds cleanup providers with `Danger::Destructive` — the
  confirmation policy here is what makes those safe to ship.
- Frecency ranking (fff-style: boost recently/frequently used actions) is
  an explicitly deferred follow-up; the stable `ActionSpec.id` exists
  partly so a future frecency store has a key.
