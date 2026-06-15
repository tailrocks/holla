# holla

Adaptive dev environment CLI. Run `holla` in any directory and get an interactive menu showing exactly what you can do — based on what tools are installed on your machine.

No config. No setup. It just looks at what you have and shows you what's possible.

## Development stance

Holla is Rust-first. Prefer Rust for new project-owned commands, probes, release tooling, parsers, and long-lived automation; use another language only when an external ecosystem makes it the natural fit, and keep that exception local.

## How it works

When you run `holla`, it probes your system for installed tools (git, docker, brew, gradle, etc.) and builds a menu on the fly. Only the tools you actually have show up.

```
┌─────────────────────────────────────────────────────────────┐
│  holla — adaptive dev environment                           │
├──────────────┬──────────────────────────────────────────────┤
│  Groups      │   macOS                                      │
│              │                                              │
│ ▶  macOS     │  ▶ Upgrade everything                        │
│    Git       │    Upgrade brew packages                     │
│    Docker    │    Upgrade brew casks                        │
│              │    Upgrade mise tools                        │
│              │    Upgrade Amp CLI                           │
└──────────────┴──────────────────────────────────────────────┘
   ↑↓ navigate   → select   ← back   q quit
```

Select an action and watch it run with live output per task, side by side.

## What it can do

| Group | Actions |
|---|---|
| **macOS** | Upgrade brew, brew casks, mise, amp — all in parallel |
| **Git** | Pull / push / status across all repos in the current directory |
| **Docker** | Stop all containers, full clean (containers + images + volumes) |
| **Gradle** | Stop daemon, clean build directories |
| **IntelliJ IDEA** | Remove `.idea` dirs and `.iml` files |

Groups appear only when the relevant tool is detected. A machine without Docker won't see the Docker group.

## Install

### macOS (Homebrew)

```bash
brew tap tailrocks/holla
brew install holla@preview
```

### Debian / Ubuntu (apt)

The recommended way (used for servers in this environment) is via the published `holla-apt` repository.

See the full design and install instructions in [docs/debian-apt-repo.md](docs/debian-apt-repo.md) (includes the proper host `holla-apt.tailrocks.com`, matching the velnor-apt pattern used in ChainArgos, GPG setup, CI flow, and user install with `signed-by`).

Debian packages (`.deb`) for `amd64` and `arm64` are also attached to every GitHub Release and can be installed directly with `dpkg -i` as a fallback.

### From source

```bash
cargo install --git https://github.com/tailrocks/holla
```

Debian packages (`.deb`) for `amd64` and `arm64` are also attached to every GitHub Release and can be installed directly with `dpkg -i`.

## Usage

```bash
holla
```

That's it.

## License

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
