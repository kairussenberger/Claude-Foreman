# Implementation Plan — Custom Pipeline Mode

**Status:** planned (not started). Authored 2026-06-19.
**Goal:** let a user define their *own* pipeline — an ordered list of agents, each with its
own `.md` prompt and model — instead of being locked to the built-in
`planner → coder → tester → reviewer`. Agents already hand off through `.pipeline/*.md`,
so the data flow is generic; this plan removes the hardcoded "exactly these four, in this
order" assumption and adds a small authoring UI.

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for the system this builds on. Read that first.

---

## 1. Why this is tractable

The handoff *mechanism* is already pipeline-agnostic: an agent writes a file into
`.pipeline/`, a polling watcher notices the file appear (→ "done"), the next agent reads it.
Stage "running" is detected generically by parsing `subagent_type` out of the stream
(`lib.rs:772-795`). Much of the frontend already loops over agent-name strings rather than
naming the four agents inline (`status.agents`, `setStageState(agent, …)`, stage-dot
`.map`).

What is hardcoded — the work this plan targets — is **"there are exactly 4 agents in this
fixed order,"** baked into four places:

| # | Location | What's hardcoded |
|---|----------|------------------|
| 1 | `src-tauri/src/lib.rs` | `STAGES: [(&str,&str);4]` (`:52`), `AGENTS: [&str;4]` (`:59`), `assets()` install list (`:39`), and everything that reads them: `pipeline_status` (`:445`), `install_assets` (`:358`), `copy_claude_config` (`:382`), the `AGENTS.contains(&sub)` running-gate (`:781`) |
| 2 | `src-tauri/templates/ship.md` | 5 numbered steps with **per-stage STOP semantics** in prose (confirm gate, spec OPEN QUESTIONS, changes blocked, tester FAIL, review VERDICT) |
| 3 | `src/main.ts` | `STAGE_ORDER` (`:65`), agent→file map (`:67`), `HANDOFFS` (`:72`), `RERUN_PROMPTS` (`:80`) |
| 4 | Auto-fix loop (`main.ts:75`) | assumes the coder→tester→reviewer roles exist |

---

## 2. The keystone decision: a uniform handoff convention

`ship.md` today is **not a loop** — each stage has bespoke stop logic in prose
("if the spec has OPEN QUESTIONS, STOP"; "if status is FAIL, STOP"). A custom pipeline of
arbitrary stages cannot carry per-stage prose. The fix is to make every stage speak the
same machine-readable status line, exactly like `review.md` already ends with `VERDICT:`
(parsed at `lib.rs:336-352`).

**Convention:** every handoff file ends with a line:

```
STATUS: OK            # continue to the next stage
STATUS: STOP          # pause and surface to the user (questions / confirmation)
STATUS: BLOCKED       # hard stop, cannot proceed
```

The final stage may additionally (or instead) emit `VERDICT: SHIP | NEEDS WORK | BLOCK`
for the Shipper + auto-fix to consume (back-compatible with today).

Once this exists, the orchestrator becomes a generic loop: *delegate to stage N, wait for
its file, read its STATUS line, continue / stop / block.* This single change is what makes
everything downstream data-driven. **Do this first.**

---

## 3. Data model

A pipeline is declared as JSON in the repo, `.claude/foreman-pipeline.json`:

```jsonc
{
  "version": 1,
  "name": "default",
  "stages": [
    { "agent": "planner",  "file": "spec.md",         "model": "inherit", "confirm": true },
    { "agent": "coder",    "file": "changes.md",       "model": "inherit" },
    { "agent": "tester",   "file": "test-results.md",  "model": "inherit" },
    { "agent": "reviewer", "file": "review.md",         "model": "inherit", "verdict": true }
  ],
  "autofix": { "from": "coder", "until_verdict": "SHIP", "max_passes": 2 }
}
```

- `agent` — basename of `.claude/agents/<agent>.md` (the prompt file; existing in-app editor
  reads/writes it via `read_agent_file`/`write_agent_file`).
- `file` — handoff filename in `.pipeline/`.
- `model` — per-stage model (`ALLOWED_MODELS`, `lib.rs:63`); `set_agent_model` still rewrites
  the agent frontmatter.
