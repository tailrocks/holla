# Plan 002: Establish a test baseline and fix the four known behavior bugs

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat ad8a0f1..HEAD -- src/`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **State**: DONE
- **Completed**: 2026-07-17
- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none (001 recommended first, not required)
- **Category**: bug / tests
- **Planned at**: commit `ad8a0f1`, 2026-07-16

## Why this matters

The repo has **zero tests** — CI runs `cargo nextest run --all-features
--no-tests=pass`, which passes vacuously. Plans 003–005 rewrite the whole UI
layer; without characterization tests for the menu-building and probe-parsing
logic, those rewrites can silently change behavior. This plan also fixes four
confirmed bugs that would otherwise be carried into the new architecture:

1. **Wrong repo set**: the "Parent folder" group is gated and previewed on
   repos in `..`, but its actions operate on repos in `.`.
2. **Dead omz feature**: `omz` is a zsh shell function, not a binary;
   `which("omz")` never succeeds, and spawning `omz` as a program would fail
   anyway.
3. **Shell injection/quoting**: repo directory names are interpolated into a
   `sh -c` script; a directory name containing `"` or `$(...)` breaks or
   executes.
4. **`compose: logs -f` hangs forever**: the task runner waits for process
   exit (`.output().await`), and `docker compose logs -f` never exits.
   (The full streaming fix is plan 004; here we only stop offering the
   footgun by removing `-f`.)

## Current state

Repo facts:

- Rust edition 2024, toolchain 1.97.1 (`rust-toolchain.toml`), tokio full,
  clap 4.5, anyhow. Binary `holla`, sources in `src/`.
- CI (`.github/workflows/ci.yml`): `cargo nextest run --all-features
  --color=always --no-tests=pass`, `cargo build`, `cargo fmt --all --check`,
  `cargo clippy --all-targets -- -D warnings`. These are the verification
  gates for every step.
- No test files exist anywhere in `src/` (verified: `grep -rn '#\[test\]'
  src/` → no matches).
- Convention: modules under `src/commands/` expose `pub async fn` helpers
  that build `TaskDef` values and call `run_tasks`/`run_parallel_tasks` from
  `src/tui/app.rs`.

### Bug 1 — repo-set mismatch

`src/probe.rs:116-125` scans the PARENT directory:

```rust
fn discover_parent_git_repos() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("..") else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}
```

`src/tui/menu.rs:150-152` gates the group on that parent list:

```rust
// ── Parent folder ────────────────────────────────────────────────
if probe.git && probe.parent_git_repos.len() > 1 {
    let repo_list = probe.parent_git_repos.join(", ");
```

…but the handlers call `crate::commands::git::pull_all()` etc., and
`src/commands/git.rs:66-76` scans the CURRENT directory:

```rust
fn find_git_repos() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(".") else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}
```

So the menu can show "Pull 5 repos in parallel" (siblings in `..`) while the
action pulls a different set (children of `.`), often zero.

**Decided fix** (matches PRODUCT.md: groups describe the CURRENT folder):
the group is about git repos *inside the current directory* (the
"folder-of-repos" pattern). Rename the probe field to `child_git_repos`,
scan `.`, retitle the group "Repos in this folder", and pass the discovered
repo list from the probe into the `commands::git` functions instead of
re-scanning (single source of truth).

### Bug 2 — omz

`src/probe.rs:44` — `let omz = which("omz").is_ok();` (always false: omz is
a zsh function defined by oh-my-zsh, not on PATH).
`src/commands/upgrade.rs:8` and `:61` spawn `omz` as a program.

**Decided fix**: detect oh-my-zsh by directory: `$ZSH` env var if set, else
`~/.oh-my-zsh` exists. Run the upgrade via its standalone script:
`sh "$ZSH_DIR/tools/upgrade.sh"` (that script is what `omz update` runs
internally). Store the resolved dir in the probe as `omz_dir: Option<PathBuf>`.

### Bug 3 — quoting

`src/commands/git.rs:40-56` (inside `push_all_remotes`) builds a shell script
with `format!` interpolation of `repo`:

```rust
let script = format!(
    r#"if git -C "{repo}" remote get-url gitlab &>/dev/null; then
  echo -e "\e[1;37mPushing {repo}\e[0m"
  git -C "{repo}" push origin && git -C "{repo}" push gitlab
else
  echo -e "\e[1;33m  {repo} has no 'gitlab' remote, pushing to origin only\e[0m"
  git -C "{repo}" push origin
fi"#
);
```

