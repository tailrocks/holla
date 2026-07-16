# Plan 013: User-defined actions and a scriptable CLI surface

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. On any STOP condition, stop and report. When done, update the
> status row in `plans/README.md`.
>
> **Drift check (run first)**: plan 005 must be DONE (ActionSpec/GroupSpec/
> Provider registry, danger confirmations). Verify and STOP if not.

## Status

- **Priority**: P3
- **Effort**: M
- **Risk**: MED (user-supplied commands enter the action system — danger
  handling must be conservative)
- **Depends on**: plans/005-provider-model-async-probe-search.md
- **Category**: direction
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

Two grounded asymmetries:

1. **Providers are hardcoded.** The mission is "adapts to what you have" —
   but a user/team with their own routine ("deploy staging", "reset local
   db") can't teach holla about it without a Rust PR. PRODUCT.md roadmap
   already lists "plugin/user-defined actions" as Later; this makes it real
   while keeping the no-config DEFAULT (zero files needed; config is
   opt-in).
2. **holla is interactive-only.** Everything it knows dies with the TUI.
   `holla docker.clean-all` in a script, or `holla --list --json` for
   tooling, makes the same registry scriptable (Mole's `--json` dual-mode
   idea; also what makes end-to-end testing of the launcher cheap).

## Current state (after 005)

- `ActionSpec { id, label, description, preview, keywords, danger, run }`;
  providers in a fixed registry; destructive → ChoiceDialog.
- CLI: bare `clap::Command::new("holla")` — no subcommands, no flags
  beyond `--version` (`src/main.rs`).
- clap 4.5 already a dependency (derive feature on).

### Design (decided; crate-first)

1. **Config files** (TOML — `toml` crate, MIT/Apache-2.0, serde-based):
   - Global: `~/.config/holla/actions.toml` (`dirs` crate for XDG path).
   - Per-project: `.holla.toml` at the current directory root (its actions
     join the "Current folder" group).

   ```toml
   [[action]]
   id = "team.deploy-staging"        # required, must match ^[a-z0-9._-]+$
   label = "deploy: staging"         # required
   description = "Deploy current branch to staging"
   command = ["./scripts/deploy.sh", "staging"]  # argv array — NO shell string form
   danger = "mutating"               # safe | mutating | destructive (required)
   keywords = ["deploy", "staging"]
   group = "Current folder"          # optional; default per file origin
   confirm = true                    # optional; force confirmation even if not destructive
   ```

   Decisions: argv-array only (no `sh -c` strings — the 002 quoting lesson
   is structural now); `danger` is REQUIRED (no default — the author must
   choose); `destructive` and `confirm = true` both route through the 005
   confirmation dialog; malformed entries are skipped with a Toast warning
   listing file+index (never a crash, never a silent drop); ids colliding
   with built-ins are rejected (built-ins win, Toast warning).
2. **Trust boundary**: per-project `.holla.toml` runs code from a repo you
   cd'd into. Mitigation (decided): first time a given project file HASH is
   seen, its actions render with a `⚠ unreviewed` badge and activation
   opens a one-time trust dialog showing the exact argv; accepted hashes
   persist in `~/.cache/holla/trusted.json`. Changed file → re-prompt.
   Global config is trusted implicitly (user-owned).
3. **CLI surface** (clap subcommands on the existing derive setup):
   - `holla` → TUI (unchanged default).
   - `holla list [--json]` → all detected groups/actions
     (id, label, group, danger); human table or JSON (serde).
   - `holla run <action-id> [--yes]` → execute one action headlessly
     through the SAME registry: streams task output to plain stdout
     (the 004 executor already separates events from rendering — reuse
     `TaskEvent`, print lines). Destructive/confirm actions REQUIRE
     `--yes` (else exit 3 with an explanatory line; no interactive
     fallback in headless mode).
   - `holla doctor` → probe report: what was detected, timings, config
     files loaded/skipped-with-errors.
   - Exit codes: 0 ok, 1 task failed, 2 unknown action, 3 confirmation
     required, 4 config error.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Gates | `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo nextest run --all-features && cargo build` | all exit 0 |
| CLI smoke | `cargo run -- list --json | python3 -m json.tool` | valid JSON |

## Scope

**In scope**: `src/config.rs` (new), `src/providers/user.rs` (new),
`src/main.rs` (subcommands), headless runner path in `src/tui/app.rs`'s
executor module (print-mode consumer of `TaskEvent` — no TUI), trust store,
`Cargo.toml` (+`toml`).

**Out of scope**: full plugin system (dynamic libs, WASM — recorded as
future direction, not this plan); shell-string commands; per-action env/cwd
customization beyond the project root default (add later if asked);
Windows.

## Git workflow

Branch `advisor/013-user-actions-cli`; Conventional Commits + DCO
(`git commit -s`); no push/PR without operator instruction.

## Steps

### Step 1: Config parsing (pure)

`src/config.rs`: serde structs + validation per design 1. Tests: valid
file round-trip; missing `danger` → entry-level error; shell-string
`command = "…"` (non-array) → rejected; bad id charset rejected; collision
with builtin id rejected; error report carries file + entry index. ≥8 tests.

**Verify**: `cargo nextest run --all-features` green.

### Step 2: UserActionsProvider + trust flow

Design 1–2. Global actions join their `group`; project actions join
"Current folder" with the badge/trust dialog. Trust store: hash =
blake3? NO new crypto dep — `sha2` (RustCrypto, MIT/Apache-2.0) or simply
`DefaultHasher`? Decision: `sha2` (stable across runs/versions; DefaultHasher
is not). Tests: unknown hash → untrusted; accepted persists; file edit →
re-prompt (hash change).

**Verify**: tests green; manual: drop a `.holla.toml` in a scratch repo,
run holla → badge + trust dialog on first activation.

### Step 3: CLI subcommands

Design 3. The registry build must run WITHOUT a terminal session for
`list`/`run` (this falls out of 005's split of scan-vs-render — if it
doesn't, that's a 005 layering bug: STOP and report rather than hacking).
Tests: `list --json` schema snapshot; `run` unknown id → exit 2;
destructive without `--yes` → exit 3 (spawn the binary via
`assert_cmd`? — crate-first: yes, `assert_cmd` + `predicates` as
dev-dependencies, both MIT/Apache-2.0).

**Verify**: `cargo run -- list --json` → valid JSON; exit-code tests green.

### Step 4: doctor + docs

`holla doctor` per design 3. README gains a short "Scripting" + "Custom
actions" section (keep README quickstart-thin; details in PRODUCT.md
appendix or docs/ if it grows).

