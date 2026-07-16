# Plan 004: Rewrite the task runner — live streaming output, cancellation, TermRock panes

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat ad8a0f1..HEAD -- src/tui/app.rs`
> Plans 002/003 must NOT have touched this file. If it changed, compare the
> excerpts below before proceeding; on mismatch, STOP.

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P1
- **Effort**: L
- **Risk**: MED (process management + concurrent UI; wrong teardown can leak
  child processes)
- **Depends on**: plans/003-termrock-migration-menu.md
- **Category**: bug / migration
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

The README promises "watch it run with live output per task". The current
runner cannot do that — three confirmed defects:

1. **No streaming**: `src/tui/app.rs:89-94` uses `.output().await`, which
   buffers everything until the process EXITS. Output appears only after
   completion. Long-running commands look frozen.
2. **Sequential mode blocks the UI entirely**: the awaiting loop at
   `app.rs:117-124` runs BEFORE the render loop starts, so for sequential
   task lists the screen is blank until every task finishes.
3. **No cancellation**: `q` exits the render loop but children keep running
   (orphaned); follow-style commands can never be offered (002 had to
   downgrade `compose logs -f`).

This plan rebuilds the runner as a streaming, cancellable TermRock screen —
the execution surface every later feature (upgrades, cleanups, deletions)
renders through.

## Current state

`src/tui/app.rs` (219 lines, whole file is the rewrite target):

- Types: `TaskState {Pending, Running, Done(bool)}` (`:21-26`),
  `TaskDef { label, program, args }` + `TaskDef::new` (`:28-42`),
  `RunningTask { label, output: Arc<Mutex<Vec<String>>>, state }` (`:44-48`).
- Public API (used by `src/commands/*` and `src/tui/menu.rs::run_shell`):

  ```rust
  pub async fn run_tasks(tasks: Vec<TaskDef>) -> anyhow::Result<()>        // sequential
  pub async fn run_parallel_tasks(tasks: Vec<TaskDef>) -> anyhow::Result<()> // parallel
  ```

  KEEP these signatures — 7 call sites across `src/commands/{docker,git,
  gradle,idea,mise,upgrade}.rs` and `src/tui/menu.rs:294-302` depend on them.
- Spawning (`:88-115`): `tokio::spawn` +
  `tokio::process::Command::output().await`, stdout+stderr concatenated
  after exit, `TaskState::Done(success)` set under a `std::sync::Mutex`.
- Sequential wait loop before rendering (`:117-124`) — defect 2.
- Render loop (`:128-203`): hand-rolled `ratatui` Tabs + Paragraph with
  manual scroll offset; keys: q quit, ←/h →/l switch tab, ↑k/↓j scroll.
- Exit summary printed to stdout after teardown (`:209-218`).
- Terminal lifecycle hand-rolled (`:59-63`, `:205-206`) — same pattern plan
  003 already replaced in menu.rs.

### TermRock pieces to use (verified at rev `da54a03…`)

- **Session** — as in plan 003 (`crossterm/session.rs:42-144`).
- **Tabs** (`widgets/tabs.rs`): `Tab { id, label, glyph, active, enabled }`
  + `TabsState<Id>` — per-task tab strip with a status glyph per task
  (`○` pending / `◉` running / `✓` ok / `✗` failed — keep the existing
  glyph vocabulary from `app.rs:134-139`; render colors via
  `Role::TextMuted/Accent/Success/Danger`).
- **Viewport** (`widgets/viewport.rs:12-67`): renders borrowed
  `&[Line]` with `DialogScroll` state — the output pane.
- **TailScroll** (`scroll/mod.rs:37-72`): tail-relative offset —
  `offset == 0` means "pinned to bottom"; `scroll_by(filled, delta)`;
  `to_top_offset(content_len, viewport_len)` converts to a top-based offset
  for rendering. Use it so output follows the tail by default and stops
  following when the user scrolls up (classic log-follow behavior).
- **Subscriptions** (`runtime/subscription.rs:16-42`): wrap an
  `std::sync::mpsc::Receiver` in `StdSubscription`; poll each frame:
  `Ready(event)` → apply; `Pending` → render; `Closed` → source done. This
  is TermRock's sanctioned bridge for foreign event sources (tokio tasks).
- **Keymap + HintBar** — same pattern as plan 003 Step 4.
- **ansi_text** (`src/ansi_text.rs` in termrock): `styled_spans` converts
  raw ANSI-colored subprocess output into styled ratatui spans — use it so
  `git`/`docker`/`brew` colored output renders correctly instead of showing
  escape garbage.

### Design decisions (already made)

1. **Streaming**: spawn with `Stdio::piped()` for stdout and stderr; two
   `tokio::spawn` readers per task using
   `tokio::io::BufReader::lines()`; each line is sent as
   `TaskEvent::Line { task: usize, line: String }` over a
   `std::sync::mpsc::Sender` (std, not tokio — the UI side polls via
   `StdSubscription`). Exit status sent as `TaskEvent::Done { task, success }`.
2. **Sequential vs parallel**: one orchestrator `tokio::spawn` drives task
   order — parallel: spawn all; sequential: spawn task N+1 only after N's
   `Done`. The UI thread NEVER awaits tasks; it only polls the channel.
   Defect 2 dies structurally.
3. **Cancellation**: on first `Esc`/`q` press while tasks run → confirm via
   `ChoiceDialog` ("Tasks are still running — stop them?"); on confirm,
   `child.start_kill()` (tokio `Child`) for all running children, statuses
   become `Done(false)` with a "cancelled" note. To also kill grandchildren
   of `sh -c` tasks, spawn children with `process_group(0)` (available on
   `tokio::process::Command` via `std::os::unix::process::CommandExt`
   passthrough) and send SIGTERM to the negative pgid via
   `libc::kill(-pgid, SIGTERM)`. If you implement pgid kill, add
   `libc = "0.2"` to dependencies; if that proves fragile, plain
   `start_kill()` on the direct child is the acceptable fallback — note
   which one you shipped.
4. **Completion**: when all tasks are `Done`, status line shows
   "done — press q to close"; the loop no longer needs the kill dialog.
   Keep the post-exit stdout summary (`app.rs:209-218` behavior).
5. **Layout**: header StatusBar (task counts: running/ok/failed) + Tabs row +
   output Viewport + HintBar footer. No SplitPane in this plan (single
   focused output pane, tabs switch tasks) — revisit in 008 if side-by-side
   is wanted.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Tests | `cargo nextest run --all-features` | all pass |
| Build | `cargo build` | exit 0 |
| Manual smoke | see Step 7 | live output, cancel works, no orphans |

## Scope

**In scope**:
- `src/tui/app.rs` (full rewrite)
- `src/tui/mod.rs` (exports)
- `Cargo.toml` (only if adding `libc` for pgid kill)

**Out of scope**:
- `src/commands/*` and `src/tui/menu.rs` — call sites keep working via the
  preserved `run_tasks`/`run_parallel_tasks` signatures. (Exception: if a
  call-site type import must change, keep it mechanical.)
- Action/Provider model (005), disk analyzer (008).
- TermRock repo changes — if a log-pane widget (append + follow) feels like
  it should exist upstream, note it for plan 007 but build with
  Viewport+TailScroll here.

## Git workflow

- Branch: `advisor/004-task-runner` from `main` (after 003 merged).
- Conventional Commits, DCO sign-off. Suggested:
  1. `feat(tui): stream task output live via piped stdio`
  2. `feat(tui): cancellable task runner on termrock widgets`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Define the event model and executor (no UI yet)

In `src/tui/app.rs` create:

```rust
pub enum TaskEvent {
    Line { task: usize, line: String },   // one output line (stdout or stderr)
    Started { task: usize },
    Done { task: usize, success: bool },
}

struct TaskHandle { child_pid: Option<i32>, /* or the tokio Child for start_kill */ }
```

`fn spawn_tasks(defs: Vec<TaskDef>, parallel: bool, tx: mpsc::Sender<TaskEvent>) -> Vec<TaskHandle>`
implementing design decisions 1–2. Readers must merge stdout and stderr
lines in arrival order (two reader tasks per child sharing `tx`).

Unit-test the executor headlessly (no terminal): run `sh -c "echo a; echo b
1>&2; exit 3"` and assert the channel yields both lines and
`Done { success: false }`; run two sleep-tasks sequentially and assert task 1
`Done` arrives before task 2 `Started`.

