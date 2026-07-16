# Plan 014: Provider expansion pack — project task runners, cargo, brew services, git hygiene

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. On any STOP condition, stop and report. When done, update the
> status row in `plans/README.md`.
>
> **Drift check (run first)**: plan 005 must be DONE (Provider registry,
> ActionSpec, danger levels, streaming scan). Verify and STOP if not.

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P3
- **Effort**: M
- **Risk**: LOW-MED (each provider is additive and isolated; danger levels
  must be assigned conservatively)
- **Depends on**: plans/005-provider-model-async-probe-search.md
- **Category**: direction
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

Mission: "adapts to what you have installed." The mise-tasks provider
proved the highest-value pattern holla has — *surface the project's own
runnable tasks* — but it stops at mise. The same folders carry
`package.json` scripts, `justfile` recipes, `Makefile` targets, `Taskfile`
tasks; the same machines carry cargo projects, brew services, and stale
git branches. Each is the identical shape: detect → enumerate → offer as
searchable actions through the existing runner and confirmation flow. This
plan is deliberately a batch: one pattern, several thin providers.

## Current state (after 005)

- `src/providers/` with `Provider::scan(&self) -> Option<GroupSpec>` run on
  worker threads (blocking subprocess calls allowed inside `scan`).
- Exemplar to copy: the mise-tasks flow — detection (`which` + file
  presence), enumeration (subprocess, parsed by a PURE tested function),
  `ActionSpec` per item (from plan 005 Step 1; parser pattern originally
  `parse_mise_tasks` from plan 002).
- Danger vocabulary + confirmation flow established (005); `serde_json`
  available (008/013); toml available if 013 landed (else add it here).

### Providers to add (spec)

| Provider | Detect | Enumerate | Actions (danger) |
|---|---|---|---|
| `node_scripts` | `package.json` present | `scripts` map via serde_json (pure parse, tested) | `npm run <name>` — or the right agent: `pnpm-lock.yaml`→pnpm, `yarn.lock`→yarn, `bun.lockb`→bun, else npm (Mutating) |
| `just` | `which just` + `justfile`/`.justfile` | `just --summary` (space-separated names; pure parse) | `just <recipe>` (Mutating) |
| `make` | `which make` + `Makefile` | `make -qp` is messy — use documented targets only: parse `^([a-zA-Z0-9_-]+):` lines from the Makefile itself, exclude `.PHONY`-declared-only-special and pattern rules (pure parse, tested, conservative: skip targets starting with `.` or containing `%`/`$`) | `make <target>` (Mutating) |
| `taskfile` | `which task` + `Taskfile.yml` | `task --list --json` (serde_json; it has stable JSON output) | `task <name>` (Mutating) |
| `cargo_project` | `Cargo.toml` present + `which cargo` | static action set | `cargo build`, `cargo test`, `cargo clippy` (Mutating); `cargo clean` (Destructive — deletes target/) |
| `brew_services` | `which brew` + `brew services list --json` non-empty | JSON parse | per service: start/stop/restart (Mutating) |
| `git_hygiene` | in a git repo | `git branch --merged <default>` minus default/current (pure parse) | "delete merged branches" (Destructive, confirmation lists the exact branches in the dialog body); `git fetch --prune` (Mutating); `git gc` (Mutating) |

Rules for ALL of the above:

- Enumeration output parsing = pure function + unit tests with real
  captured fixtures (copy the mise pattern). Subprocess failures →
  provider contributes nothing (never an error dialog at startup).
