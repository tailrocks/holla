# Linux port readiness

Status: design complete; first runtime slice targets Debian 12 on `linux/amd64`.
Plans 005, 006, and 008 were complete before this audit.

## Decisions

### 1. Providers

Linux keeps portable providers and hides only invalid actions. Homebrew packages,
Homebrew services, mise, and shell-neutral project tasks remain useful on Linux;
Homebrew cask upgrades are macOS-only. Native apt/dnf and systemd-user providers
are follow-ups, not aliases for the Homebrew paths.

| Provider | Linux verdict | Follow-up |
|---|---|---|
| Find | works as-is | None |
| Disk | works with current no-op thread setup | Mount-type exclusions |
| Current folder | works as-is | None |
| Node scripts | works as-is | None |
| Just / Make / Taskfile | works as-is | None |
| Cargo | works as-is | None |
| Git hygiene / repository Git | works as-is | None |
| System | mixed: brew packages, mise, Amp, and Oh My Zsh work; brew casks hidden | apt/dnf provider |
| Brew services | works when Linuxbrew is installed; Homebrew delegates to systemd | Native systemd-user provider |
| Docker | works as-is | Linux cache insight paths |
| Gradle | works as-is | None |
| Insights | portable rows work; macOS paths hidden | XDG/Linux rows |
| User actions | works as-is | None |

### 2. Disk accounting and mounts