**Verify**: `cargo nextest run --all-features` → new executor tests pass.

### Step 2: Replace the run_tui internals

Rewrite `run_tui(task_defs, parallel)`:

- `Session::enter` + `Terminal` (pattern from plan 003 Step 2).
- Per-task UI state: `label`, `lines: Vec<String>`, `state: TaskState`,
  `tail: TailScroll`.
- Frame loop: poll `StdSubscription(rx)` in a drain-loop each tick (apply
  ALL `Ready` events before drawing — batching matters for fast producers);
  then draw; then `event::poll(Duration::from_millis(50))` for input.
- Render: Tabs (selected task), Viewport of the selected task's lines
  passed through `termrock::ansi_text::styled_spans`, with scroll offset
  `tail.to_top_offset(lines.len(), viewport_height)`.

**Verify**: `cargo run` → pick "git: status" in a repo → output appears
immediately, not after exit. `cargo clippy` clean.

### Step 3: Tail-follow scrolling

Keys (via a static `Keymap<RunnerKey>`): ↑/k scroll up (unpins tail), ↓/j
scroll down, End re-pins to tail, ←/h →/l switch task tab (reset nothing —
each task keeps its own `TailScroll`), PageUp/PageDown.

**Verify**: run a task with long output (`sh -c 'seq 1 500'` via any menu
action or a temporary test binary); output follows bottom; ↑ stops
following; End resumes following.