- Every subprocess uses argv arrays (no `sh -c` — 002's structural rule).
- Keywords: include tool name + verbs ("test", "build", "cleanup",
  "services") so 005's search surfaces them naturally.
- Item caps: enumerated lists cap at 30 entries per provider (a 400-target
  Makefile must not flood the launcher); cap noted in the group title
  ("Make (30 of 412)").
- `node_scripts` runs scripts only from the CURRENT project dir the user
  launched in (same trust posture as mise today; the 013 trust dialog does
  NOT apply to ecosystem-standard files — document this stance in the
  provider's module doc).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Gates | `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` | all exit 0 |

## Scope

**In scope**: `src/providers/{node_scripts,just,make,taskfile,cargo,
brew_services,git_hygiene}.rs`, registry additions, fixtures for parsers,
`Cargo.toml` (nothing new expected beyond serde_json; `serde_yaml` is NOT
needed — Taskfile enumeration uses its `--json` flag).

**Out of scope**: gradle/maven task enumeration (gradle daemon startup is
seconds — bad fit for scan; revisit with caching); kubernetes (context
switching is high-blast-radius; needs its own safety design); docker
compose service enumeration (compose actions exist; per-service granularity
later); Windows anything.

## Git workflow

Branch `advisor/014-provider-expansion`; one commit per provider
(`feat(providers): surface package.json scripts` …); DCO (`git commit -s`);
no push/PR without operator instruction.

## Steps

### Step 1: node_scripts (the exemplar for the batch)

Spec row 1 incl. agent selection by lockfile. Tests: parse fixture
package.json (scripts present/absent/empty; non-string values skipped);
agent pick per lockfile combination (4 cases); cap logic.

**Verify**: `cargo nextest run --all-features` green; manual: holla in a
JS repo lists `npm run …` actions under Current folder.

### Step 2: just, make, taskfile

Spec rows 2–4, same shape. Make parser: conservative rules from the spec,
fixture with pattern rules/specials proving exclusions.

**Verify**: parser tests green (≥3 per provider); manual in this repo:
none of the three appear (no justfile/Makefile/Taskfile here) — absence is
the assertion.

### Step 3: cargo_project + brew_services

Spec rows 5–6. `cargo clean` MUST be `Danger::Destructive` (test-enforced,
like 005's docker test). brew services JSON fixture test.

**Verify**: tests green; manual: holla in this repo shows cargo actions;
`cargo clean` prompts confirmation.

### Step 4: git_hygiene

Spec row 7. Default-branch detection: `git symbolic-ref
refs/remotes/origin/HEAD` with fallback to `main`/`master` presence (pure
logic, tested). The confirmation dialog body lists every branch that will
be deleted (reuse 005's dialog; body is generated per invocation).

**Verify**: fixture-repo test for the merged-branch parse + exclusion of
current/default; manual smoke in a scratch repo with a merged branch.

### Step 5: Full gate + launcher feel check

**Verify**: gates green. Manual: launcher in a busy repo — groups remain
scannable (caps working), search "test" surfaces node/cargo/just test
actions together; startup latency unchanged (providers run on workers —
spot-check that no new provider runs subprocesses before first paint;
005's non-blocking contract covers this if followed).

## Test plan

≥20 new tests: parsers with fixtures (node 3, make 3, just 2, taskfile 2,
brew 2, git 4), danger assignments (2), caps (2). All colocated per
convention.

## Done criteria

- [x] Seven providers registered; each contributes only when detected
      (absence tests included).
- [x] All enumeration parsers are pure functions with fixture tests.
- [x] `grep -rn '"sh", "-c"\|"sh".into()' src/providers/` → no matches
      (argv arrays only).
- [x] `cargo clean` and "delete merged branches" are Destructive
      (test-enforced) and confirm with exact-content dialogs.
- [x] Per-provider caps enforced (test) and surfaced in group titles.
- [x] Four gates exit 0; `plans/README.md` row updated.

### Completion evidence (2026-07-17)

- Seven providers registered; 31 focused provider tests plus an empty-project,
  empty-PATH integration test cover parsing, detection absence, caps, and danger.
- Warm `brew services list --json` measured 2.58 s, tripping the 200 ms STOP
  threshold. A versioned five-minute cache removes Homebrew startup from the
  normal path; cold refresh remains isolated on its post-first-paint worker.
- Scratch Git smoke detected one merged branch, blocked deletion without
  `--yes`, deleted only that branch with `--yes`, and preserved `main`.
- `cargo fmt --all --check`, clippy with warnings denied, 253/253 nextest tests,
  `cargo build`, and the argv-only grep passed. PTY smoke painted while scans
  ran, search `test` surfaced Cargo and cross-provider matches, and terminal
  state restored cleanly.

## STOP conditions

- A provider needs >200ms enumeration on the happy path (e.g. a slow tool)
  — don't ship it slow; report for a caching design instead.
- Makefile parsing rules produce garbage on a real-world fixture — tighten
  toward "fewer, correct targets"; if still bad, drop the make provider and
  record why (conservative beats wrong).
- Any action would delete data without a Destructive marking.

## Maintenance notes

- This file's spec table is the checklist for future providers (Linux port
  adds apt/systemd rows; same rules apply).
- Gradle/maven enumeration deferred pending a caching story (011's cache
  module is the likely host).
- If provider count makes the empty-query launcher noisy, plan 010's
  frecency "Recent" group is the intended counterweight — land it.
