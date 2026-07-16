# Plan 015: Linux port readiness — seam audit, design, and first bootable slice (design/spike)

> **Executor instructions**: This is a DESIGN/SPIKE plan: its deliverable
> is a written design + a compiling Linux build with a minimal provider
> set — not feature parity. Follow the steps; record findings in the
> deliverable doc. On any STOP condition, stop and report. When done,
> update the status row in `plans/README.md`.
>
> **Drift check (run first)**: plans 005, 006, 008 should be DONE for the
> audit to be meaningful (the seams this plan audits are theirs). If only
> 005 is done, run the audit against what exists and say so in the doc.

## Status

- **Priority**: P3 (mission-committed, timing flexible)
- **Effort**: L (audit S + design M + bootable slice M)
- **Risk**: LOW (read-mostly; the slice touches cfg boundaries only)
- **Depends on**: plans/005 (hard), 006/008 (soft — audit targets)
- **Category**: direction / migration
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

PRODUCT.md commits: "Linux is a planned second target; every subsystem
keeps a platform seam so Linux can be added without redesign." Debian
packaging ALREADY exists (`Cargo.toml` `[package.metadata.deb]`,
`.github/workflows/release-deb.yml`, apt repo docs) — holla ships Linux
binaries whose TUI works but whose brains (providers, disk correctness,
trash, cleanup taxonomy) are macOS-shaped. This plan turns "planned" into
an audited, sequenced design plus a proof slice, so the port is a set of
bounded tasks instead of a rewrite discovered too late. TermRock's
supported baseline is Linux+macOS (its canonical render platform is
Linux), so the UI layer is already portable.

## Current state

- CI runs on `ubuntu-latest` (build+test already green on Linux).
- `rust-toolchain.toml` carries Linux targets
  (`aarch64/x86_64-unknown-linux-gnu`); release-deb workflow builds via
  zigbuild for glibc 2.17.
- Known macOS-specific code after 005–009 (audit confirms the full list):
  `src/du/platform.rs` (iopolicy/QoS/sysctl — already
  `#[cfg(target_os = "macos")]`), firmlink + `Mobile Documents` skip
  rules (006), `trash` crate (cross-platform by design — verify its XDG
  behavior), Spotlight `mdfind` (011, degrades gracefully by design),
  insights registry paths (009 — macOS-only rows), `open`/`open -R`
  (012), brew-centric system provider (005).

### Design questions the deliverable must answer

1. **Provider matrix**: for each provider, one of: works-as-is / needs
   Linux variant / macOS-only-hide. Linux additions worth designing now:
   apt/dnf upgrades, systemd user services, XDG cache locations for the
   insights registry (`~/.cache/*`), Docker paths on Linux.
2. **Disk engine on Linux**: what replaces the macOS thread init (nothing —
   no-op is correct); ext4/btrfs/xfs st_blocks semantics (same 512-unit —
   confirm); btrfs compression/CoW caveats (document like APFS clones);
   `/proc`, `/sys`, mounted-network-fs skip rules (equivalent of firmlink
   list — design the default `skip_paths` for Linux: pseudo-filesystems by
   fstype from `/proc/mounts`, not by path guess).
3. **Trash**: `trash` crate implements the FreeDesktop spec — verify
   behavior for cross-filesystem items and sudo-less operation; decide the
   report copy.
4. **Openers**: `open`/`open -R` → `xdg-open` and file-manager reveal
   (no universal reveal — decide fallback: open parent dir).
5. **Process detection** (009 `skip_if_running`): `sysinfo` already
   cross-platform — confirm.
6. **Packaging/terminal matrix**: min terminal expectations on Linux match
   TermRock's Ghostty-class truecolor baseline — document that holla
   inherits it (no NO_COLOR path, per TermRock README).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Gates (mac) | `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` | all exit 0 |
