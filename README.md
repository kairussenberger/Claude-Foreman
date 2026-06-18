# Foreman

**A native macOS app that runs a four-agent Claude Code pipeline — a Planner, a Coder, a Tester, and a Reviewer that hand work off to each other — so you ship better code from one prompt instead of babysitting a single agent through every step.**

![The crew: Planner → Coder → Tester → Reviewer](docs/crew.png)

*Four specialists, each kept in a clean context: the **Planner** turns your request into a spec, the **Coder** implements exactly that, the **Tester** writes & runs tests, and the **Reviewer** gives a SHIP / NEEDS WORK / BLOCK verdict — then a fifth **Shipper** agent commits / pushes / opens a PR on your word.*

> Foreman drives the `claude` CLI you already use. It runs on your **Claude subscription** — no API key, no extra billing — and **never merges anything**: it leaves the diff for you.

---

## Quick start

```sh
git clone https://github.com/kairussenberger/Claude-Foreman.git
cd Claude-Foreman
./scripts/install.sh             # build + install Foreman.app
./scripts/install-launcher.sh    # ⭐ recommended — Desktop icon that auto-updates on every launch
```

> ### ⭐ Launch from the Desktop icon — and you're always up to date
> The launcher's Desktop **Foreman** icon **pulls the latest features from GitHub and rebuilds before opening, every time you click it.** So whenever new features ship, you just open Foreman and you have them — no manual updates. (Prefer not to? Skip the launcher and update by hand with `git pull && ./scripts/install.sh`.)

