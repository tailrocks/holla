# Plan 009: macOS cleanup insights — developer-storage taxonomy as providers

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **LICENSE RULE (hard)**: the category taxonomy below is informed by
> studying Mole (https://github.com/tw93/Mole, GPL-3.0). Paths, age
> thresholds, and safety concepts are facts/ideas and are fine to use. You
> must NEVER open Mole's source while writing this code, never port its
> functions, never copy strings/lists verbatim from its files. Implement
> from this plan's spec only. Same for any other GPL tool (ncdu, gdu,
> macdirstat).
>
> **Drift check (run first)**: verify plan 008's done criteria (the
> `src/cleanup/` choke point exists with `validate`/`execute`/`DeleteMode`/
> `dry_run`). Missing = STOP.

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P3
- **Effort**: L
- **Risk**: HIGH (destructive surface expands to well-known real user data
  locations; every action is confirmation-gated and choke-point-routed)
- **Depends on**: plans/005-provider-model-async-probe-search.md,
  plans/006-disk-scan-engine.md, plans/008-disk-analyzer-tui.md
- **Category**: direction / feature
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

PRODUCT.md flow #3: holla should KNOW what developer storage is — "Xcode
DerivedData, 48 GB, safe to delete: rebuilt on demand" — instead of making
the user drill through the raw tree. This turns the scan engine + cleanup
core into named, explained, sized, selectable cleanup recommendations,
grouped and searchable like every other action ("cleanup" in the launcher
surfaces all of them). Nothing runs automatically; selection + confirmation
stay mandatory (product invariant).

## Current state

After 005/006/008:

- Providers contribute `GroupSpec { id, title, actions: Vec<ActionSpec> }`;
  `ActionSpec { id, label, description, preview, keywords, danger, run }`;
  destructive actions confirm via ChoiceDialog (005).
- `src/du/::scan(ScanOptions)` sizes any root, streaming, cancellable (006).
- `src/cleanup/::{validate, execute, DeletePlan, DeleteMode, DeleteReport}`
  is the only deletion path; Trash default; dry-run structural; ops.log
  (008).
- Legacy direct-deletion TaskDefs still exist (noted in 008):
  `src/commands/gradle.rs:8-23` and `src/commands/idea.rs:5-22` run
  `find . -name … -exec rm -rf {} +` via `sh`. This plan migrates them.

### The taxonomy (spec — implement from here, not from any GPL source)

New module `src/insights/mod.rs`: a data-driven registry of
`InsightSpec`s. Each is data, not code:

```rust
pub struct InsightSpec {
    pub id: &'static str,               // "xcode.derived-data"
    pub title: &'static str,            // "Xcode DerivedData"
    pub explain: &'static str,          // one sentence: what it is + rebuild cost
    pub safety: Safety,                 // see below
    pub roots: &'static [RootPattern],  // where it lives
    pub min_age_days: Option<u32>,      // only offer items older than this
    pub detect: Detect,                 // Always | ToolOnPath("docker") | DirExists(...)
    pub skip_if_running: Option<&'static str>, // process name, e.g. "Xcode"
}
pub enum Safety {
    Rebuildable,      // "safe to delete — regenerated on demand"
    CacheOldOnly,     // "safe if old — re-downloaded when needed"
    ReviewFirst,      // "look before deleting" (never preselected)
}
```

Initial registry (paths are ~-relative; `$XDG`/env overrides where noted):