- `confirm` — optional Stage-0-style "play back understanding first, then STOP" (today's
  planner-only gate becomes a per-stage flag).
- `verdict` — marks the stage whose file the Shipper + verdict parse read (defaults to the
  last stage).
- `autofix` — generalizes today's hardcoded coder→tester→reviewer fix loop: on a non-target
  verdict, re-run from stage `from` up to `max_passes` times.

**Default pipeline = the existing four.** If `foreman-pipeline.json` is absent, synthesize it
from the current `STAGES`/`AGENTS` consts so every existing repo behaves identically — this
is the back-compat guarantee.

---

## 4. Backend changes (`src-tauri/src/lib.rs`)

1. **Replace the consts with a loader.** Add `struct Stage { agent, file, model, confirm,
   verdict }` and `struct Pipeline { name, stages: Vec<Stage>, autofix: Option<AutoFix> }`.
   `fn load_pipeline(root) -> Pipeline` reads `.claude/foreman-pipeline.json`, falling back to
   a built-in default equal to today's four. Keep the current `STAGES`/`AGENTS` consts only as
   the source for that default.
2. **`pipeline_status` (`:445`)** — iterate `load_pipeline(root).stages` instead of the
   `AGENTS` array. `AgentInfo`/`HandoffFile` already serialize fine; the lists just become
   variable-length. (`main.ts` already renders from `status.agents` — see §6.)
3. **`install_assets` / `assets()` (`:39,:358`)** — keep installing the *embedded* default
   agents + `ship.md` + `settings.json` as a starting point, then also write
   `foreman-pipeline.json` (the default) if absent. Custom agents the user adds live as
   real `.claude/agents/*.md` files; nothing in the binary needs them.
4. **`copy_claude_config` (`:382`)** — copy the whole `.claude/agents/` dir + the
   `foreman-pipeline.json` + `commands/` rather than the fixed `assets()` list, so overnight
   worktrees inherit a custom pipeline. (Switch from per-file copy to dir copy.)
5. **Running-gate (`:781`)** — replace `AGENTS.contains(&sub)` with "is `sub` one of the
   loaded pipeline's stage agents." Pass the stage-agent set into the reader thread (clone a
   `Vec<String>` before spawn).
6. **Watcher / "done" detection** — already keys off files appearing in `.pipeline/`; point it
   at the loaded stage files instead of `STAGES`.