Then open Foreman → **Choose repo** → **Install agents into repo** → type a feature → hit **▶ Ship**. Watch the four figures light up in turn and a verdict land. (Prereqs in [Install](#2-install).)

---

## Contents

1. [What it is](#1-what-it-is)
2. [Install](#2-install)
3. [The two modes (+ the Shipper)](#3-the-two-modes--the-shipper)
4. [The agents](#4-the-agents)
5. [Features at a glance](#5-features-at-a-glance)
6. [How it works](#6-how-it-works)
7. [Repo layout](#7-repo-layout)
8. [Notes & caveats](#8-notes--caveats)
9. [Porting to Windows](#9-porting-to-windows)

---

## 1. What it is

One agent doing everything — plan, code, test, review — fills its context with four jobs at once, and quality drops. Foreman splits the work across **four specialists that each stay in a clean, narrow context** and hand off through small files in a `.pipeline/` folder:

```
your request → Planner (spec.md) → Coder (changes.md) → Tester (test-results.md) → Reviewer (review.md) → VERDICT
```

You drive it from a native Mac app: pick a repo, install the agents into it, type a feature request, and Foreman runs the whole pipeline — showing each agent working live, opening every handoff file as it lands, and ending on a **SHIP / NEEDS WORK / BLOCK** verdict. A fifth **Shipper** agent then acts on that verdict (commit, push, PR, deploy) when you tell it to.

It's a control panel over the `claude` CLI — Foreman spawns `claude -p "/ship …" --output-format stream-json` and renders the stream. The per-agent models, the pipeline logic, and the verdict all come from agent + command files it installs into your repo.

## 2. Install

### Prerequisites

- **macOS** (Apple Silicon or Intel)
- **[Claude Code](https://docs.claude.com/en/docs/claude-code)** — the `claude` CLI installed and logged in (a Pro/Max subscription works; no API key needed)
- **[Node.js](https://nodejs.org) 18+**
- **[Rust](https://rustup.rs)** (rustup) — Tauri compiles the native shell
- **Xcode Command Line Tools** — `xcode-select --install`

### Build & install

```sh
./scripts/install.sh
```

…or do it by hand:

```sh
npm install
npm run tauri build            # first build compiles Rust — a few minutes
cp -R src-tauri/target/release/bundle/macos/Foreman.app /Applications/
```

The app is an **unsigned local build**, so the first time you open it macOS may warn about an unidentified developer — right-click the app → **Open** once, and it's trusted from then on.

### ⭐ Recommended: the auto-updating launcher

This is the way to run Foreman. It installs a Desktop **Foreman** icon that keeps you on the latest version automatically — on each click it **pulls new features from GitHub, rebuilds if anything changed, then opens the app**:

```sh
./scripts/install-launcher.sh
```

Launch from that icon and you'll never update by hand — new features just show up the next time you open it. (Rather not? Update manually any time with `git pull && ./scripts/install.sh`.)

## 3. The two modes (+ the Shipper)

A hamburger menu (☰, top-left) switches between:

- **Default** — one feature at a time, in the repo you select. Interactive: if an agent genuinely needs a decision, the run pauses, notifies you, and you reply to continue. Per-agent model dropdowns, a global effort slider, live token counts, and an optional **auto-fix** loop (on a non-SHIP verdict it loops the findings back through the pipeline, up to N passes, until it SHIPs).
- **Parallelised — overnight** — queue many features and run them **concurrently, each in its own `git worktree` + branch** (capacity-capped). Each runs fully autonomously to a verdict; a runs list shows every run's stage, verdict, and tokens. Nothing is merged — you review the branches in the morning.

The **Shipper** is a separate window (its own agent, locked to Sonnet / medium effort). You prompt it in plain language — *"commit these and open a PR to main"*, *"merge the branch and tag v1.2.0"* — and it executes via `git` / `gh`. It reads the verdict and the working tree; it has no pipeline of its own. You can open it from the header or fire it straight at a specific overnight run with **⇪ Ship this**.

## 4. The agents

Each is a Markdown file Foreman installs into your repo's `.claude/agents/` — editable in-app or by hand. Models are the defaults; change any of them from the crew dropdowns.

| Agent | Default model | Does | Writes |
|-------|---------------|------|--------|
| **Planner** | Opus | Turns the request into a precise spec; never writes code | `.pipeline/spec.md` |
| **Coder** | Sonnet | Implements exactly the spec; no scope creep | code + `.pipeline/changes.md` |
| **Tester** | Sonnet | Writes & runs tests; reports, never fixes | tests + `.pipeline/test-results.md` |
| **Reviewer** | Opus | Read-only final gate: SHIP / NEEDS WORK / BLOCK | `.pipeline/review.md` |
| **Shipper** | Sonnet | Acts on the verdict — commit / push / PR / deploy | (your repo / remote) |

## 5. Features at a glance

| | |
|---|---|
| **Live crew** | Four pixel figures that bob while their agent works; the active one is driven by real delegation events |
| **Per-agent models** | Pick Opus / Sonnet / Haiku / Fable / inherit per agent |
| **Global effort** | `low → max` reasoning-effort slider, remembered per repo |
| **Token usage** | Live + per-session token totals |
| **Interactive questions** | A genuine question pauses the run, notifies you, and resumes on your reply |
| **Auto-fix** | Loop a non-SHIP verdict back through the pipeline up to N passes |
| **Overnight parallel** | Many features at once, each in its own worktree/branch, capped concurrency |
| **Shipper** | A promptable 5th agent that commits / pushes / opens PRs |
| **Run history** | Past runs + verdicts + tokens, persisted across launches |
| **Per-project config** | Profiles, a preflight **doctor** (claude / node / git / pipeline), and an in-app agent-prompt editor |
| **Re-run a stage** | ↻ on any agent to re-run just that stage |
| **Worktree cleanup** | Discard a finished overnight worktree + its branch from the UI |
| **Menu-bar tray** | Open / Quit from the macOS menu bar |
| **Native notifications** | A ping when a verdict lands or input is needed |

## 6. How it works

- Foreman spawns the `claude` CLI headless: `claude -p "/ship <request>" --output-format stream-json --verbose`, in the repo you selected (or a fresh worktree, for overnight runs).
- The `/ship` command (installed into `.claude/commands/`) runs the four agents in order; each writes its handoff file to `.pipeline/`, which the next agent reads.
- The app tracks progress by watching those handoff files and parsing the stream — an agent shows **working** when it's delegated and **done** when its file lands.
- The final verdict is parsed from `review.md`. Foreman **never commits or merges**; the Shipper only acts when you ask it to.

## 7. Repo layout

```
index.html · shipper.html      the two app windows
src/                           frontend (vanilla TS + marked)
  main.ts                      pipeline UI, modes, events
  shipper.ts                   the Shipper window
src-tauri/
  src/lib.rs                   Rust: folder mgmt, headless runner, worktrees, tray
  templates/*.md               the agents + /ship + /ship-auto + settings, installed into repos
  icons/                       app icon
scripts/
  install.sh                   build + install to /Applications
  install-launcher.sh          optional self-updating Desktop launcher
docs/crew.png                  the hero image
```

## 8. Notes & caveats

- **Subscription, not API.** Foreman uses whatever your `claude` CLI is logged into — typically a Claude subscription, billed against your plan's usage, not per-token API credits.
- **It never merges.** Every run leaves its diff (and, for overnight runs, its branch) for you to review. Only the Shipper takes outward action, and only when you tell it to.
- **Overnight runs are autonomous** and use `bypassPermissions` inside isolated worktrees, so they can run tests/builds without prompting. The **Shipper** also runs with full permissions by design — it's meant to act — so it can `git push` / open PRs the moment you ask.
- **Unsigned build.** It's a local Tauri build; there's no Apple notarization. Right-click → Open on first launch if macOS objects.

## 9. Porting to Windows

Foreman is built on Tauri, so the app shell, the **entire frontend**, the tray, notifications, dialogs, multi-window, git/worktrees, and the pipeline engine are already cross-platform — a Windows build is mostly swapping a few macOS-specific bits. Recommended path:

1. **Resolve `claude` on Windows.** The only OS-bound code is `resolve_claude` / `which_login` / `newest_nvm_claude` in `src-tauri/src/lib.rs` (~lines 178–240), which hardcodes Unix (`/bin/zsh -ilc`, `~/.nvm`, `/opt/homebrew`, `$HOME`). Add a `#[cfg(target_os = "windows")]` branch that uses `%USERPROFILE%`, `where claude`, and common npm / nvm-windows locations (e.g. `%APPDATA%\npm\claude.cmd`).
2. **Spawn the `.cmd`.** On Windows `claude` is usually `claude.cmd`, which Rust's `Command::new` can't launch directly — invoke it as `cmd /c claude …`. The rest of `run_pipeline` / `ship_agent` is unchanged; `git` works as-is.
3. **Scripts.** `scripts/*.sh` are macOS-only (zsh / AppleScript / `/Applications`). Add an `install.ps1` (`npm install` → `npm run tauri build` → run the produced `.msi`/`.exe`); the self-updating launcher is optional.
4. **Build & ship.** You can't cleanly cross-compile from macOS — build on Windows (Rust MSVC toolchain + Node + WebView2; `npm run tauri build` emits an `.msi`/`.exe`), or add a **GitHub Actions matrix** using [`tauri-apps/tauri-action`](https://github.com/tauri-apps/tauri-action) to produce macOS + Windows installers on every tag. The CI route is the cleanest if you don't have a Windows machine handy.

**Verify first:** confirm Claude Code's Windows CLI behaves the same headless — `claude -p --output-format stream-json --verbose --permission-mode <m> --effort <e> --resume <id>`, slash commands, and subagents. That parity is the one thing outside Foreman's control.

That's the whole surface — point an agent at this repo and it can take it from here.
