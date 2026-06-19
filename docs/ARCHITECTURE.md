# Foreman — Architecture & Development Notes

A deep dive into how Foreman is built, what's implemented, and what's next. For the
user-facing intro see [`README.md`](../README.md); this doc is for working on Foreman
itself (or pointing an agent at it).

---

## 1. What Foreman is

A native **macOS app (Tauri: Rust backend + vanilla-TS frontend)** that wraps a
four-agent Claude Code pipeline. One agent doing plan + code + test + review fills its
context with four roles at once and quality drops; Foreman splits the work across four
specialists that each stay in a clean, narrow context and hand off through small files:

```
your request → Planner (spec.md) → Coder (changes.md) → Tester (test-results.md) → Reviewer (review.md) → VERDICT
```

A fifth **Shipper** agent then acts on the verdict (commit / push / PR / deploy) on your word.

It is a **control panel over the `claude` CLI** — it does not call the Anthropic API
directly and has no API key. It spawns `claude -p "/ship …" --output-format stream-json`
and renders the stream. Per-agent models, the pipeline logic, and the verdict all come
from agent + command files Foreman installs into the target repo's `.claude/`.

**Runs on your Claude subscription. Never commits or merges** — it leaves the diff (and,
for overnight runs, the branch) for you. Only the Shipper takes outward action, and only
when asked.

---

## 2. How it works (the engine)

- Foreman spawns: `claude -p "<prompt>" --output-format stream-json --verbose --permission-mode <mode> [--effort <e>] [--resume <session>]`, with `current_dir` = the selected repo (or a fresh git worktree, for overnight runs).
- `<prompt>` is `/ship <request>` (interactive) or `/ship-auto <request>` (autonomous), unless resuming — on resume the prompt is the user's reply, sent verbatim into the same session.
- The `/ship` command (installed at `.claude/commands/ship.md`) delegates to the four subagents in order via the Task tool. Each subagent writes its handoff file to `.pipeline/`; the next reads it.
- **Progress tracking is dual-signal:**
  - A stage shows **"working"** when the orchestrator actually *delegates* to it — detected by parsing `assistant` `tool_use` blocks with `subagent_type` in the stream.
  - A stage shows **"done"** when its handoff *file* appears — detected by a polling watcher thread on `.pipeline/`.
- The final **verdict** is parsed from the `VERDICT:` line in `review.md` (only that line — scanning prose false-matches words like "blocking").
- **Token usage** comes from the stream: the authoritative total is the final `result` event's `usage` + `total_cost_usd` (cost is not displayed — runs are subscription-covered).
- **Session continuity:** the `session_id` is captured from the stream so a paused run can be **resumed** with `--resume` (this powers interactive questions, the confirmation gate, auto-fix, and per-stage re-runs).

### Rust commands (`src-tauri/src/lib.rs`)

| Command | Purpose |
|---|---|
| `init_pipeline(project, force)` | Install the `.claude/` scaffold (agents + commands + settings) into a repo |
| `pipeline_status(project)` | Which agents/command present + each agent's current model + handoff file status |
| `set_agent_model(project, agent, model)` | Rewrite the `model:` line in an agent's frontmatter |
| `read_agent_file` / `write_agent_file` | Read/write an agent's full `.md` for the in-app prompt editor |
| `doctor(project)` | Preflight: Claude CLI, Node.js, git repo, pipeline-installed |
| `read_handoff(project, name)` | Read a `.pipeline/<name>` file (path-guarded) |
| `clean_pipeline(project)` | Delete handoff files for a fresh run |
| `create_worktree(repo, slug)` | `git worktree add` + copy the repo's `.claude` config in (preserves model picks) |
| `remove_worktree(repo, path, branch)` | Remove a worktree + safe-delete its (empty) branch |
| `run_pipeline(run_id, project, request, permission_mode, effort, autonomous, resume, clean_first, fast)` | The core runner — spawn `claude`, stream events keyed by `run_id`. `fast: Some(true)` selects the `/ship-fast` orchestrator |
| `cancel_run(run_id)` / `cancel_all()` | Kill one / all in-flight runs |
| `ship_agent(project, prompt, resume)` | The Shipper — `claude -p` locked to sonnet/medium, bypassPermissions, session-resumable |
| `list_skills` / `read_skill` / `write_skill` / `create_skill` / `delete_skill` | Manage native Claude Code skills under `.claude/skills/<name>/SKILL.md` (path-guarded) |
| `build_skill(project, prompt)` | The Skill Builder — a `claude -p` agent (sonnet/medium, bypassPermissions, scoped to `.claude/skills/`) that authors a whole skill folder (SKILL.md + any scripts/references) from a prompt. Keyed `"skill-builder"` |
| `read_custom_pipeline` / `write_custom_pipeline(project, agents)` | Read / persist the ordered custom pipeline (`.claude/foreman-pipeline.json`); writing also regenerates the `/ship-custom` orchestrator |
| `build_agent(project, name, prompt)` | The Agent Builder — a `claude -p` agent (sonnet/medium, bypassPermissions) that authors one `.claude/agents/<name>.md` from a description. Keyed `"agent-builder"` |

