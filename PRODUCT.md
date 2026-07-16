# holla — product brief

## Mission

One command — `holla` — that shows a developer exactly what they can do on
this machine, in this folder, right now. No config, no setup, no memorizing
tool-specific commands. holla scans the system, adapts its menu to what it
finds, and executes chosen actions with live output.

## Platform scope

- macOS first (Apple Silicon and Intel). Linux is a planned second target;
  every subsystem keeps a platform seam (trait or `cfg` boundary) so Linux can
  be added without redesign. Windows is out of scope.
- Terminal-only. No native app, no GUI. All UI is built from the
  [TermRock component library](https://github.com/tailrocks/termrock), pinned
  by git revision. If a UI capability is missing from TermRock, it is added to
  TermRock as a reusable, product-neutral component — never hand-rolled inside
  holla.

## Core concepts

These names form holla's ubiquitous language and must be used in code:

- **Probe**: a fast, non-blocking scan that detects capabilities (installed
  tools, folder context). Never blocks first paint; results stream in.
- **Provider**: a module that, given probe results, contributes Groups of
  Actions (for example, DockerProvider, BrewProvider,
  CurrentFolderProvider).
- **Group**: a named category of actions shown in the launcher (for example,
  “Docker”, “Current folder”, “Disk usage”).
- **Action**: one selectable menu entry: label, description, command preview,
  danger level, keywords for search, and a handler.
- **Danger level**: `Safe` (read-only), `Mutating` (changes state,
  reversible-ish), `Destructive` (deletes data). Destructive actions ALWAYS
  require an explicit confirmation dialog. Nothing is ever deleted
  automatically. This is a product invariant, not a preference.
- **Insight**: a sized finding from the disk analyzer (for example,
  “Xcode DerivedData — 48 GB — safe to delete: rebuilt on demand”).

## Primary user flows

1. **Launcher**: run `holla`; the menu appears instantly (under 100 ms to
   first paint); groups fill in as probes complete; type to fuzzy-search
   across all groups and actions (“dock” finds Docker; “cleanup” finds every
   cleanup action); Enter runs the action; live streaming output follows;
   then a summary appears.
2. **Disk usage**: select “Disk usage”; choose a root (home, current folder,
   custom); watch a progressive scan with live progress; explore a
   size-sorted drill-down tree (largest first); Enter or Right descends and
   Left goes back; noise directories are folded into single rows
   (`node_modules`, `.git`, caches); multi-select entries; review expected
   reclaimed space; explicitly confirm; move to Trash by default or opt into
   permanent deletion; review a per-item result report.
3. **Cleanup insights**: analyzer and providers recognize well-known
   developer storage (DerivedData, brew/npm/pnpm/cargo/gradle caches, Docker
   data, project build artifacts) and label each with what it is and whether
   it is generally safe to remove, with age thresholds. Selection stays
   manual; recommendations never auto-execute.

## Safety invariants

- Every destructive action goes through one validated deletion choke point
  with allow-then-deny path rules. It never permits `/`, `$HOME` itself, or
  system roots.
- Dry-run support is structural: it short-circuits inside the deletion
  wrapper, not at each call site.
- Deletions default to Trash (recoverable); permanent removal is an explicit
  flag in the confirmation dialog.
- Every removal, failure, and skip is written to an operation log under
  `~/.cache/holla/` or `$XDG_CACHE_HOME/holla/`.

## References (ideas only)

- [Mole](https://github.com/tw93/Mole) (GPL-3.0): category taxonomy and
  safety architecture reference. GPL — concepts may be adapted; code must
  NEVER be copied.
- [fff](https://github.com/dmtrKovalenko/fff) (MIT): scanning and matching
  performance reference (macOS QoS pinning, P-core pool sizing).
- dua-cli (MIT), dust (Apache-2.0), and pdu (Apache-2.0): disk-scan
  architecture references.
- Licensing rule: holla is Apache-2.0. Only Apache-2.0, MIT, BSD, or MPL-2.0
  dependencies may be linked. GPL projects are design references only.

## Roadmap

Phases match `plans/`:

1. Test baseline and bug fixes (002).
2. TermRock migration of existing UI (003, 004).
3. Provider/Action model, async probe, fuzzy search, confirmations (005).
4. Disk scan engine (006) and TermRock extensions (007).
5. Disk analyzer TUI (008).
6. Cleanup insights taxonomy for macOS (009).
7. Later: Linux providers, frecency ranking for launcher, plugin and
   user-defined actions, shell completions, and a man page.