| Linux check | `cargo zigbuild --target x86_64-unknown-linux-gnu` (mise has cargo-zigbuild) or CI | exit 0 |
| Linux tests | CI (`ubuntu-latest` job) | green |

## Scope

**In scope**:
- `docs/linux-port.md` (new — the design deliverable answering questions
  1–6 with a sequenced task list + effort estimates)
- The bootable slice: cfg-gating whatever currently breaks Linux compile
  (expected: nothing — CI is green; the slice is about RUNTIME correctness
  of the launcher + basic providers on a real Linux box/container)
- `src/providers/` — hide macOS-only providers on Linux
  (`#[cfg]` registration), add NOTHING new except trivial gating
- Insights registry (009): mark macOS-only rows so Linux shows an honest
  empty state, not wrong paths

**Out of scope**: implementing apt/systemd/XDG providers and the Linux
insights rows (those become follow-up plans FROM the design doc); Wayland
clipboard specifics; Windows.

## Git workflow

Branch `advisor/015-linux-readiness`; Conventional Commits + DCO
(`git commit -s`); no push/PR without operator instruction.

## Steps

### Step 1: Seam audit

Sweep for platform assumptions:
`grep -rn "cfg(target_os\|/Library/\|mdfind\|open -R\|sysctl\|brew\|xattr\|\.Trash" src/`
plus the design-question list. Output: the provider matrix + per-module
verdict table in `docs/linux-port.md`.

**Verify**: doc section exists; every `src/` module has a row.

### Step 2: Answer design questions 2–6

Research (crate docs, kernel/FreeDesktop docs) + record decisions with
sources. Where a decision needs runtime proof, note it for Step 3.

**Verify**: each question has a decision + rationale + source in the doc.

### Step 3: Bootable slice on real Linux

Run holla (container or VM: `docker run -it -v $PWD:/w -w /w rust:1.96
bash` then build; TUI needs a real pty — `docker run -it` provides one).
Checklist: launcher paints; git/current-folder/cargo providers work;
task runner streams; disk analyzer scans `/tmp` fixture correctly
(st_blocks totals vs `du -sk`); trash crate round-trip into XDG trash;
macOS-only groups absent; no wrong-path insights.

**Verify**: checklist results recorded in the doc with exact
container/image used; failures become tasks in the doc's sequenced list.

### Step 4: Gate + sequence the follow-ups

Finish `docs/linux-port.md` with the ordered follow-up plan list
(apt/systemd providers, XDG insights rows, Linux skip_paths, statx tuning)
each with S/M/L estimate — candidates for future `plans/0xx` files.

**Verify**: mac gates green; CI green; doc complete.

## Test plan

- Gating tests: on Linux CI, macOS-only providers absent from the registry
  (cfg-compiled test); on macOS unchanged.
- The Step-3 checklist is the acceptance test — recorded, not automated
  (automating TUI-in-container is a follow-up).

## Done criteria

- [ ] `docs/linux-port.md` answers all 6 design questions with sources and
      contains the provider matrix + sequenced follow-up list.
- [ ] holla builds AND was manually exercised on Linux (record image/
      distro); launcher + runner + a fixture disk scan verified.
- [ ] macOS-only surface is cfg-hidden, not broken, on Linux (test).
- [ ] No feature regressions on macOS (full gates green).
- [ ] `plans/README.md` row updated.

## STOP conditions

- The audit finds a macOS assumption BAKED into a public type (not
  cfg-seamable) — that's a design flaw to report against the owning plan,
  not to patch here.
- Runtime slice reveals termrock rendering issues on the Linux terminal —
  report upstream (termrock's canonical platform is Linux; likely holla's
  fault, but verify before assuming).

## Maintenance notes

- Every new provider/insight/plan from here on fills in its own row of the
  provider matrix — reviewers should ask "what does this do on Linux?" at
  PR time.
- The doc's follow-up list is the Linux milestone backlog; renumber into
  `plans/` when scheduled.
