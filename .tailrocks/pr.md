# Repository PR conventions

## Base branch

Pull requests target `main`.

## Branching

One branch per change, cut from the current tip of `main`, named with a
`fix/`, `feat/`, `docs/`, `chore/`, or `refactor/` prefix.

## Commits

Subjects follow Conventional Commits (`fix(scope): ...`, `feat: ...`).

Every branch must record the exact `main` commit it was created from, so that
once `main` moves it stays clear from which version to analyze the history of
changes. Add this trailer to every commit on the branch:

    Based-on-main: <full 40-character sha of origin/main at branch time>

The PR body states the same sha up front ("Branched from main@<short sha>").

## Checks

Run the repository gates before opening and before merging (mirrors
`mise.toml`):

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo nextest run --all-features --no-tests=pass
cargo check --all-targets --all-features --locked
```

## Before merge

- Every code-review finding on the pull request is resolved first: automated
  reviews (e.g. Codex) and human review comments must each be addressed by a
  change or explicitly answered — never merged over.
- CI checks are green on the head commit.