**Verify**: gates green; `cargo run -- doctor` lists probes + configs.

## Test plan

≥18 new tests: config validation (8), trust store (4), CLI exit codes +
JSON (5, via assert_cmd), provider merge/collision (1+).

## Done criteria

- [ ] Zero-config behavior unchanged (no config files → identical launcher;
      existing tests untouched and green).
- [ ] No shell-string command path exists (`grep -rn '"sh"' src/config.rs
      src/providers/user.rs` → none; argv arrays only).
- [ ] `danger` mandatory in config (test-enforced); destructive headless
      runs require `--yes` (test-enforced exit 3).
- [ ] Project-file trust: first-run dialog + persisted hash + re-prompt on
      change (tests + manual smoke recorded).
- [ ] `holla list --json` stable schema documented in the subcommand help.
- [ ] Four gates exit 0; `plans/README.md` row updated.

## STOP conditions

- Registry can't build headlessly without dragging in the terminal session
  (005 layering issue — report, don't hack).
- Any temptation to support `command = "string"` with a shell — the
  argv-only rule is a security decision.
- Trust-dialog UX can't be expressed with ChoiceDialog/ModalStack — note
  the termrock gap for plan 007's follow-up instead of hand-rolling.

## Maintenance notes

- The JSON output of `list` becomes a public interface once scripts use
  it — version it (`"v":1` envelope) from day one.
- The trust store pattern (hash + accept + re-prompt) is reusable if
  plugins ever arrive; keep it in its own module.
- WASM/dylib plugins: explicitly deferred — revisit only with a concrete
  consumer; config actions cover the 90% case.
