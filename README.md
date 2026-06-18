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
./scripts/install.sh          # builds + installs Foreman.app to /Applications
```

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

**Optional** — a self-updating Desktop launcher that rebuilds Foreman whenever you've edited its source, then opens it:

```sh
./scripts/install-launcher.sh
```

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