**Decided fix**: eliminate the shell entirely. In Rust: run
`git -C <repo> remote get-url gitlab` via `tokio::process::Command` (arg
vector, no shell); on success push both remotes, otherwise push origin only —
each push its own `TaskDef` with program `git` and args
`["-C", repo, "push", remote]`. The colored warning line becomes part of the
task label/description instead of an echo.

### Bug 4 — logs -f

`src/tui/menu.rs:122-127`:

```rust
current_actions.push(Action {
    label: "compose: logs".into(),
    description: "Follow service logs".into(),
    preview: "$ docker compose logs -f".into(),
    handler: Box::new(|| Box::pin(run_shell("docker compose logs -f"))),
});
```

**Decided fix (interim until plan 004)**: change to
`docker compose logs --tail 200` (bounded, exits), label/description/preview
updated to "Show recent service logs". Plan 004 restores follow-mode with a
cancellable streaming runner.

### Testability refactors needed (mechanical, no behavior change)

- `src/probe.rs:85-114` `discover_mise_tasks()` mixes subprocess I/O with
  parsing. Extract the parsing into
  `fn parse_mise_tasks(stdout: &str) -> Vec<MiseTask>` and call it from
  `discover_mise_tasks`; unit-test the pure function.
- `Menu::build(&Probe)` (`src/tui/menu.rs:39-274`) is already pure given a
  `Probe` value — testable as-is by constructing `Probe` structs by hand.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Format | `cargo fmt --all` then `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --all-targets -- -D warnings` | exit 0 |
| Tests | `cargo nextest run --all-features` | all pass, >0 tests run |
| Build | `cargo build` | exit 0 |

## Scope

**In scope**:
- `src/probe.rs`
- `src/tui/menu.rs` (menu-building code and group title only — do NOT touch
  the render/event-loop half, lines ~294–571; it is replaced wholesale by
  plan 003)
- `src/commands/git.rs`
- `src/commands/upgrade.rs`

**Out of scope**:
- `src/tui/app.rs` (task runner — plan 004 rewrites it; only exception: none)
- `Cargo.toml` dependencies (no new deps needed)
- CI workflows, packaging, docs

## Git workflow

- Branch: `advisor/002-test-baseline` from `main`.
- Conventional Commits, DCO sign-off (`git commit -s`). Suggested commits:
  1. `test: characterize Menu::build and mise task parsing`
  2. `fix: scan current directory for multi-repo group`
  3. `fix: detect oh-my-zsh by directory and run upgrade script`
  4. `fix: drop shell interpolation in push_all_remotes`
  5. `fix: bound compose logs instead of following forever`
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Extract `parse_mise_tasks` and add the first tests

In `src/probe.rs`, split `discover_mise_tasks` so the string parsing lives in
`fn parse_mise_tasks(stdout: &str) -> Vec<MiseTask>`. Add at the bottom of
the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_description() { /* feed "build  # Build the app" style lines */ }

    #[test]
    fn skips_blank_lines_and_handles_missing_description() { /* ... */ }
}
```

Cover: name+description line, name-only line, blank lines, leading `#`
stripping (current behavior at `src/probe.rs:97-106` — preserve it exactly).

**Verify**: `cargo nextest run --all-features` → tests run and pass.

### Step 2: Characterize `Menu::build`

Add `#[cfg(test)] mod tests` at the bottom of `src/tui/menu.rs`. Construct
`Probe` values by hand (all-false baseline, then targeted variants) and
assert on group titles, action labels, and previews. Minimum cases:

- All-false probe → `menu.groups.is_empty()`.
- `docker: true` → a "System" group containing labels
  `"docker: stop all containers"` and `"docker: clean everything"`.
- `git: true, in_git_repo: true` → "Current folder" group contains
  `"git: pull"`, `"git: push"`, `"git: status"`.
- `mise_tasks` with one task → label `"mise: <name>"` and preview
  `"$ mise run <name>"`.

Note: `Probe` currently has no constructor for tests; add
`#[cfg(test)] impl Probe { fn empty() -> Self { ... } }` or derive a helper —
keep it test-only.

**Verify**: `cargo nextest run --all-features` → all pass.

### Step 3: Fix the repo-set mismatch (Bug 1)

1. In `src/probe.rs`: rename `parent_git_repos` → `child_git_repos`; change
   `discover_parent_git_repos()` to read `"."` instead of `".."` and rename
   it `discover_child_git_repos()`; sort the result
   (`vec.sort()`) so menu previews and task order are deterministic.
