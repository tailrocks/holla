# Plan 007: Extend TermRock — multi-select, metadata columns, progress, log pane

> **Executor instructions**: This plan's work happens in the TERMROCK repo
> (`/Users/donbeave/Projects/tailrocks/termrock`), not in holla. Follow
> termrock's own contributor rules (its `AGENTS.md`) — they differ from
> holla's: work directly on `main` (no feature branches), Conventional
> Commits with DCO sign-off, each independently verified change committed
> and pushed when the bootstrap gate is green, and every breaking public
> change adds the next numbered `migrations/` file + `MIGRATING.md` index
> row in the same commit. If anything in "STOP conditions" occurs, stop and
> report. When done, update the status row in holla's `plans/README.md`.
>
> **Drift check (run first)**: in the termrock repo run
> `git log --oneline -5` and read `MIGRATING.md`. The API baseline below was
> verified at `da54a033f368ed0888af90ae43d19bcb96fb8581` (v0.8.0,
> migrations 0001–0002). If newer migrations exist, read them before
> touching anything; adapt file/line references.

## Status

- **Priority**: P2
- **Effort**: L
- **Risk**: MED (public-API design in a shared library; forward-only policy
  means mistakes are cheap to fix but migrations are mandatory)
- **Depends on**: plans/003-termrock-migration-menu.md (holla consumes
  termrock; extension needs land before or with 008)
- **Category**: direction / migration
- **Planned at**: holla commit `ad8a0f1`, 2026-07-16

## Why this matters

holla's disk analyzer (plan 008) and launcher polish need four capabilities
TermRock v0.8.0 does not have. TermRock's product direction (its
`AGENTS.md`) is explicit: "Assume a visual or interaction pattern belongs in
TermRock unless it is provably specific to a consumer's product domain" and
"When a capability is missing, extend or refactor TermRock rather than
implementing a local visual substitute." All four gaps below are
domain-neutral interaction patterns — they belong upstream.

Verified gaps (at v0.8.0):

1. **Multi-select**: `TreeState` and `ListState` hold a single
   `selected: Option<Id>` (`widgets/tree.rs:44`, `widgets/list.rs:39`);
   no checked-set, no toggle outcome, no checkbox rendering anywhere.