In-flight children live in a `RunState { children: Mutex<HashMap<String, Child>> }`, keyed by `run_id` (`"default"` for default mode, a unique slug per overnight run, `"shipper"` for the Shipper).

### Events (Rust → frontend), all run-tagged

- `pipeline-log` `{ run_id, kind, text, raw }`
- `pipeline-stage` `{ run_id, agent, file, phase: "running"|"done" }`
- `pipeline-usage` `{ run_id, input_tokens, output_tokens, cache_read, cache_creation, is_final }`
- `pipeline-done` `{ run_id, code, verdict }`
- `pipeline-session` `{ run_id, session_id }`
- `shipper-log` `{ kind, text }`, `shipper-session` (string), `shipper-done` (∅)
- `shipper-retarget` `{ repo }` — emitted by the main window to re-point an open Shipper

---

## 3. File structure

```
Claude-Foreman/
├─ index.html               main cockpit window
├─ shipper.html             Shipper window
├─ vite.config.ts           multi-page build (index + shipper entries)
├─ package.json · tsconfig.json
├─ README.md
├─ docs/
│  ├─ crew.svg / crew.png   the four-agent hero image
│  └─ ARCHITECTURE.md       this file
├─ scripts/
│  ├─ install.sh            build + install Foreman.app to /Applications
│  ├─ install-launcher.sh   create the auto-updating Desktop launcher (generates its AppleScript)
│  └─ launch.sh             run by the launcher: git pull → rebuild-if-changed → open
└─ src/
│  ├─ main.ts               main window — pipeline UI, the two modes, history, config, all event wiring
│  ├─ shipper.ts            Shipper window logic
│  └─ styles.css            shared styling for both windows
└─ src-tauri/
   ├─ src/lib.rs            Rust: commands, headless runner, worktrees, tray, claude resolution
   ├─ src/main.rs           entry → lib::run()
   ├─ templates/            the .claude assets installed into target repos:
   │  ├─ planner.md  coder.md  tester.md  reviewer.md     the core (default) pipeline agents
   │  ├─ fast-coder.md  fast-reviewer.md                   the fast pipeline's own independent agents
   │  ├─ ship.md            interactive orchestrator (with the Stage-0 confirmation gate)
   │  ├─ ship-auto.md       autonomous orchestrator (overnight — never pauses)
   │  ├─ ship-fast.md       fast-path orchestrator (fast-coder → fast-reviewer; no planner/tester/gate)
   │  └─ settings.json      permissions.allow list pre-approving test/build commands
   ├─ capabilities/default.json   window permissions (covers "main" + "shipper")
   ├─ tauri.conf.json · Cargo.toml (tray-icon feature)
   └─ icons/                app icon (the blue Planner figure)
```

The `templates/*` files are embedded in the binary via `include_str!` and written into a
target repo's `.claude/` on **Install agents into repo**. Overnight worktrees get the
repo's *own* `.claude/agents` copied in (so per-agent model picks carry over), falling
back to the embedded defaults.

