# Foreman

A native macOS cockpit (Tauri) for the four-agent Claude Code pipeline:
**Planner → Coder → Tester → Reviewer**, each a specialist subagent that hands work
off through `.pipeline/*.md` files. The goal is better output from Claude Code by
keeping each role in a clean, narrow context instead of asking one agent to do everything.

Foreman **drives the Claude Code CLI headless** (`claude -p "/ship ..." --output-format stream-json`).
It does not call the API directly, so it uses your existing Claude Code login and the
per-agent models defined in the agent files (Opus for Planner/Reviewer, Sonnet for Coder/Tester).

## What v1 does

1. **Folder management** — installs the pipeline into any repo: `.claude/agents/{planner,coder,tester,reviewer}.md`, `.claude/commands/ship.md`, and a `.pipeline/` handoff folder. Won't overwrite your edits unless you tick *overwrite*.
2. **Handoff files** — lists, renders (markdown), and cleans the `.pipeline/` files each agent produces.
3. **Pipeline** — spawns the headless run, streams the log live, lights up each stage as its handoff file lands, parses the final `VERDICT:` from `review.md`, and fires a native notification when done.

## Prerequisites

- macOS with Xcode Command Line Tools
- [Rust](https://www.rust-lang.org/tools/install) (`rustup`) and Node 18+
- The `claude` CLI on your PATH, already logged in (`claude` once interactively)

## Run it

```bash
npm install
npm run tauri dev     # launches the app; first Rust build takes a few minutes
```

To produce a distributable `.app`:

```bash
npm run tauri build
```

## Using it

1. **Choose repo** — pick the project you want features shipped into.
2. **Install agents into repo** — writes the pipeline scaffold (idempotent).
3. Type a feature request, pick a permission mode, click **Ship**.
4. Watch the stages light up; read each handoff file as it appears.
5. Read the verdict in `review.md`. **Foreman never merges** — it leaves the branch/diff for you.

### Permission modes

Headless runs are unattended, so you choose how much Claude Code may do without asking:

- `acceptEdits` *(default)* — auto-approves file edits, still gated on riskier actions.
- `default` — prompts; not suitable for a truly hands-off run.
- `plan` — no writes; useful to dry-run the Planner.
- `bypassPermissions` — fully unattended. **Only run this on a throwaway branch or a git worktree** so a bad run can't touch your main tree.

## Project layout

```
src/                      frontend (vanilla TS + marked for markdown)
  main.ts                 UI logic + Tauri event wiring
src-tauri/
  src/lib.rs              commands: init/status/read/clean + streaming runner
  templates/*.md          the agent + /ship definitions installed into target repos
  capabilities/           window permissions (dialog, notification, opener)
```

## Roadmap (next iterations)

- Menu-bar / tray presence + run from the tray
- Auto-create a branch or git worktree per feature
- Re-run a single stage; edit a handoff file and resume
- History of past runs with their verdicts
