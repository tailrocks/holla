# Plan 001: Write PRODUCT.md — holla's mission, scope, UX flows, and roadmap

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat ad8a0f1..HEAD -- README.md PRODUCT.md`
> If `PRODUCT.md` already exists, treat that as a STOP condition (someone else
> wrote it — reconcile, don't overwrite).

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: direction / docs
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

holla is about to grow from a 1.2k-LOC menu into an adaptive dev-environment
launcher with a disk-usage analyzer. Eight follow-up plans (002–009) each need
the same product context: what holla is, what it must never do (delete without
confirmation), and which vocabulary to use in code. Writing the product brief
first gives every executor and reviewer one authoritative source of intent and
stops scope drift. The audit playbook treats a PRD as the strongest grounding
signal for future direction work.

## Current state

- `README.md` (repo root) — describes the current tool: "Adaptive dev
  environment CLI. Run `holla` in any directory and get an interactive menu
  showing exactly what you can do — based on what tools are installed on your
  machine." It documents 5 groups (macOS, Git, Docker, Gradle, IntelliJ IDEA)
  and install instructions. Keep it as the user-facing quickstart; PRODUCT.md
  is the deeper product brief that README links to.
- No `PRODUCT.md`, no `docs/` product content exists (only
  `docs/debian-apt-repo.md`, which is packaging documentation — out of scope).
- `Cargo.toml:6` description: "Adaptive dev environment CLI — adapts to what
  you have installed".

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format check (unchanged code) | `cargo fmt --all --check` | exit 0 |
| Sanity build | `cargo build` | exit 0 |

(This plan only adds Markdown; the build commands just prove you didn't touch
code.)

## Scope

**In scope** (the only files you may create/modify):
- `PRODUCT.md` (create)
- `README.md` (add one link line to PRODUCT.md, nothing else)
- `plans/README.md` (status row update)

**Out of scope**:
- Any `.rs` file, `Cargo.toml`, CI workflows, `docs/debian-apt-repo.md`.

## Git workflow

- Branch: `advisor/001-product-mission-doc` from `main`.
- Conventional Commits with DCO sign-off, e.g.
  `git commit -s -m "docs: add PRODUCT.md product brief"`.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Create PRODUCT.md with the following required content

Write `PRODUCT.md` covering every section below. The content decisions are
already made — flesh out prose, do not invent new scope:

```markdown
# holla — product brief

## Mission
One command — `holla` — that shows a developer exactly what they can do on
this machine, in this folder, right now. No config, no setup, no memorizing
tool-specific commands. holla scans the system, adapts its menu to what it
finds, and executes chosen actions with live output.

## Platform scope
- macOS first (Apple Silicon and Intel). Linux is a planned second target;
  every subsystem keeps a platform seam (trait or cfg boundary) so Linux can
  be added without redesign. Windows is out of scope.