---

## 4. The modes + the Shipper

Switched via the hamburger (☰, top-left); choice persists.

- **Default** — one feature at a time, interactive. Uses `/ship`. Features: the crew with per-agent **model dropdowns** + **↻ re-run** buttons, a global **effort slider**, live **token** counts, the **confirmation gate** (the Planner always plays back its understanding + assumptions first — distinct blue popup), **interactive questions** (a genuine pause shows the amber reply box and resumes on your reply), and an optional **auto-fix** loop (on a non-SHIP verdict, resume up to N passes until SHIP).
- **Fast** — the same interactive pane as Default, but runs two **independent** agents — `fast-coder` → `fast-reviewer` — via `/ship-fast`: no Planner, no Tester, **no confirmation gate**. These are their own `.claude/agents/fast-*.md` files (deliberately *not* the default coder/reviewer): the fast-coder implements straight from the request — no spec — and writes `changes.md`; the fast-reviewer judges against the request + `git diff` (no `test-results.md`) and writes `review.md` with the VERDICT. Same verdict/tokens/auto-fix/Shipper machinery as Default. Implemented as a `fastMode` flag over the Default pane that swaps in the fast crew cards (`#mode-default.fast` shows the `.fast-only` cards and hides the four default ones) and passes `fast: true` to `run_pipeline`. Agent membership: `ALL_AGENTS` (6) is what Foreman installs / lets you edit / set a model on / delegate to; `AGENTS` (the core 4) still gates default-mode `initialized`.
- **Custom** — a user-defined pipeline of arbitrary agents (reuses the Default pane, like Fast). The left panel becomes a stage list (per-stage model, ↑/↓ reorder, ✎ edit, 🗑 remove) with **＋ Add agent**: write the instruction `.md` by hand, or describe it and have `build_agent` author it. Agents are real `.claude/agents/<slug>.md`; the order persists to `.claude/foreman-pipeline.json`; Rust regenerates a concrete `/ship-custom` orchestrator on every change (so the agent never parses JSON). Each stage hands off via `.pipeline/<slug>.md`, may read all earlier handoff files, and the **last** stage is told to emit `VERDICT: SHIP | NEEDS WORK | BLOCK`. Runs are full-interactive (questions + verdict + auto-fix), no planner gate. Backend generalizations that made this possible: `run_pipeline` computes its watched stages + verdict file **per run** (custom = one `.pipeline/<slug>.md` per stage, last carries the verdict), the "stage running" detection accepts any delegated subagent name, and agent read/write/model commands validate a safe slug rather than a fixed membership list. Only mandatory contract: stages hand off via files and the last renders a verdict — names/roles are fully variable.
- **Skills** — manage this repo's native **Claude Code skills** (`.claude/skills/<name>/SKILL.md`). A left list (name + description, click to edit, two-click 🗑 to delete) + an inline `SKILL.md` editor, with **＋ New** (slugged folder from a template). **Lint-on-Save** soft-warns (amber status line) when a saved skill won't trigger — placeholder/empty/missing `description`, broken frontmatter, or `name` ≠ folder. A **Skill Builder** panel (`build_skill`) lets you describe a skill and have an agent author the whole folder — SKILL.md **plus** any `scripts/`/`references/`/`assets/` it needs, with correct cross-references — straight into `.claude/skills/`, then it appears in the list to edit. The `claude` CLI auto-discovers these; agents and the orchestrator invoke them via the Skill tool. (The four built-in pipeline agents are left as-is for now — see roadmap; this is the library + authoring surface.)
- **Parallelised — overnight** — queue many features; each runs in its own `git worktree` + branch, capped by an adjustable **concurrency** (default 2). Uses `/ship-auto` + `bypassPermissions` (autonomous, never pauses). A **runs list** shows each run's stage dots, verdict, tokens; finished runs get **⇪ Ship this** and **🗑 Discard** buttons.
- **History** — past runs (both modes) with verdict + tokens + branch, persisted in `localStorage`, surviving relaunch.

