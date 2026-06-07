# holla

Adaptive dev environment CLI. Run `holla` in any directory and get an interactive menu showing exactly what you can do — based on what tools are installed on your machine.

No config. No setup. It just looks at what you have and shows you what's possible.

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

```bash
brew tap tailrocks/holla
brew install holla
```

Or from source:

```bash
cargo install --git https://github.com/tailrocks/holla
```

## Usage

```bash
holla
```

That's it.