### Step 4: Cancellation

Implement design decision 3. While any task is not `Done`, `q`/Esc opens
`ChoiceDialog` (title "Stop running tasks?", actions Stop/Keep running —
Stop focused; `ChoiceDialogState::handle_key` gives Esc=Cancelled,
Enter=activate, arrows traverse — verified `dialog.rs:238-246`). On Stop:
kill all running children, mark them cancelled, stay on screen to show
final states.

**Verify**: run `sh -c 'sleep 100'` task, press q, confirm Stop → returns to
summary within ~1s; `ps aux | grep 'sleep 100' | grep -v grep` → empty (no
orphan). With pgid kill: `sh -c 'sleep 100 & wait'` also leaves no orphan.

### Step 5: Completion state + summary

When all tasks `Done`: hint bar switches to "q close"; after quit, print the
existing summary format (`app.rs:209-218`: `✓/✗ label [ok/failed]` between
`─` rules) — preserve it, tests in commands land later.

**Verify**: run any quick task → "done" hint appears; summary prints after
exit.

### Step 6: Restore follow-mode logs action

In `src/tui/menu.rs`, change the compose logs action (bounded to
`--tail 200` by plan 002) back to a follow variant now that it's safe:
label `"compose: logs (follow)"`, preview `"$ docker compose logs -f"`,
command `docker compose logs -f` — cancellable via Step 4. Update the 002
characterization test accordingly.

**Verify**: `cargo nextest run --all-features` → all pass.

### Step 7: Full gate + manual smoke

**Verify**:
`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` → all exit 0.
Manual: (a) parallel upgrade with ≥2 tools shows interleaved live output;
(b) sequential docker stop→rm renders task 1 output while task 2 pending;
(c) cancel mid-run leaves no orphan processes; (d) terminal restored after
panic (`RUST_BACKTRACE=1` + induced error) — if not, add a panic hook that
restores the session, mirroring Session::Drop.

## Test plan

- Executor unit tests (Step 1): streaming order, stderr capture, exit codes,
  sequential ordering. These run headless in CI.
- `TailScroll` usage logic: pure-function test for offset math (pin →
  to_top_offset == max; after scroll_by(-3) → offset 3 from tail).
- UI rendering: manual smoke only (Step 7), same policy as plan 003.
- Model tests after `src/probe.rs`'s test module style (from 002).

## Done criteria

- [x] `grep -n "output().await" src/tui/app.rs` → no matches (streaming only).
- [x] `run_tasks` / `run_parallel_tasks` signatures unchanged
      (`grep -rn "run_tasks\|run_parallel_tasks" src/commands/ src/tui/menu.rs`
      compiles without call-site edits beyond imports).
- [x] Sequential runs render before completion (executor test proves
      ordering; smoke (b) proves rendering).
- [x] Cancellation kills children — smoke (c) recorded in status row note.
- [x] All four gate commands exit 0; new tests ≥ 6.
- [x] `plans/README.md` status row updated.

## STOP conditions

- Current-state excerpts don't match `src/tui/app.rs` (drift).
- `tokio::process` + pgid handling requires `unsafe` beyond a single
  `libc::kill` call — report instead of expanding unsafe surface.
- You need to modify more than imports in `src/commands/*`.
- Viewport+TailScroll can't express follow-mode without re-implementing
  scroll math locally — note the gap for plan 007 and STOP if blocked.

## Maintenance notes

- The `TaskEvent` channel is the seam plan 008 reuses for deletion progress
  reporting — keep it output-format-agnostic (lines, not styled spans, in
  events; styling happens at render).
- If TermRock later ships a log-pane widget (plan 007 candidate), replace
  the Viewport+TailScroll wiring here and delete local follow logic.
- Watch: tasks producing megabytes of output — lines are kept in memory;
  acceptable now, add a ring-buffer cap (e.g. 100k lines/task) if it bites.
