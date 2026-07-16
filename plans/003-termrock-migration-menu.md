# Plan 003: Migrate the launcher menu UI to TermRock v0.8 components

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat ad8a0f1..HEAD -- src/tui/ src/main.rs Cargo.toml`
> Plan 002 legitimately touches the menu-building half of `src/tui/menu.rs`
> (group titles, probe field names). Any OTHER drift in the render/event-loop
> half, `main.rs`, or `Cargo.toml` is a STOP condition.

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P1
- **Effort**: L
- **Risk**: MED (full UI-layer replacement; characterization tests from 002
  protect menu content, not rendering)
- **Depends on**: plans/002-test-baseline-and-bugfixes.md
- **Category**: migration
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

The product mandate (PRODUCT.md): all TUI is built from TermRock components.
Today holla hand-rolls everything — terminal setup/teardown, list rendering,
selection state, borders, footer hints — directly on ratatui + crossterm in
`src/tui/menu.rs` (~280 lines of render/event code). That code duplicates
what TermRock owns (focus behavior, hit geometry, scroll, theme roles,
keymap-driven hints) and has no theming consistency with the rest of the
tailrocks ecosystem. This plan replaces the menu screen's chrome and event
loop with TermRock v0.8 primitives, establishing the integration pattern
(Session, Theme, Keymap, widget States) every later screen (task runner 004,
launcher search 005, disk analyzer 008) builds on.

## Current state

### holla side

- `src/tui/menu.rs:304-445` — `pub async fn run(menu: Menu)`: enables raw
  mode + alternate screen by hand, builds
  `Terminal<CrosstermBackend<Stdout>>`, runs a poll/draw loop with local
  `group_idx`/`action_idx`/`focus_left` state, and on Enter breaks out,
  restores the terminal, and awaits the chosen action handler.
- `src/tui/menu.rs:447-571` — `render_groups`, `render_actions`,
  `render_preview`: hand-built `ratatui::widgets::List`/`Paragraph` with
  hardcoded `Color::Cyan`/`DarkGray` styles.
- Layout: header (3 rows) / body split 20% groups | 40% actions | 40%
  preview / footer (3 rows) — `menu.rs:329-364, 384-396`.
- Key model (`menu.rs:399-434`): Up/Down navigate, Right/Tab/Enter move
  focus right or activate, Left back, `q` quit.
- `src/main.rs:14-16`: `Probe::run()` → `Menu::build(&probe)` →
  `tui::menu::run(menu).await`.
- `Cargo.toml:18-29` current deps include `ratatui = "0.30"`,
  `crossterm = { version = "0.29", features = ["event-stream"] }`,
  `owo-colors`, `tabled`, `indicatif`. (`tabled`, `indicatif`, `owo-colors`
  are currently referenced nowhere in `src/` except — verify with grep — if
  truly unused they are removed in this plan's cleanup step.)

### TermRock side (verified against the termrock repo at rev `da54a03…`, v0.8.0)

**Pin the LATEST main tip at execution time** (operator decision: always the
newest TermRock, even newer than this plan's verified baseline). Resolve it
first:

```bash
git ls-remote https://github.com/tailrocks/termrock.git main   # take the full SHA
```

Then pin (README instruction, `README.md:26-28`):

```toml
termrock = { git = "https://github.com/tailrocks/termrock.git", rev = "<FULL_SHA_FROM_LS_REMOTE>", features = ["crossterm"] }
```

The API excerpts below were verified at `da54a033f368ed0888af90ae43d19bcb96fb8581`
(v0.8.0). TermRock is forward-only with breaking changes: after pinning a
newer rev, read `MIGRATING.md` in the termrock repo and apply every numbered
`migrations/` file AFTER `0002-v0.8.0-canonical-widget-contracts.md`, in
order, adapting the code shapes in this plan accordingly. If a newer
migration removes/renames a widget this plan uses and the migration file
doesn't make the replacement obvious, STOP and report.

Baseline: Rust ≥1.95 (holla is on 1.96 — OK), `ratatui-core 0.1.2`,
`ratatui-widgets 0.3.2`, `crossterm 0.29.0` — matches holla's existing
`ratatui 0.30` (which is built on ratatui-core 0.1.2) and `crossterm 0.29`.
Keep holla's ratatui pinned to `"0.30.2"` to match termrock's workspace.

APIs you will use (all verified):

- **Session** (`crossterm/session.rs:42-85`):
  `Session::enter(io::stdout(), SessionOptions::default())` acquires raw
  mode, alternate screen, mouse capture, bracketed paste, hides cursor;
  `restore()` / `Drop` undoes in reverse (`:87-126,140-144`). Replaces all
  hand-rolled enable/disable calls. `SessionOptions { alternate_screen,
  mouse_capture, bracketed_paste, raw_mode }` all default `true` (`:13-30`).
- **Theme** (`style/mod.rs:132-178`): `Theme::tailrocks_phosphor()` (also
  `Default`); look up styles via `theme.style(Role::…)`; `Role` has 22
  semantic variants (`style/mod.rs:106-130`) — use `Role::BorderFocused`,
  `Role::Border`, `Role::Selection`, `Role::TextMuted`, `Role::Accent`
  instead of hardcoded Cyan/DarkGray.
- **List** (`widgets/list.rs`): `ListRow { id, label: Line, role: RowRole
  {Item,Separator}, enabled }` (`:15-27`); `ListState<Id> { selected:
  Option<Id>, hovered, focused, offset, viewport_height, regions }`
  (`:37-58`); `state.handle_key(rows, key) -> ListOutcome {Ignored, Changed,
  Activated(Id), Cancelled}` handles Up/Down/j/k/Home/End/PageUp/
  PageDown/Enter/Esc (`:78-90`). Render with
  `frame.render_stateful_widget(&List{ rows, theme }, area, &mut state)`.
  IDs are caller-owned, stable, `Clone + PartialEq`.
- **Panel** (`widgets/panel.rs`): `Panel::new(&theme).title(" Preview ")
  .emphasis(PanelEmphasis::Focused)` for bordered frames.
- **Viewport** (`widgets/viewport.rs:12-67`): borrowed `&[Line]` +
  `DialogScroll` state — use for the preview pane so long previews scroll.
- **Keymap** (`keymap.rs:228-270`): static
  `KeyBinding { chords, action, hint, visibility, glyph }` table wrapped in
  `Keymap::new(...)`; `keymap.dispatch(chord) -> Option<A>`; hint spans for
  the footer come from the same table (`hint_spans()`).
- **HintBar** (`widgets/hint_bar.rs`) renders the footer hints;
  **StatusBar** (`widgets/status_bar.rs`) renders the header (left slot:
  `holla`, right slot: cwd).
- **Event bridging** (verified in lookbook `main.rs:592-600`):

  ```rust
  let chord = KeyChord::from(termrock::input::KeyEvent::from(key));
  match KEYMAP.dispatch(chord) { ... }
  ```

  where `key` is a `crossterm::event::KeyEvent` (skip `kind != Press`).
- **Consumer architecture**: TermRock imposes no runtime
  (`docs/content/docs/application-patterns.mdx`); keep holla's simple
  poll/draw loop, just built from these parts. The lookbook
  (`crates/termrock-lookbook/src/main.rs:255-687`) is the canonical example
  of a full loop; its `interactors.rs` shows per-widget wiring
  (List `:93-140`, SplitPane `:328-368`).

Local checkout for reference reading: `/Users/donbeave/Projects/tailrocks/termrock`.

### Design decisions (already made — implement, don't relitigate)

1. **One `ListState<ActionId>` over a single flattened row list** replaces
   the two-pane groups/actions split for the menu's LEFT+MIDDLE panes:
   groups become `RowRole::Separator` rows; actions are `RowRole::Item`
   rows with `id = (group_index, action_index)`. Rationale: TermRock's List
   owns selection/scroll/hit-testing; separators give the grouped look; a
   single list is the layout plan 005's fuzzy search needs (search results
   are a flat filtered list). Keep the preview as the right pane
   (40% width, `SplitPane` optional — a fixed `Layout::horizontal` split is
   fine for now; SplitPane arrives with the task runner in 004).
2. **Footer hints come from the Keymap**, not a hand-built Paragraph.
3. **Menu content code is untouched**: `Menu::build`, `Group`, `Action`
   structs stay exactly as after plan 002 (they are replaced later, in 005).
   Only the presentation half changes.
4. Theme: `Theme::tailrocks_phosphor()`. Do not fork colors. If a needed
   style has no Role, use the closest Role — do NOT hardcode RGB in holla.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Fetch new dep | `cargo build` | exit 0 (first build fetches termrock) |
| Format | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Tests | `cargo nextest run --all-features` | all pass (002's tests stay green) |
| Manual smoke | `cargo run` in a folder with a git repo | menu renders; arrows move; q quits; terminal restored |

## Scope

**In scope**:
- `Cargo.toml` (add termrock; pin ratatui `0.30.2`; remove unused
  `owo-colors`, `tabled`, `indicatif` IF grep confirms unused)
- `src/tui/menu.rs` (render/event half; keep `Menu::build` content intact)
- `src/tui/mod.rs` (exports if needed)
- `src/main.rs` (only if the run signature changes)

**Out of scope**:
- `src/tui/app.rs` — the task runner keeps its old implementation until
  plan 004. It still works because it manages its own terminal lifecycle
  AFTER the menu's Session is dropped.
- `src/probe.rs`, `src/commands/*` — no changes.
- Any fuzzy search / text input — plan 005.
- Any TermRock repo change — if you hit a missing capability, STOP (that's
  plan 007's territory).

## Git workflow

- Branch: `advisor/003-termrock-menu` from `main` (after 002 merged).
- Conventional Commits, DCO sign-off (`git commit -s`). Suggested:
  1. `build: add termrock pinned by git revision`
  2. `refactor(tui): render menu with termrock session, list, and hint bar`
  3. `build: drop unused ui dependencies`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Add the dependency at the latest main revision

Resolve the tip: `git ls-remote https://github.com/tailrocks/termrock.git main`.
Add to `[dependencies]` in `Cargo.toml`:

```toml
termrock = { git = "https://github.com/tailrocks/termrock.git", rev = "<FULL_SHA_FROM_LS_REMOTE>", features = ["crossterm"] }
```

Pin `ratatui = "0.30.2"` (re-check termrock's workspace `Cargo.toml` at the
pinned rev — match its `ratatui-core`/`crossterm` versions if they moved).
If the pinned rev is newer than `da54a03…`, apply any new `migrations/`
files per the note in "Current state".

**Verify**: `cargo build` → exit 0. If the rev fails to resolve, STOP
(report).

### Step 2: Replace terminal lifecycle with Session

In `src/tui/menu.rs::run`, replace the manual
`enable_raw_mode`/`EnterAlternateScreen` block (`menu.rs:310-314`) and the
teardown (`menu.rs:437-438`) with:

```rust
let mut session = termrock::crossterm::Session::enter(
    std::io::stdout(),
    termrock::crossterm::SessionOptions::default(),
)?;
let backend = ratatui::backend::CrosstermBackend::new(session.writer_mut());
let mut terminal = ratatui::Terminal::new(backend)?;
// ... loop ...
drop(terminal);
session.restore()?; // Drop also restores; explicit call surfaces errors
```

Important: the chosen action handler must run AFTER `session.restore()`
(same position as today's post-loop `handler().await` at `menu.rs:440-443`),
because `src/tui/app.rs` sets up its own terminal session.

**Verify**: `cargo run`, press `q` → shell prompt intact (no raw-mode
residue); `cargo clippy --all-targets -- -D warnings` → exit 0.

### Step 3: Rebuild the menu as a flattened TermRock List

Define next to the render code:

```rust
type ActionId = (usize, usize); // (group index, action index)

fn menu_rows<'a>(menu: &'a Menu, theme: &Theme) -> Vec<ListRow<'a, ActionId>> {
    // for each group: one Separator row (label = group title, id = (gi, usize::MAX), enabled = false)
    // then one Item row per action (label = action label, id = (gi, ai), enabled = true)
}
```

State: `ListState::<ActionId>::new(Some(first_item_id))`. Event loop:

- `Event::Key` (Press only) → first give the key to
  `list_state.handle_key(&rows, termrock::input::KeyEvent::from(key))`:
  - `ListOutcome::Activated(id)` → break loop with `Some(id)`.
  - `ListOutcome::Cancelled` → break with `None` (Esc quits, like `q`).
  - `ListOutcome::Changed` → redraw (preview follows selection).
  - `Ignored` → dispatch against the app keymap (Step 4) for `q`.
- Selection identity: `ListState.selected: Option<ActionId>` replaces
  `group_idx`/`action_idx`/`focus_left` — delete those locals.

Render: two-pane horizontal layout — left 60%: the List inside
`Panel::new(&theme).title(" holla ")`; right 40%: preview `Viewport` with
`DialogScroll` state showing the selected action's label, description, and
preview lines (styled with `Role::Accent` for the label, `Role::TextMuted`
for the command lines).

Header: `StatusBar` with left slot `holla`, right slot the cwd (same cwd
logic as `menu.rs:341-343`). Footer: `HintBar` (Step 4).

Delete `render_groups`, `render_actions`, `render_preview`
(`menu.rs:447-571`) and all direct `ratatui::widgets::{List, ListItem,
ListState, Block, Borders}` imports from menu.rs.

**Verify**: `cargo run` → grouped list renders with separator headings;
Up/Down skips separators (List does this — separators are non-selectable);
Enter on an action runs it; preview updates with selection.
`cargo nextest run --all-features` → 002's content tests still pass.

### Step 4: App keymap + hint bar

Define the menu's action enum and static keymap (pattern from lookbook
`main.rs:62-164`):

```rust
#[derive(Clone, Copy, PartialEq)]
enum MenuKey { Quit }

static MENU_KEYMAP: Keymap<MenuKey> = Keymap::new(&[
    KeyBinding { chords: &[KeyChord::plain(LogicalKey::Char('q'))],
                 action: MenuKey::Quit, hint: Some("quit"),
                 visibility: Visibility::Shown, glyph: None },
]);
```

Navigation hints (`↑↓ navigate`, `⏎ run`, `esc quit`) render via the
HintBar; navigation itself is List-owned, so advertise those chords with
`Visibility::Shown` bindings that map to a `MenuKey::Noop`-style action or
use `hint_spans` composition — follow how the lookbook builds its footer
(`main.rs`, search for `hint`). Replace the footer Paragraph
(`menu.rs:384-396`).

**Verify**: footer shows key hints sourced from the keymap; pressing `q`
quits; `cargo clippy` clean.

### Step 5: Dependency cleanup

`grep -rn "owo_colors\|tabled\|indicatif" src/` — if (and only if) zero
matches, remove `owo-colors`, `tabled`, `indicatif` from `Cargo.toml`.

**Verify**: `cargo build && cargo nextest run --all-features` → exit 0.

### Step 6: Full gate + manual smoke

**Verify**:
`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` → all exit 0.
Manual: run `holla` in (a) an empty dir (menu may be small/absent —
"No supported tools" path at `menu.rs:305-308` must still work), (b) this
repo (git actions present), (c) resize the terminal while open (no panic).

## Test plan

- 002's characterization tests must pass unchanged (menu content untouched).
- New unit test for `menu_rows`: given a `Menu` with 2 groups × 2 actions,
  the row vec is `[Separator, Item, Item, Separator, Item, Item]` with
  correct ids and separator rows `enabled == false`. Colocated
  `#[cfg(test)]` module, model after 002's tests.
- Rendering/event behavior: manual smoke (Step 6). Automated TUI snapshot
  tests are deferred (candidate follow-up; termrock-lookbook's deterministic
  preview approach is the pattern if wanted later).

## Done criteria

- [x] `Cargo.toml` pins termrock by full git rev with `features = ["crossterm"]`.
- [x] `grep -n "enable_raw_mode\|EnterAlternateScreen" src/tui/menu.rs` → no
      matches (Session owns lifecycle).
- [x] `grep -n "Color::Cyan\|Color::DarkGray" src/tui/menu.rs` → no matches
      (theme roles only).
- [x] `grep -c "termrock::" src/tui/menu.rs` ≥ 5.
- [x] All four gate commands exit 0; 002 tests + new `menu_rows` test pass.
- [x] Manual smoke checklist from Step 6 done (record results in the status
      row note).
- [x] `src/tui/app.rs` untouched (`git diff --stat` confirms).
- [x] `plans/README.md` status row updated.

## STOP conditions

- The termrock rev fails to fetch/build, or holla's ratatui/crossterm
  versions conflict with termrock's (`cargo tree -d` shows duplicate
  ratatui-core) — report the exact error.
- Menu content tests from 002 fail for reasons other than
  imports/constructors.
- You find yourself re-implementing selection, scrolling, focus, or hit
  geometry locally — that means a TermRock capability is missing; record
  what's missing for plan 007 and STOP.
- Terminal is left corrupted after exit in any smoke scenario.

## Maintenance notes

- This establishes the pattern: Session for lifecycle, Theme roles for all
  styling, Keymap+HintBar for keys, widget `State` structs for interaction.
  Reviewers should reject any new hardcoded colors or hand-rolled key
  handling in later PRs.
- TermRock is forward-only with breaking changes; when bumping the pinned
  rev, read `MIGRATING.md` in the termrock repo and apply numbered migration
  files in order.
- The flattened separator-list is intentionally the same shape plan 005's
  fuzzy-filtered results use — don't reintroduce a two-pane group/action
  split.