2. **Aligned trailing metadata column**: a `TreeNode`/`ListRow` is one
   `label: Line` (`tree.rs:26`, `list.rs:24`); a right-aligned size/count
   cell requires consumer column math (which would be exactly the "copied
   neutral rendering body" AGENTS.md forbids).
3. **Progress indication**: no progress bar / gauge / spinner widget at all
   (the only running cues are `TreeNodeStatus::Loading`'s text suffix,
   `tree.rs:16-21`, and StatusBar text).
4. **Streaming log pane**: `Viewport` renders borrowed lines with
   `DialogScroll` (`widgets/viewport.rs:12-67`) and `TailScroll` exists
   (`scroll/mod.rs:37-72`), but nothing owns append + follow-tail +
   "re-pin on End" as a widget. holla wired it manually in plan 004; that
   wiring is reusable interaction and should move up.

Also noted (NOT in this plan's scope, record as a termrock TODO if desired):
`Theme` is not constructible with custom roles (`style/mod.rs:132-172` —
only `tailrocks_phosphor()`); holla currently doesn't need a different
brand, so leave it.

## Current state (termrock, verified at `da54a03…`)

- Workspace: `crates/termrock` (library) + `crates/termrock-lookbook`
  (catalog binary). Version 0.8.0, edition 2024, Rust ≥1.95,
  `unsafe_code = "forbid"`, clippy correctness/suspicious/perf = deny.
- Every public widget must have: catalog API inventory, contract matrix
  entry, documentation, story, and deterministic preview (AGENTS.md
  "Every public widget must be represented by the catalog's generated API
  inventory, contract matrix, documentation, story, and deterministic
  preview"). Look at how an existing widget is wired into
  `crates/termrock-lookbook/src/{stories.rs,interactors.rs}` and
  `docs/content/docs/components.mdx` and mirror it for each addition.
- Verification gate (from `compatibility.toml` [[verification]] blocks):

  ```bash
  mise x -- cargo fmt --all -- --check
  mise x -- cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  mise x -- cargo nextest run --workspace --all-features --locked
  mise x -- cargo test --doc --workspace --locked
  mise x -- cargo run -p termrock-lookbook -- render --out docs/public/component-previews
  mise x -- cargo run -p termrock-lookbook -- check --dir docs/public/component-previews
  ```

- Widget pattern to follow (read these as exemplars before designing):
  `widgets/list.rs` (borrowed rows + `State` owning interaction + typed
  outcomes + hit regions), `widgets/tree.rs` (same + disclosure regions),
  `widgets/toast.rs` (self-contained builder widget).
- Style: widgets take `&Theme`, use `theme.style(Role::…)` only; IDs are
  caller-owned `Clone + PartialEq`; keyboard handled in `State::handle_key`
  returning a typed outcome enum; mouse via painted `HitRegion`s.

## Commands you will need

(Run inside `/Users/donbeave/Projects/tailrocks/termrock`.)

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Full gate | the 6 commands listed above, in order | all exit 0 |
| Focused tests | `mise x -- cargo nextest run -p termrock --all-features` | pass |

## Scope

**In scope** (termrock repo):
- `crates/termrock/src/widgets/` — extend `tree.rs`, `list.rs`; new
  `progress.rs`, `log_pane.rs`; export from `widgets/mod.rs`
- `crates/termrock-lookbook/src/` — stories + interactors for each change
- `docs/content/docs/components.mdx` — contract lines
- `migrations/0003-*.md` + `MIGRATING.md` — only if a change is breaking
- Version bump in workspace `Cargo.toml` per repo convention (check recent
  release commits for whether minor bumps accompany feature additions)

**Out of scope**:
- holla repo entirely (plan 008 re-pins after this lands).
- Theme constructor/rebranding (noted above, deliberately deferred).
- Fuzzy matching inside termrock — matching is consumer-owned policy
  (nucleo in holla); only match-span RENDERING conventions could ever move
  up, and not in this plan.

## Design (proposals — refine against maintainer taste, keep product-neutral)

### 1. Multi-select for Tree and List

Additive where possible:

```rust
pub struct Selection<Id> { checked: Vec<Id> }   // or HashSet; Vec keeps order
impl<Id: Clone + PartialEq> Selection<Id> {
    pub fn toggle(&mut self, id: &Id);
    pub fn is_checked(&self, id: &Id) -> bool;
    pub fn checked(&self) -> &[Id];
    pub fn clear(&mut self);
}
```

- `TreeState`/`ListState` gain `selection: Option<Selection<Id>>`
  (None = single-select behavior unchanged) OR the widget structs gain a
  `checkable: bool` — pick ONE mechanism, apply to both widgets
  symmetrically.
- Key: `Space` toggles the focused row (`handle_key` gains a
  `Toggle(Id)`-style outcome variant — for List this is a new
  `ListOutcome::CheckToggled(Id)`; Tree already HAS `Toggle(Id)` for
  disclosure, so the new variant must be distinct, e.g. `CheckToggled(Id)`).
- Render: leading cell `[x]` / `[ ]` (or `☑/☐` — match the repo's existing
  non-color-cue vocabulary, cf. Form's `*`/`⊘` markers) before the label;
  checked rows styled with `Role::Accent`.
- Checkbox hit region (click toggles) — mirror how tree.rs separates
  `disclosure_regions` from row regions (`tree.rs:182-205`).

### 2. Trailing metadata cells

```rust
pub struct TreeNode<'a, Id> { /* existing */, pub trailing: Option<Line<'a>> }
pub struct ListRow<'a, Id>  { /* existing */, pub trailing: Option<Line<'a>> }
```

Render right-aligned in a reserved column (width = max trailing width,
clamped; truncate label first, never the trailing cell — sizes must stay
readable). This IS a breaking struct change → migration file 0003 with
before/after (`.. , trailing: None` addition for existing consumers).

### 3. Progress widget

`widgets/progress.rs`:

```rust
pub enum ProgressKind { Determinate { fraction: f64 }, Indeterminate { tick: u64 } }
pub struct Progress<'a> { pub kind: ProgressKind, pub label: Option<&'a str>, pub theme: &'a Theme }
```

Stateless `Widget` (consumer owns tick counter/fraction). Determinate: block
bar (`█`/`░`) + percentage; indeterminate: spinner glyph cycle (braille or
the repo's existing glyph taste) + label. Roles: `Accent` fill, `TextMuted`
track/label.

### 4. LogPane widget

Owns what holla's plan-004 wired manually:

```rust
pub struct LogPaneState { lines: Vec<Line<'static>>, tail: TailScroll, follow: bool }
impl LogPaneState {
    pub fn append(&mut self, line: Line<'static>);   // keeps follow if pinned
    pub fn handle_key(&mut self, key: KeyEvent) -> LogPaneOutcome; // Up/Down/PageUp/PageDown unpin; End re-pins
}
pub struct LogPane<'a> { pub title: Option<&'a str>, pub theme: &'a Theme }
// StatefulWidget rendering via Viewport internals + follow indicator ("⇣ following")
```

Optional cap (`with_max_lines(n)`) evicting from the front. If maintainer
taste says this is just "Viewport + a documented recipe", downgrade to a
documented composition story in the lookbook instead of a widget — either
resolution is acceptable; record which.

## Steps

### Step 1: Read the exemplars and confirm the baseline

Read `AGENTS.md`, `MIGRATING.md` (+ any migration > 0002), `widgets/list.rs`,
`widgets/tree.rs`, `widgets/toast.rs`, lookbook `stories.rs`/`interactors.rs`
wiring for List and Tree, and `docs/content/docs/components.mdx`.

**Verify**: you can run the full 6-command gate on a clean checkout → all
exit 0 (baseline green before touching anything).

### Step 2: Metadata cells (smallest, unblocks 008 sizing display)

Implement design 2 for Tree + List, update all in-repo usages (lookbook
stories get a demo with sizes), write migration `0003` (struct fields
added — breaking for struct-literal construction), unit tests for
right-alignment/truncation with wide Unicode labels (repo has wide-Unicode
fixture conventions — see `18a39b9` "test: keep wide Unicode fixtures
language-neutral" for taste).

**Verify**: full gate green. Migration file exists + MIGRATING.md row.

### Step 3: Multi-select (design 1)

Both widgets, one mechanism; stories showing checked rows; tests for
toggle/clear/click-on-checkbox; contract lines in components.mdx.

**Verify**: full gate green.

### Step 4: Progress widget (design 3)

Widget + story with deterministic preview (indeterminate spinner must render
deterministically in previews — take `tick` as input, never wall-clock:
matches the repo's determinism requirement for previews).

**Verify**: full gate green; `render`+`check` lookbook commands pass.

### Step 5: LogPane (design 4) — or documented composition

Implement or formally downgrade (see design 4). If implemented: tests for
append-while-pinned, unpin-on-scroll, re-pin-on-End, eviction cap.

**Verify**: full gate green.

### Step 6: Record the new revision for holla

Note the resulting termrock `main` SHA. Holla plans 004 (LogPane swap,
optional) and 008 (hard dependency on 1–3) re-pin to ≥ this SHA.

**Verify**: `git log --oneline -6` shows the four features + migration;
working tree clean; pushed per termrock's AGENTS.md rule.

## Test plan

Per feature, colocated tests in the widget files (the repo already has
`widgets/tests.rs` and inline test modules — follow whichever the touched
file uses). Wide-Unicode label + trailing-cell truncation cases mandatory.
Lookbook deterministic previews are themselves regression tests
(`render` + `check`).

## Done criteria

- [ ] All four capabilities merged on termrock `main` (or LogPane formally
      downgraded to a documented composition) with the full 6-command gate
      green.
- [ ] Breaking changes each have a numbered migration file + MIGRATING.md
      index row in the same commit.
- [ ] Every new/changed widget has: story, interactor, deterministic
      preview, components.mdx contract line.
- [ ] No product wording ("holla", "disk", "task") in any termrock API —
      names stay domain-neutral (Selection, trailing, Progress, LogPane).
- [ ] New termrock SHA recorded in holla `plans/README.md` status note.

## STOP conditions

- Newer migrations (>0002) already reshaped List/Tree such that the designs
  above don't fit — re-derive against the new shapes; if the fit is
  unclear, report a design memo instead of code.
- The maintainer-taste calls in designs 1/4 (mechanism choice; widget vs
  recipe) feel underdetermined after reading exemplars — write the two
  options down and report rather than guessing.
- The lookbook determinism check fails for the spinner — do not weaken the
  check; make tick an input.
- Anything requires `unsafe` (workspace forbids it).

## Maintenance notes

- holla plan 008 depends on Steps 2–3 (sizes column + multi-select);
  Step 4 (progress) it also uses; Step 5 is a nice-to-have swap for holla's
  task runner.
- Future consumers (other tailrocks TUIs) get all four — that's the point
  of upstreaming. Review focus: API neutrality and the no-local-substitute
  rule.