| id | roots | safety | age | detect / skip_if_running |
|---|---|---|---|---|
| `xcode.derived-data` | `~/Library/Developer/Xcode/DerivedData` | Rebuildable | — | DirExists / skip "Xcode" |
| `xcode.device-support` | `~/Library/Developer/Xcode/iOS DeviceSupport`, `watchOS…`, `tvOS…` | CacheOldOnly | 90 | DirExists |
| `xcode.archives` | `~/Library/Developer/Xcode/Archives` | ReviewFirst | — | DirExists |
| `simulator.caches` | `~/Library/Developer/CoreSimulator/Caches` | Rebuildable | — | skip "Simulator" |
| `brew.cache` | `~/Library/Caches/Homebrew` | Rebuildable | — | ToolOnPath("brew") |
| `npm.cache` | `~/.npm/_cacache`, `~/.npm/_logs` | Rebuildable | — | ToolOnPath("npm") |
| `pnpm.store` | `~/Library/pnpm/store` (or `pnpm store path` output) | CacheOldOnly | 30 | ToolOnPath("pnpm") |
| `yarn.cache` | `~/.yarn/cache`, `~/Library/Caches/Yarn` | Rebuildable | — | ToolOnPath("yarn") |
| `bun.cache` | `~/.bun/install/cache` | Rebuildable | — | ToolOnPath("bun") |
| `cargo.registry-cache` | `~/.cargo/registry/cache`, `~/.cargo/git` | CacheOldOnly | 30 | ToolOnPath("cargo") |
| `gradle.caches` | `~/.gradle/caches`, `~/.gradle/daemon`, `~/.gradle/wrapper/dists` | CacheOldOnly | 30 | DirExists / skip "gradle" is impractical — instead run `gradle --stop` as a pre-step (see below) |
| `maven.repository` | `~/.m2/repository` | ReviewFirst | — | DirExists |
| `pip.cache` | `~/Library/Caches/pip` | Rebuildable | — | ToolOnPath("pip3") or DirExists |
| `uv.cache` | `~/.cache/uv` or `$XDG_CACHE_HOME/uv` | Rebuildable | — | ToolOnPath("uv") |
| `docker.data` | (special — via `docker system df`, actions call docker CLI) | ReviewFirst | — | ToolOnPath("docker") |
| `user.caches` | `~/Library/Caches` (per-entry listing) | ReviewFirst | 30 | Always |
| `user.logs` | `~/Library/Logs` | Rebuildable | 7 | Always |
| `ide.jetbrains-logs` | `~/Library/Logs/JetBrains` | Rebuildable | 7 | DirExists |
| `project.artifacts` | (special — scan for build dirs, below) | ReviewFirst | 7 | Always |

Never-touch list (hard-coded guard additions to `cleanup::validate` deny
rules, tested): `~/Library/Mobile Documents*` (iCloud), `~/Library/
Application Support` roots as a WHOLE (only enumerated children of caches
are offered), anything under `~/.Trash`, `~/Library/Keychains`,
browser profile data (only `~/Library/Caches/<browser bundle>` cache dirs
may ever appear via `user.caches`).