7. **Verdict parse (`:336`)** — read the stage flagged `verdict` (or the last stage's file)
   instead of hardcoded `review.md`. Keep `VERDICT:`-line-only matching.
8. **New commands:** `read_pipeline(project) -> Pipeline`, `write_pipeline(project, Pipeline)`,
   and `add_agent(project, name, template?)` (creates `.claude/agents/<name>.md` from a blank
   or copied template). `remove`/reorder are just `write_pipeline` with a new stage list.
   `read_handoff` path-guard (`:580`) already blocks `/` and `..` — keep it.

No change needed to `run_pipeline`'s spawn/stream core, `cancel_*`, worktree create/remove,
or the Shipper — they're already agent-agnostic.

---

## 5. The generic orchestrator (`templates/ship.md` + `ship-auto.md`)

Rewrite `ship.md` from 5 bespoke numbered steps into a **generic loop over the declared
stages** (the orchestrator reads `.claude/foreman-pipeline.json` itself):

```
Read .claude/foreman-pipeline.json. For each stage in order:
  - (if confirm) delegate the agent to write <file> with Understanding+Assumptions and
    end "STATUS: STOP"; show it to me and wait for my reply.
  - else delegate the <agent> subagent; wait for .pipeline/<file>.
  - read the final STATUS line: OK → continue; STOP → surface to me and pause;
    BLOCKED → stop and explain.
Report the VERDICT from the verdict-stage file. Never commit/merge/push.
```

`ship-auto.md` is the same loop minus the pauses (treats STOP as continue / logs it), as
today. The per-stage STOP prose moves *into each agent's own `.md`* — e.g. the tester agent
is told "if tests fail, end with STATUS: BLOCKED" — which is exactly where a custom-agent
author would want that logic anyway.

---

## 6. Frontend changes (`src/main.ts`, `styles.css`)

1. **Delete the four hardcoded arrays** (`STAGE_ORDER :65`, file map `:67`, `HANDOFFS :72`)
   and derive them at runtime from `pipeline_status` / `read_pipeline`. Store the current
   pipeline's stage list in module state and use it everywhere those consts are referenced
   (`resetStages :330`, stage dots `:785`, `:898`).
2. **`RERUN_PROMPTS` (`:80`)** — generate per-stage from a template string
   (`Re-run ONLY the <agent> stage … rewrite .pipeline/<file> …`) instead of 4 literals.
3. **Crew render** — the default-mode column already loops `status.agents` (`:153`); confirm
   it handles N≠4 without layout breakage (CSS may assume four columns — make the crew row
   wrap/flow). The mini stage-dots (`:785`) already `.map(STAGE_ORDER)`; swap the source.
4. **Confirmation gate (`awaitingConfirmation :107`, `:467`, `:595`)** — drive it off the
   stage's `confirm` flag rather than "first pause == planner." A pipeline with no `confirm`
   stage simply never shows the blue popup.
5. **Auto-fix (`:75`)** — read `autofix.from` / `max_passes` from the pipeline rather than
   assuming the coder/tester/reviewer trio.

---

## 7. Authoring UI (the only net-new surface)

A "Custom pipeline" editor reachable from the ☰ menu (alongside Default / Overnight /
History). Most plumbing already exists — the in-app agent-prompt editor uses
`read_agent_file`/`write_agent_file`; reuse it.

- A reorderable list of stage rows: **agent name · model select · `confirm`/`verdict`
  toggles · ✎ edit prompt · 🗑 remove**, plus **+ Add agent** (calls `add_agent`, opens the
  editor on a blank template).
- **Save** → `write_pipeline`. Validate: ≥1 stage, unique agent names, every agent has a
  `.md`, exactly one `verdict` stage (default last).
- Saved presets (optional, later): keep a couple of named `foreman-pipeline.*.json` and a
  picker. MVP can ship with just "edit the one pipeline per repo."

---

## 8. What does NOT generalize for free (scoped cuts)

- **Confirmation gate** → becomes a per-stage `confirm` flag (was planner-only-by-assumption).
- **Auto-fix** → becomes `autofix.from` config (was fixed coder→tester→reviewer).
- **Per-stage STOP prose** → moves into each agent's own `.md` via the STATUS convention.
- **Overnight / worktree mode** → already agent-agnostic; only the `copy_claude_config`
  dir-copy change (§4.4) is needed so custom agents reach the worktree. Essentially free.
- **Shipper** → unchanged; it already reads the verdict file + working tree.

---

## 9. Build order (suggested)

1. **STATUS-line convention** in the four default agents + parse it generically
   (`lib.rs` verdict/status reader). Ship this alone first — back-compatible, unlocks the loop.
2. **`load_pipeline` + default synthesis**; convert `pipeline_status`, the running-gate, the
   watcher, and the verdict parse to read it. No behavior change yet (default == today).
3. **Generic `ship.md` / `ship-auto.md`** loop; move STOP prose into agent files.
4. **Frontend de-hardcoding** (§6) — derive arrays from status; verify N≠4 layout.
5. **`read_pipeline`/`write_pipeline`/`add_agent` + authoring UI** (§7).
6. **`copy_claude_config` dir-copy** so overnight inherits custom pipelines.
7. Refresh `~/Developer/cart-sandbox`'s `.claude/` and run a custom 2-stage pipeline end to
   end (default + overnight) as the live check.

**Effort:** ~1–2 sessions. Steps 1–4 are the core (and individually shippable); step 5 is the
only new UI; step 6 is small.

---

## 10. Risks / open questions

- **CSS assumes four crew columns** — confirm the crew row and re-run buttons reflow for 2 or
  6 agents (likely a flexbox/wrap tweak in `styles.css`).
- **Orchestrator reading its own config** — the `claude` agent must reliably read
  `foreman-pipeline.json` and loop; if that proves flaky, fall back to *generating* a concrete
  numbered `ship.md` from the JSON on `write_pipeline` (Rust templates the steps), trading
  dynamism for determinism. Decide after testing step 3.
- **Loop/branching scope** — MVP is strictly linear (list of stages). Conditionals / fan-out /
  loops are explicitly out of scope; the STATUS convention leaves room to add them later.