- Terminal-only. No native app, no GUI. All UI is built from the TermRock
  component library (https://github.com/tailrocks/termrock), pinned by git
  revision. If a UI capability is missing from TermRock, it is added to
  TermRock as a reusable, product-neutral component — never hand-rolled
  inside holla.

## Core concepts (ubiquitous language — use these names in code)
- **Probe**: a fast, non-blocking scan that detects capabilities (installed
  tools, folder context). Never blocks first paint; results stream in.
- **Provider**: a module that, given probe results, contributes Groups of
  Actions (e.g. DockerProvider, BrewProvider, CurrentFolderProvider).
- **Group**: a named category of actions shown in the launcher (e.g.
  "Docker", "Current folder", "Disk usage").
- **Action**: one selectable menu entry: label, description, command preview,
  danger level, keywords for search, and a handler.
- **Danger level**: `Safe` (read-only), `Mutating` (changes state,
  reversible-ish), `Destructive` (deletes data). Destructive actions ALWAYS
  require an explicit confirmation dialog. Nothing is ever deleted
  automatically. This is a product invariant, not a preference.
- **Insight**: a sized finding from the disk analyzer (e.g. "Xcode
  DerivedData — 48 GB — safe to delete: rebuilt on demand").

## Primary user flows
1. **Launcher**: run `holla` → menu appears instantly (<100 ms first paint)
   → groups fill in as probes complete → type to fuzzy-search across all
   groups and actions ("dock" finds Docker; "cleanup" finds every cleanup
   action) → Enter runs the action → live streaming output → summary.
2. **Disk usage**: select "Disk usage" → choose a root (home, current
   folder, custom) → progressive scan with live progress → size-sorted
   drill-down tree (largest first), Enter/→ descends, ← goes back →
   noise dirs are folded into single rows (node_modules, .git, caches) →
   multi-select entries → delete flow shows expected reclaimed space →
   explicit confirmation → Trash by default, permanent delete opt-in →
   per-item result report.
3. **Cleanup insights**: analyzer and providers recognize well-known
   developer storage (DerivedData, brew/npm/pnpm/cargo/gradle caches, Docker
   data, project build artifacts) and label each with what it is and whether
   it is generally safe to remove, with age thresholds. Selection stays
   manual; recommendations never auto-execute.

## Safety invariants
- Every destructive action goes through one validated deletion choke point
  (allow-then-deny path rules; never `/`, `$HOME` itself, system roots).
- Dry-run support is structural (short-circuits inside the deletion wrapper,
  not per-call-site).
- Deletions default to Trash (recoverable); permanent removal is an explicit
  flag in the confirm dialog.
- Every removal/failure/skip is written to an operation log under
  `~/.cache/holla/` (or `$XDG_CACHE_HOME/holla/`).

## References (ideas only)
- Mole (https://github.com/tw93/Mole, GPL-3.0): category taxonomy and safety
  architecture reference. GPL — concepts may be adapted; code must NEVER be
  copied.
- fff (https://github.com/dmtrKovalenko/fff, MIT): scanning/matching
  performance reference (macOS QoS pinning, P-core pool sizing).
- dua-cli (MIT), dust (Apache-2.0), pdu (Apache-2.0): disk-scan architecture
  references.
- Licensing rule: holla is Apache-2.0. Only Apache-2.0/MIT/BSD/MPL-2.0
  dependencies may be linked. GPL projects are design references only.

## Roadmap (phases match plans/)
1. Test baseline + bug fixes (002)
2. TermRock migration of existing UI (003, 004)
3. Provider/Action model, async probe, fuzzy search, confirmations (005)
4. Disk scan engine (006) + TermRock extensions (007)
5. Disk analyzer TUI (008)
6. Cleanup insights taxonomy for macOS (009)
7. Later: Linux providers, frecency ranking for launcher, plugin/user-defined
   actions, shell completions, man page.
```

**Verify**: `test -f PRODUCT.md && wc -l PRODUCT.md` → file exists, ≥80 lines.

### Step 2: Link it from README.md

Add exactly one line to `README.md` after the opening paragraph (after line 5
"No config. No setup. ..."):

```markdown
Product brief, mission, and roadmap: see [PRODUCT.md](PRODUCT.md).
```

**Verify**: `grep -n "PRODUCT.md" README.md` → one match.

### Step 3: Confirm no code touched

**Verify**: `git status --porcelain` → only `PRODUCT.md`, `README.md`,
`plans/README.md` listed. `cargo build` → exit 0.

## Test plan

None (docs only). Verification via commands above.

## Done criteria

- [x] `PRODUCT.md` exists and contains all sections: Mission, Platform scope,
      Core concepts, Primary user flows, Safety invariants, References,
      Roadmap.
- [x] The GPL "ideas only, never code" rule is present verbatim in the
      References section.
- [x] `grep -c "Destructive" PRODUCT.md` ≥ 1 (confirmation invariant stated).
- [x] `README.md` links to PRODUCT.md; no other README changes
      (`git diff README.md` shows exactly one added line).
- [x] `cargo build` exits 0.
- [x] `plans/README.md` status row updated.

## STOP conditions

- `PRODUCT.md` already exists.
- You feel the need to change any `.rs` or `Cargo.toml` file.
- README structure differs so much from the excerpt that the insertion point
  (after the "No config. No setup." paragraph) can't be found.

## Maintenance notes

- Plans 002–009 quote this document's vocabulary (Probe, Provider, Group,
  Action, Danger level, Insight). If terminology changes here, grep the other
  plans and code for the old term.
- Future features must be checked against "Safety invariants" — a PR that
  deletes anything without the choke point + confirmation violates the brief.