The **Shipper** is a separate window (`shipper.html`), its own agent locked to **sonnet / medium effort**, `bypassPermissions`. You prompt it (*"commit & open a PR to main"*); it reads `review.md` + the working tree and acts via `git`/`gh`. It keeps session context (`--resume`) and can be retargeted at a specific overnight worktree via **⇪ Ship this**.

---

## 5. Distribution & auto-update

- **Install:** `git clone` → `./scripts/install.sh` (build + copy `Foreman.app` to `/Applications`).
- **Recommended: the auto-updating launcher** (`./scripts/install-launcher.sh`) — a Desktop icon whose `launch.sh` does `git pull --ff-only` → rebuild-if-changed → open. So a `git push` of a new feature reaches users on their next launch. Requires the build toolchain + a clean working tree (pull is best-effort, skipped otherwise).
- Published at **github.com/kairussenberger/Claude-Foreman** (public). Pushed over **HTTPS** via `gh` as the credential helper (SSH push was rejected on this machine).

---

## 6. Roadmap — what can still be implemented

### Recently shipped
- **Fast mode** — Coder → Reviewer via `/ship-fast`, a `fastMode` flag over the Default pane (see §4). Done.
- **Skills space** — in-app authoring of native `.claude/skills` (see §4). Done. Deferred follow-up: give the four built-in agents the `Skill` tool and extract their expertise into skills, so the pipeline itself runs on skills (the user chose "library now, extract later").

### Custom pipeline mode — MVP shipped (branch `feature/custom-pipeline`)
A user-defined ordered list of arbitrary agents that hand off via files, with the last
rendering a verdict — see §4 **Custom**. Built from the lean spec in
[`CUSTOM_PIPELINE_PLAN.md`](CUSTOM_PIPELINE_PLAN.md). Deferred polish: fancier crew figures,
per-stage verdict designation (currently always the last stage), overnight/worktree runs for
custom pipelines, and saved/named pipelines.

### Other candidates
- **In-app auto-updater** (Tauri updater + GitHub Releases) — true "download an app, it updates itself" for non-developers; needs a signing key, a CI release pipeline, and per-release version bumps. (Today's auto-update is the clone+rebuild launcher.)
- **Windows port** — see [README §9](../README.md#9-porting-to-windows). Localized to `resolve_claude`/`which_login` (`#[cfg(windows)]`), the `.cmd` spawn, Windows install scripts, and a GitHub Actions matrix build.
- **Shipper remote-action guardrail** — have the Shipper echo the exact `git push`/PR command and require an explicit "yes" before anything that touches a remote (it currently acts immediately under `bypassPermissions`).
- **In-app diff viewer** — syntax-highlighted `git diff` per run, instead of dropping to a terminal.
- **Code-sign + notarize** the `.app` (no Gatekeeper warning; shareable) and a **Linux** build target.
- **LICENSE** file (none yet).

---

## 7. Gotchas & notes for contributors

- **`claude` resolution** (`resolve_claude`): GUI/launchd launches get a minimal PATH that omits nvm, so the app scans known locations (incl. newest `~/.nvm/.../bin/claude`) then falls back to an *interactive* login shell (`zsh -ilc`, which sources `~/.zshrc`). Plain `-lc` is **not** enough.
- **settings.json allow-list**: pre-approves test/build commands (`npm test`, `node --test`, `pytest`, `cargo test`, …) so the Tester can run tests under `acceptEdits` without a spurious "approve npm test" pause.
- **Stage timing**: "working" is driven by the *delegation* event, not the previous file appearing (which fired too early and jumped ahead on resume).
- **Unique branches**: each overnight run's id is `<slug>-<sessionStamp><counter>`, so re-running the same feature never collides with or clobbers a prior worktree.
- **Multi-window**: Vite is multi-page (`index.html` + `shipper.html` in `rollupOptions.input`); the capability lists `windows: ["main","shipper"]` and grants `core:event:allow-emit`, `core:webview:allow-create-webview-window`, `core:window:allow-set-focus`.
- **The pipeline never commits.** A finished run leaves uncommitted changes in the working tree / worktree; the verdict comes from `review.md`, not git state.
