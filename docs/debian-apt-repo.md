# holla — Debian package + apt-native repository (design)

Goal: install and upgrade `holla` (the adaptive dev environment CLI) with native apt:

```bash
sudo apt update && sudo apt install holla   # install
sudo apt upgrade                            # upgrade later
```

Own repository, hosted on GitHub (GitHub Pages), built + signed in CI on tag.

## Pieces

1. **Build the `.deb`** — `cargo-deb` (Rust-native; reads `[package.metadata.deb]` in `Cargo.toml`).
   - Package contents: binary → `/usr/bin/holla`.
   - `maintainer-scripts`: `debian/postinst` (simple message on configure).
   - No complex conffiles or users; pure CLI tool.
   - `depends = "$auto"` for library deps (resolved against latest Debian in the dedicated release-deb build).

2. **Build the apt repository** — `reprepro` (standard, simple, signs Release).
   - `conf/distributions` (in the dedicated `tailrocks/holla-apt` repo):
     ```
     Origin: Holla
     Label: Holla
     Codename: stable
     Architectures: amd64 arm64
     Components: main
     Description: apt repository for holla — adaptive dev environment CLI
     SignWith: <GPG key id>
     ```
   - `reprepro includedeb stable holla_*.deb` → builds `dists/` + `pool/`, generates `Packages`, `Release`, and a **GPG-signed** `InRelease`.

3. **Sign** — a dedicated GPG signing key.
   - Private key stored as GitHub Actions secrets in the `holla-apt` repo (`APT_GPG_PRIVATE_KEY`, `APT_GPG_PASSPHRASE`); imported in CI for reprepro `SignWith`.
   - Public key published at `https://apt.tailrocks.com/holla-apt/holla.gpg` (and in the repo) for users to install into `/etc/apt/keyrings`.

4. **Host on GitHub Pages** — the reprepro output tree (`dists/`, `pool/`, `holla.gpg`) is deployed via a GitHub Actions workflow (using the official `actions/deploy-pages`).
   The `gh-pages` branch is still maintained as an internal git state store for `reprepro` (to keep old package versions).
   Served at `https://apt.tailrocks.com/holla-apt/`.

### Where it lives (storage decision)
- **Store = GitHub Pages.** apt fetches the signed tree over HTTPS directly.
- **Dedicated repo** `tailrocks/holla-apt` (NOT the holla source repo) so the `.deb` binaries don't bloat the code git history; the signed tree lives on its `gh-pages` branch.
- **GitHub Releases** is used as the blob store for the raw `.deb` assets (attached by `release-deb.yml`); the `holla-apt` publish downloads from the apt-repo's own release for security (default GITHUB_TOKEN works).
- **Cross-repo upload pattern**: `holla`'s `release-deb.yml` (on tag) builds the debs (using jackin-style mise + zigbuild for latest Debian glibc only), attaches to holla release, then uploads the debs to `holla-apt`'s releases (using `GH_HOLLA_APT_TOKEN`) and triggers its `publish.yml`.
- **Keep it lean**: the holla binary is small; a few versions fit comfortably.

## CI (GitHub Actions, on tag `v*` or manual dispatch)

**Policy:** You should always use GitHub Actions for GitHub Pages deployments (never "Deploy from a branch"). Use `actions/configure-pages`, `actions/upload-pages-artifact`, and `actions/deploy-pages`. The `gh-pages` branch (if used) is only for internal state (e.g. reprepro history), never as the Pages source.

`holla/.github/workflows/release-deb.yml` (separate from the main multi-platform tarball + Homebrew release.yml):
1. Uses `jdx/mise-action` (jackin pin) + sccache + mold + `cargo zigbuild` (for arm64) — no old glibc .2.17 shims (latest Debian only).
2. `cargo install cargo-deb --locked` (via mise exec).
3. `cargo deb --target ... --no-build --deb-version "$VERSION"` (version pinned to tag).
4. Stage .deb + .sha256, upload artifacts.
5. In `attach-and-deliver` job: attach debs to the (holla) source release; if `GH_HOLLA_APT_TOKEN` present, upload debs to `holla-apt` releases (same tag) and `gh workflow run publish.yml --repo tailrocks/holla-apt -f version=...`.
6. Fallback: echo manual steps if no token.

`holla-apt/.github/workflows/publish.yml` (triggered by dispatch or repo_dispatch):
- Downloads the .deb(s) from *this* (`holla-apt`) repo's release (using default token — the cross-upload from holla makes this possible).
- Imports GPG from secrets.
- Checks out `gh-pages` state, injects keyid into `conf/distributions`.
- `reprepro -b public includedeb stable ...`
- Uploads the `public/` tree as a Pages artifact and deploys it with `actions/deploy-pages`.
  The gh-pages branch is still updated (for reprepro state only).

Each new tag on holla → new .deb(s) → uploaded cross-repo → published signed apt tree → `apt upgrade` picks it up.

## User install (modern `signed-by` keyring — not deprecated `apt-key`)

```bash
sudo install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://apt.tailrocks.com/holla-apt/holla.gpg \
  | sudo tee /etc/apt/keyrings/holla.gpg > /dev/null
echo "deb [signed-by=/etc/apt/keyrings/holla.gpg] https://apt.tailrocks.com/holla-apt stable main" \
  | sudo tee /etc/apt/sources.list.d/holla.list
sudo apt update
sudo apt install holla
```

`signed-by=` scopes the key to this repo only (current security best practice).

Direct fallback (if the apt repo is not yet published or for air-gapped):
```bash
dpkg -i holla_*.deb   # from a holla GitHub Release
```

## Notes / decisions
- **cargo-deb**: fits the Rust project (config in `Cargo.toml`); `maintainer-scripts=debian/`.
- **Dedicated apt repo + cross-upload**: allows the publish job to use only its own default `GITHUB_TOKEN` (no need to read the potentially private holla repo). Same pattern as velnor/velnor-apt.
- **Latest Debian only**: builds target modern glibc (no .2.17 compat in the deb flow; that is only for the portable tarballs used by Homebrew). Uses debian:stable-slim intent in build (via runner + target choice).
- **Multi-arch**: amd64 (native) + arm64 (via zigbuild) to match the matrix.
- **No auto-merge / Renovate etc.**: follow the org standards (root renovate.json, GH_RENOVATE_TOKEN, manual merges only).
- **Apache 2.0**: only on repo-level files (LICENSE, READMEs, manifests like Cargo.toml / distributions); no source headers.

## Status
See holla's `release-deb.yml`, `Cargo.toml` (`[package.metadata.deb]`), `debian/postinst`, and the dedicated `tailrocks/holla-apt` repo (README, `publish.yml`, `conf/distributions`).

The design is modeled directly on `velnor-project/velnor` + `velnor-apt` (and informed by `jackin-project/jackin` for the mise + zig + sccache CI patterns). 

For the consumer side in ChainArgos servers: see `ChainArgos/java-monorepo/ansible-configs/install-base.yml` (holla apt tasks in the common base, using the apt.tailrocks.com host and velnor-style idempotent tasks).