**`project.artifacts`** (the "purge" idea): scan configured dev roots
(default: `~/Projects` if it exists, plus the current directory's parent)
for artifact dirs named `node_modules`, `target`, `build`, `dist`, `.venv`,
`venv`, `.next`, `.turbo`, `.gradle`, `DerivedData`, `Pods`,
`__pycache__` — counted ONLY when a project indicator file sits beside them
(`package.json`, `Cargo.toml`, `go.mod`, `pyproject.toml`, `pom.xml`,
`build.gradle`, `build.gradle.kts`, `.git`), min age from mtime, nested
artifacts deduped (an artifact inside another artifact isn't double-listed).
Depth cap 6. Uses `src/du/` for sizing.

**`docker.data`**: no filesystem deletion — reuses the EXISTING docker
provider actions (005) and adds `docker.builder-prune`
(`docker builder prune -f`, Destructive) and a `docker system df` sized
preview. Docker's VM disk is Docker's own to manage; never touch
`~/Library/Containers/com.docker.docker` directly (ReviewFirst pointer
only).

**gradle pre-step**: the insight's delete action first runs
`gradle --stop` via the 004 runner (best effort), then deletes through the
choke point — replacing the legacy `find -exec rm` in
`src/commands/gradle.rs`.

### UX (already decided)

- New provider `InsightsProvider` → group "Cleanup" in the launcher.
  Each detected insight = one `ActionSpec` (keywords include "cleanup",
  "cache", tool name) whose `run` opens the **insights screen** for that
  entry — or the group-level action `cleanup.review-all` ("Review all
  cleanup candidates") opens it with everything.
- Insights screen (`src/tui/insights.rs`): a TermRock List (multi-select,
  trailing = sized bytes; sizing computed lazily per-insight via
  `du::scan` on its roots, `Progress` while sizing, largest-first once
  sized). Each row: title + safety badge (`Rebuildable` → `Role::Success`
  "safe — rebuilt on demand"; `CacheOldOnly` → `Role::Warning` "safe if
  old"; `ReviewFirst` → `Role::Warning` "review first", NEVER
  preselected). Enter on a row drills into a per-item list (e.g. individual
  DerivedData project dirs, individual `~/Library/Caches` entries) — also
  multi-select.
- Deletion: identical flow to 008 (same dialog, same choke point, Trash
  default, dry-run available, ops.log). `skip_if_running` insights check
  `pgrep -x <name>` at delete time; running → items become `skipped` with
  reason "App is running".
- Age filtering: items younger than `min_age_days` are shown greyed
  (disabled rows) with "too recent" note, not hidden — honesty over magic.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Tests | `cargo nextest run --all-features` | all pass |
| Build | `cargo build` | exit 0 |

## Scope

**In scope**:
- `src/insights/` (new: registry data + detection + sizing orchestration)
- `src/providers/insights.rs` (new provider)
- `src/tui/insights.rs` (new screen — heavy reuse of 008's analyzer
  components; factor shared pieces into `src/tui/` helpers as needed)
- `src/cleanup/validate.rs` — ADD deny rules from the never-touch list
  (additions only; loosening any rule is a STOP condition)
- `src/commands/gradle.rs`, `src/commands/idea.rs` — migrate legacy
  `find -exec rm` deletion to the choke point
- `Cargo.toml` — crate-first directive: prefer `sysinfo` (MIT) for
  running-process detection (`skip_if_running`) over shelling `pgrep` —
  it's cross-platform (pays off at the Linux port). If `sysinfo`'s startup
  cost for a one-shot process check proves heavy (>50 ms), shelling
  `pgrep -x` is the acceptable fallback; record which shipped.

**Out of scope**:
- sudo/system-domain cleanup (`/Library/Caches`, `/private/var/*`) —
  future plan; user-domain only here.
- App uninstall / leftover-residue scanning (Mole's `mo uninstall`) —
  explicitly deferred; large and riskier.
- Time Machine snapshots (report-only ideas) — deferred.
- Linux paths — the registry is data; Linux rows come with the port.

## Git workflow

- Branch: `advisor/009-cleanup-insights` from `main`.
- Conventional Commits, DCO sign-off. Registry data, screen, and legacy
  migration in separate commits. Do NOT push or open a PR unless the
  operator instructed it.

## Steps

### Step 1: Registry + detection (pure data + tests)

`src/insights/mod.rs` with the table above as `const REGISTRY:
&[InsightSpec]`. Pure functions: `detect(spec, &Probe) -> bool`,
`expand_roots(spec, home) -> Vec<PathBuf>` (env overrides honored),
`is_old_enough(mtime, min_age_days, now) -> bool`. Tests: every registry id
unique; every root expands under an injected fake `$HOME`; never-touch
paths rejected by `cleanup::validate` (drive the 008 validator with each —
this test wires the two modules together).

**Verify**: `cargo nextest run --all-features` → registry tests pass.

### Step 2: Sizing orchestration

`insights::size(spec) -> SizeHandle` — runs `du::scan` per root
(sequentially per insight, insights sized in parallel up to N=3),
aggregates, cancellable. Per-item enumeration for drillable insights
(DerivedData children, Caches children) with per-item sizes and mtimes.

**Verify**: ignored manual test `manual_size_insights_smoke` prints sized
detected insights on this machine; numbers plausible vs `du -sk`.

### Step 3: Provider + insights screen

UX section above. Reuse 008's selection summary + confirm + report
components (factor them out of `analyzer.rs` into shared helpers rather
than duplicating — e.g. `src/tui/confirm.rs`).

**Verify**: manual: `cargo run` → "Cleanup" group present; typing
"cleanup" in search surfaces it; review-all screen sizes progressively;
grey too-recent rows; ReviewFirst rows not preselected.

### Step 4: project.artifacts scanner

Spec above; pure decision function
`classify_artifact(dir_name, siblings, mtime, now) -> Option<Artifact>`
unit-tested (indicator present/absent, nested dedup, age gate), filesystem
walk on fixtures.

**Verify**: fixture test: fake `~/Projects` tree with a real project
(has `package.json` + old `node_modules`) and a decoy (`node_modules` with
no indicator) → exactly one candidate.

### Step 5: Migrate legacy deleters + never-touch hardening

- `src/commands/gradle.rs` / `idea.rs`: replace `find -exec rm` TaskDefs
  with enumeration (walk for `.gradle`/`build` / `.idea`/`*.iml` under cwd,
  same depth caps as today) + `cleanup::execute` (Trash default). Their
  menu actions were already `Destructive` (005) so confirmation holds.
- Add never-touch deny rules to the validator + corpus tests.

**Verify**:
`grep -rn "exec rm\|rm -rf\|rm -f" src/ --include=*.rs` → no matches
outside test fixtures; validator corpus green.

### Step 6: Full gate + smoke

**Verify**: four gates exit 0. Manual smoke on a throwaway fixture HOME if
feasible (`HOME=/tmp/holla-home cargo run` — registry expansion honors
`$HOME`, so a fake home with planted DerivedData works end-to-end without
touching real data). Record in status note: detected insights, one
Trash-mode delete round-trip, ops.log line.

## Test plan

Registry integrity (3+), root expansion under fake HOME (3+), validator
never-touch corpus additions (6+), artifact classifier (5+), sizing
orchestration smoke (1 ignored), legacy-migration behavior (gradle/idea
enumerate-then-delete on fixtures, 2+). ≥20 new tests.

## Done criteria

- [x] `grep -rn "rm -rf\|exec rm" src/ --include=*.rs` → no production
      matches; ALL deletion routes through `src/cleanup/`.
- [x] Every REGISTRY entry has non-empty `explain` and a `Safety`; every id
      unique (test-enforced).
- [x] Never-touch list enforced by validator tests (iCloud, Keychains,
      ~/.Trash, bare Application Support).
- [x] ReviewFirst rows never preselected; too-recent rows disabled not
      hidden (UI code + manual smoke).
- [x] `skip_if_running` produces `skipped` outcomes, not failures (test
      against the test runner's own process name for a deterministic
      positive, via whichever detection mechanism shipped).
- [x] Four gates exit 0; ≥20 new tests; smoke recorded.
- [x] `plans/README.md` status row updated.

## STOP conditions

- Any impulse to open Mole's source "just to check a path" — the spec above
  is the source; if a path seems wrong, verify against the actual filesystem
  on this machine or Apple/tool documentation, and note the correction.
- A registry root resolves to something outside the user domain or inside
  the never-touch list on this machine — fix the spec, don't bypass the
  validator.
- 008's shared components resist reuse without duplication — report the
  refactor needed instead of copy-pasting the confirm/report flow.
- Weakening ANY validator rule.

## Maintenance notes

- The registry is deliberately data-driven: Linux support = new rows +
  `cfg` on detection; new tools = new rows. PRs adding rows must include
  `explain`, `Safety`, and validator-corpus coverage.
- Docker VM internals, sudo/system caches, app-uninstall residue, Time
  Machine snapshot reporting: recorded here as deferred, with the reasons
  (risk, scope). Revisit after the user-domain insights prove out.
- Watch for macOS releases moving cache locations (Mole handles this with
  a monthly audit; holla equivalent = a periodic manual registry review).