No Linux scan-thread initialization is needed. Linux `st_blocks` reports allocated
512-byte units, so the existing `metadata.blocks() * 512` calculation is correct
for ext4, XFS, and the ordinary Btrfs inode view
([inode(7)](https://man7.org/linux/man-pages/man7/inode.7.html)). It measures
allocated blocks, not logical length.

Btrfs compression and shared reflink/snapshot extents mean per-inode block totals
are not exclusive reclaimable bytes. Holla must describe them as allocated
estimates, as it does for APFS clones; a future exact mode would require extent
inspection. Btrfs explicitly permits snapshots to share extents and compression is
per extent ([subvolumes](https://btrfs.readthedocs.io/en/latest/Subvolumes.html),
[filesystem format](https://btrfs.readthedocs.io/en/stable/btrfs-man5.html)).

Root scans must not guess paths. The follow-up reads `/proc/self/mountinfo`, keys
mounts by major/minor device, and excludes pseudo and configured remote filesystem
types. Linux documents mountinfo's device and filesystem fields in
[`proc`](https://docs.kernel.org/filesystems/proc.html); `/proc` itself is a
pseudo-filesystem ([procfs(5)](https://man7.org/linux/man-pages/man5/procfs.5.html)).
Until then Linux adds no default skip paths, avoiding macOS firmlink and iCloud
rules that were previously wrong there.

### 3. Trash

Keep `trash` 5.2.6. Its Linux backend implements FreeDesktop Trash without sudo
([crate docs](https://docs.rs/trash/latest/trash/)). Holla remains an unprivileged
interactive tool and never silently substitutes permanent deletion.

The home trash is `$XDG_DATA_HOME/Trash`. For another mounted filesystem the spec
uses the mount's `.Trash/$uid` or `.Trash-$uid`; if a safe trash directory cannot
be used, deletion must fail or use a conforming implementation fallback, never
erase directly
([FreeDesktop Trash specification](https://specifications.freedesktop.org/trash/latest/)).
UI copy remains “Move to Trash”; backend errors stay visible per item. The crate's
mount lookup is also why Holla must not add its own competing mount parser to the
cleanup path.

### 4. Open and reveal

Opening uses `xdg-open <absolute-path>` on Linux. There is no portable reveal and
select operation, so reveal opens the absolute parent directory. `xdg-open` is a
desktop-session tool and should not run as root
([xdg-open(1)](https://manpages.debian.org/unstable/xdg-utils/xdg-open.1.en.html)).
Headless failures are reported; they are not treated as success.

### 5. Process detection

The implemented seam is `pgrep -x`, then `ps -axo ucomm=`, not `sysinfo` as the
original plan assumed. It works on Linux when procps and `/proc` are present.
Linux limits the kernel process name used by exact matching, so the `ps` basename
fallback remains necessary ([pgrep(1)](https://man7.org/linux/man-pages/man1/pgrep.1.html),
[ps(1)](https://man7.org/linux/man-pages/man1/ps.1.html)). A later structural
cleanup may replace both subprocesses with current `sysinfo`, but that is not
required for the first slice.

### 6. Packaging and terminal baseline

Debian packages remain `amd64` and `arm64`; CI and release jobs use fixed runner
labels, never `*-latest`. Holla inherits TermRock's Linux/macOS, UTF-8, modern-VT,
truecolor baseline. There is intentionally no reduced-color or `NO_COLOR` mode.
OSC 8/22/52 capabilities remain optional
([TermRock README at the pinned revision](https://github.com/tailrocks/termrock/blob/e46458ac9e8145dbc5fb89f9f27d29ced8816b0c/README.md)).

## Module audit

Verdicts describe the first Linux slice, not future feature parity.

| Module | Verdict |
|---|---|
| `cleanup/mod.rs` | Portable Unix core; macOS native-trash branch cfg-gated; FreeDesktop backend on Linux |
| `commands/cleanup_paths.rs` | Portable |
| `commands/docker.rs` | Portable Docker CLI argv |
| `commands/git.rs` | Portable Git CLI argv |
| `commands/gradle.rs` | Portable |
| `commands/idea.rs` | Portable filesystem cleanup |
| `commands/mise.rs` | Portable |
| `commands/mod.rs` | Portable dispatch |
| `commands/upgrade.rs` | Mixed; brew casks cfg-hidden on Linux |
| `config.rs` | Portable XDG-style config via `dirs` |
| `du/cache.rs` | Portable |
| `du/hardlinks.rs` | Portable Unix inode/device accounting |
| `du/mod.rs` | Portable engine |
| `du/platform.rs` | Explicit seam: macOS policies; Linux no-op and no guessed skips |
| `du/spotlight.rs` | macOS-only discovery; honest `Unavailable` on Linux |
| `du/tree.rs` | Portable |
| `du/walker.rs` | Portable Unix block accounting; mount filtering follow-up |
| `find/mod.rs` | Portable FFF integration |
| `frecency.rs` | Portable |
| `insights/artifacts.rs` | Portable artifact traversal |
| `insights/mod.rs` | Platform-tagged registry; macOS rows absent on Linux |
| `insights/process.rs` | Cross-Unix via procps; dependency documented above |
| `insights/sizing.rs` | Portable Unix block accounting |
| `main.rs` | Portable launcher and headless CLI |
| `model.rs` | Platform-neutral public model |
| `probe.rs` | Portable command/path probes |
| `providers/brew_services.rs` | Cross-platform Homebrew; systemd-native work separate |
| `providers/cargo.rs` | Portable |
| `providers/current_folder.rs` | Portable |
| `providers/disk.rs` | Portable entry point |
| `providers/docker.rs` | Portable |
| `providers/find.rs` | Portable |
| `providers/git_hygiene.rs` | Portable |
| `providers/gradle.rs` | Portable |
| `providers/insights.rs` | Portable wrapper; registry owns platform filtering |
| `providers/just.rs` | Portable |
| `providers/make.rs` | Portable |
| `providers/mod.rs` | Portable registry; action-level cfg seams |
| `providers/node_scripts.rs` | Portable |
| `providers/repos.rs` | Portable |
| `providers/system.rs` | Mixed; cask action cfg-hidden |
| `providers/taskfile.rs` | Portable |
| `providers/user.rs` | Portable argv-only actions |
| `search.rs` | Portable FFF search |
| `tui/analyzer.rs` | Portable TermRock rendering; Linux disk caveats above |
| `tui/app.rs` | Portable TermRock event/render loop |
| `tui/cleanup_flow.rs` | Portable; cleanup backend selects platform implementation |
| `tui/finder.rs` | Variant: macOS `open`; Linux `xdg-open`, parent reveal fallback |
| `tui/insights.rs` | Portable rendering |
| `tui/menu.rs` | Portable rendering |
| `tui/mod.rs` | Portable runner and terminal lifecycle |
| `tui/trust.rs` | Portable trust flow |

## Linux runtime proof

Environment: Docker official Rust `bookworm`, `linux/amd64`, pinned manifest
`sha256:b5a086f64ffecaa4e283063184770107915756739598173e1f5712d6b34b84d0`;
Rust 1.97.1 installed from the repository's fixed toolchain file. This digest was
resolved from the newest official `rust:bookworm` image on 2026-07-17.

| Check | Result |
|---|---|
| Build, clippy, tests | PASS — clippy `-D warnings`; 246 unit tests + 8 CLI tests passed, 2 manual tests ignored; debug build passed |
| Launcher first paint in a PTY | PASS — immediate scan frame, populated frame, and alternate-screen/mouse/paste restoration observed with `docker exec -it` |
| Git, current-folder, and Cargo providers | PASS — fixture registry exposed `git.{pull,push,status}` and all four Cargo actions |
| Task runner streaming | PASS — `git.status` and `cargo.build` streamed plain output and returned success |
| `/tmp` allocated-block fixture vs `du -sk` | PASS — Holla 1,142,784 B; `du -sk` 1,116 KiB = 1,142,784 B; 16 MiB sparse file reported 0 B |
| XDG Trash round-trip | PASS — with `XDG_DATA_HOME=/tmp/xdg-data`, source moved to `Trash/files/trash-me` and `Trash/info/trash-me.trashinfo` |
| macOS-only groups and paths absent | PASS — no brew-cask action or macOS-only insight IDs; portable Cargo/project insights remained |

## Sequenced follow-ups

1. **Linux mount policy (M):** parse `/proc/self/mountinfo`; skip proc, sysfs,
   devtmpfs, cgroup, and configured remote types by filesystem type/device.
2. **XDG insights (M):** add explicit Linux rows under `$XDG_CACHE_HOME` and
   Linux Docker storage; never reinterpret macOS rows.
3. **Native package providers (M):** detect apt and dnf independently, use argv
   execution, and model privilege requirements before offering mutation.
4. **systemd user services (M):** enumerate and operate `systemctl --user` units;
   keep this distinct from Homebrew-managed services.
5. **Btrfs accounting copy and optional extent mode (S design, L exact mode):**
   label shared/compressed totals and investigate FIEMAP/extent ownership.
6. **Process probe consolidation (S):** evaluate current `sysinfo` against procps
   exact-name behavior and startup cost, then remove subprocess dependency only if
   semantics remain exact.
7. **Linux terminal CI smoke (M):** automate PTY first-paint/restore and XDG Trash
   fixtures on the fixed Ubuntu runner.

Each future provider or insight must add or update its provider and module rows in
this document.