2. In `src/commands/git.rs`: change `pull_all/push_all/status_all/
   push_all_remotes` to accept `repos: &[String]` and delete
   `find_git_repos()`.
3. In `src/tui/menu.rs:149-190`: retitle the group `"Repos in this folder"`,
   gate on `probe.git && probe.child_git_repos.len() > 1`, and pass
   `probe.child_git_repos.clone()` into the handlers (the closures move a
   clone).
4. Update/extend the Step 2 tests: probe with `child_git_repos =
   vec!["a".into(), "b".into()]` → group titled "Repos in this folder"
   exists; with a single repo → group absent.

**Verify**: `cargo nextest run --all-features` → all pass;
`grep -rn "parent_git_repos\|Parent folder" src/` → no matches.

### Step 4: Fix omz detection and execution (Bug 2)

1. `src/probe.rs`: replace `pub omz: bool` with
   `pub omz_dir: Option<std::path::PathBuf>`, resolved as: `$ZSH` env var if
   it points at an existing dir, else `$HOME/.oh-my-zsh` if it exists, else
   `None`.
2. `src/commands/upgrade.rs`: `run_omz` takes the dir and builds
   `TaskDef::new("oh-my-zsh upgrade", "sh", &[upgrade_script_path])` where
   the path is `<dir>/tools/upgrade.sh`. Same change inside `run_all()`.
3. `src/tui/menu.rs:195,233-240`: gate on `probe.omz_dir.is_some()`; preview
   becomes `$ sh ~/.oh-my-zsh/tools/upgrade.sh`.
4. Test: probe with `omz_dir: Some(...)` → "upgrade: oh-my-zsh" action
   present; `None` → absent.

**Verify**: `cargo nextest run --all-features` → all pass;
`grep -rn 'which("omz")' src/` → no matches.

### Step 5: Remove shell interpolation from push_all_remotes (Bug 3)

Rewrite `push_all_remotes(repos: &[String])` in `src/commands/git.rs`: for
each repo run `tokio::process::Command::new("git").args(["-C", repo,
"remote", "get-url", "gitlab"]).output().await`; on success create two
`TaskDef`s (`push origin`, `push gitlab`), else one (`push origin`) whose
label notes "no gitlab remote". No `sh -c` anywhere in the function.

**Verify**: `grep -n "sh" src/commands/git.rs` → no `sh -c` matches;
`cargo clippy --all-targets -- -D warnings` → exit 0.

### Step 6: Bound compose logs (Bug 4)

`src/tui/menu.rs:122-127`: replace `docker compose logs -f` with
`docker compose logs --tail 200`, label `"compose: logs"`, description
`"Show recent service logs"`, preview `"$ docker compose logs --tail 200"`.

**Verify**: `grep -rn "logs -f" src/` → no matches.

### Step 7: Full gate

**Verify**: `cargo fmt --all --check && cargo clippy --all-targets -- -D
warnings && cargo nextest run --all-features && cargo build` → all exit 0,
test count > 0.

## Test plan

New tests (described in steps 1–4), all in `#[cfg(test)]` modules colocated
with the code (repo has no `tests/` dir; colocated is the convention to
establish). Cases: mise parsing (3+), Menu::build characterization (5+),
repo-group gating (2), omz gating (2). Expect ≥12 tests total.

## Done criteria

- [x] `cargo nextest run --all-features` exits 0 with ≥12 tests.
- [x] `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
      `cargo build` all exit 0.
- [x] `grep -rn "parent_git_repos" src/` → no matches.
- [x] `grep -rn 'which("omz")' src/` → no matches.
- [x] `grep -rn "logs -f" src/` → no matches.
- [x] `push_all_remotes` contains no `sh -c` / `format!`-built script.
- [x] No files outside the in-scope list modified (`git status`).
- [x] `plans/README.md` status row updated.

## STOP conditions

- Current-state excerpts don't match the live code (drift).
- A fix appears to require touching `src/tui/app.rs` (that's plan 004).
- You want to add a dependency to `Cargo.toml`.
- `Menu::build`'s handler closures resist the `repos: &[String]` signature
  change without borrowing gymnastics you're unsure about — report the exact
  compiler error instead of restructuring the Action type (that's plan 005).

## Maintenance notes

- Plan 003 rewrites the render half of `menu.rs` and plan 005 replaces the
  `Action`/`Group` structs; the characterization tests written here protect
  the menu CONTENT (titles/labels/previews) through both rewrites — keep them
  green, updating only imports/constructors.
- The `child_git_repos` sort order (alphabetical) is now part of tested
  behavior.
