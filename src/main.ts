import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { marked } from "marked";

// ---- Types mirroring the Rust payloads ----
type FileResult = { path: string; action: string };
type InitResult = { project: string; files: FileResult[] };
type HandoffFile = { name: string; exists: boolean; size: number; modified_ms: number | null };
type AgentInfo = { name: string; present: boolean; model: string | null };
type PipelineStatus = {
  initialized: boolean;
  agents: AgentInfo[];
  has_ship_command: boolean;
  handoffs: HandoffFile[];
};
type WorktreeInfo = { path: string; branch: string };
type LogEvent = { run_id: string; kind: string; text: string; raw: string };
type StageEvent = { run_id: string; agent: string; file: string };
type DoneEvent = { run_id: string; code: number | null; verdict: string | null };
type UsageEvent = {
  run_id: string;
  input_tokens: number;
  output_tokens: number;
  cache_read: number;
  cache_creation: number;
  is_final: boolean;
};

type StageState = "idle" | "running" | "done" | "blocked";
type Run = {
  id: string;
  request: string;
  worktree: string | null;
  branch: string | null;
  stages: Record<string, StageState>;
  verdict: string | null;
  status: "queued" | "working" | "done" | "stopped";
  tokens: number;
  tokensFinal: boolean;
  log: { kind: string; text: string }[];
  el?: HTMLElement;
};

const STAGE_ORDER = ["planner", "coder", "tester", "reviewer"];
const HANDOFF_FOR: Record<string, string> = {
  planner: "spec.md",
  coder: "changes.md",
  tester: "test-results.md",
  reviewer: "review.md",
};
const HANDOFFS = ["spec.md", "changes.md", "test-results.md", "review.md"];
const EFFORT_LEVELS = ["low", "medium", "high", "xhigh", "max"];
const DEFAULT_RUN = "default";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
function escapeHtml(s: string): string {
  return s.replace(/[&<>"]/g, (c) =>
    c === "&" ? "&amp;" : c === "<" ? "&lt;" : c === ">" ? "&gt;" : "&quot;",
  );
}
function fmtTokens(n: number): string {
  return n.toLocaleString("en-US");
}

let project: string | null = localStorage.getItem("foreman.project");
let running = false; // default-mode single run
let activeFile: string | null = null;
let sessionTokens = Number(localStorage.getItem("foreman.sessionTokens") || "0");
let appMode: "default" | "parallel" = localStorage.getItem("foreman.mode") === "parallel" ? "parallel" : "default";

// Parallel-mode state
const queue: string[] = [];
const runs = new Map<string, Run>();
let activeCount = 0;
let runCounter = 0;
const SESSION = Date.now().toString(36).slice(-4); // distinguishes runs across app launches
let overnightActive = false;
let selectedRun: string | null = null;
let pActiveFile: string | null = null;

// ---- Project selection ----
async function chooseProject() {
  const picked = await open({ directory: true, multiple: false, title: "Choose a repo" });
  if (typeof picked === "string") setProject(picked);
}
function setProject(path: string) {
  project = path;
  localStorage.setItem("foreman.project", path);
  $("project-path").textContent = path;
  $("project-path").classList.remove("muted");
  ($("open-finder") as HTMLButtonElement).disabled = false;
  ($("init-btn") as HTMLButtonElement).disabled = false;
  updateStartBtn();
  refreshStatus();
}

// ---- Status (default mode) ----
async function refreshStatus() {
  if (!project) return;
  try {
    renderStatus(await invoke<PipelineStatus>("pipeline_status", { project }));
  } catch (e) {
    appendLog({ run_id: DEFAULT_RUN, kind: "stderr", text: `status error: ${e}`, raw: "" });
  }
}
function renderStatus(status: PipelineStatus) {
  const grid = $("status-grid");
  grid.innerHTML = "";
  for (const a of status.agents) {
    const chip = document.createElement("span");
    chip.className = `chip ${a.present ? "ok" : "miss"}`;
    chip.textContent = `${a.present ? "✓" : "○"} ${a.name}`;
    grid.appendChild(chip);
    const sel = document.querySelector<HTMLSelectElement>(`.model-select[data-agent="${a.name}"]`);
    if (sel) {
      sel.disabled = !a.present || running;
      const v = (a.model || "").toLowerCase();
      if (v && Array.from(sel.options).some((o) => o.value === v)) sel.value = v;
    }
  }
  const cmd = document.createElement("span");
  cmd.className = `chip ${status.has_ship_command ? "ok" : "miss"}`;
  cmd.textContent = `${status.has_ship_command ? "✓" : "○"} /ship`;
  grid.appendChild(cmd);
  ($("run-btn") as HTMLButtonElement).disabled = !status.initialized || running;
  renderFileTabs(status.handoffs);
}

// ---- Init ----
async function initPipeline() {
  if (!project) return;
  const force = ($("force-init") as HTMLInputElement).checked;
  try {
    const res = await invoke<InitResult>("init_pipeline", { project, force });
    const created = res.files.filter((f) => f.action !== "skipped").length;
    appendLog({
      run_id: DEFAULT_RUN,
      kind: "system",
      text: `installed pipeline — ${created} written, ${res.files.length - created} skipped`,
      raw: "",
    });
    await refreshStatus();
  } catch (e) {
    appendLog({ run_id: DEFAULT_RUN, kind: "stderr", text: `init error: ${e}`, raw: "" });
  }
}

// ---- Default-mode run ----
async function runPipeline() {
  if (!project || running) return;
  const request = ($("request") as HTMLTextAreaElement).value.trim();
  if (!request) {
    ($("request") as HTMLTextAreaElement).focus();
    return;
  }
  const permission_mode = ($("perm-mode") as HTMLSelectElement).value;
  const clean_first = ($("clean-first") as HTMLInputElement).checked;
  const effort = currentEffort();

  resetStages();
  hideVerdict();
  resetRunUsage();
  if (clean_first) $("log").innerHTML = "";
  setRunning(true);
  setStageState("planner", "running");

  try {
    await invoke("run_pipeline", {
      runId: DEFAULT_RUN,
      project,
      request,
      permissionMode: permission_mode,
      effort,
      autonomous: false,
      cleanFirst: clean_first,
    });
    appendLog({ run_id: DEFAULT_RUN, kind: "system", text: `▶ shipping (effort: ${effort}): ${request}`, raw: "" });
  } catch (e) {
    appendLog({ run_id: DEFAULT_RUN, kind: "stderr", text: `run error: ${e}`, raw: "" });
    setRunning(false);
  }
}
async function cancel() {
  try {
    await invoke("cancel_run", { runId: DEFAULT_RUN });
    appendLog({ run_id: DEFAULT_RUN, kind: "system", text: "■ cancelled", raw: "" });
  } catch (e) {
    appendLog({ run_id: DEFAULT_RUN, kind: "stderr", text: `cancel error: ${e}`, raw: "" });
  }
  setRunning(false);
}
function setRunning(state: boolean) {
  running = state;
  ($("run-btn") as HTMLButtonElement).disabled = state || !project;
  ($("cancel-btn") as HTMLButtonElement).disabled = !state;
  ($("choose-project") as HTMLButtonElement).disabled = state;
  document.querySelectorAll<HTMLSelectElement>(".model-select").forEach((s) => {
    if (state) s.disabled = true;
  });
}

// ---- Default-mode stages ----
function resetStages() {
  for (const agent of STAGE_ORDER) setStageState(agent, "idle");
}
function setStageState(agent: string, state: StageState) {
  const el = document.querySelector<HTMLElement>(`#mode-default .agent[data-agent="${agent}"]`);
  if (!el) return;
  el.classList.remove("running", "done", "blocked");
  if (state !== "idle") el.classList.add(state);
  el.querySelector(".agent-state")!.textContent =
    state === "idle" ? "idle" : state === "running" ? "working…" : state === "done" ? "done ✓" : "stopped";
}

// ---- File viewer (default mode) ----
function renderFileTabs(handoffs: HandoffFile[]) {
  const tabs = $("filetabs");
  tabs.innerHTML = "";
  for (const h of handoffs) {
    const btn = document.createElement("button");
    btn.textContent = h.name;
    btn.disabled = !h.exists;
    if (h.name === activeFile) btn.classList.add("active");
    btn.onclick = () => showFile(h.name);
    tabs.appendChild(btn);
  }
}
async function showFile(name: string) {
  if (!project) return;
  activeFile = name;
  document.querySelectorAll("#filetabs button").forEach((b) => b.classList.toggle("active", b.textContent === name));
  try {
    $("file-view").innerHTML = await marked.parse(await invoke<string>("read_handoff", { project, name }));
  } catch (e) {
    $("file-view").innerHTML = `<p class="muted">Could not read ${name}: ${e}</p>`;
  }
}

// ---- Log ----
function writeLogLine(log: HTMLElement, kind: string, text: string) {
  if (!text && kind !== "stderr") return;
  const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  const line = document.createElement("div");
  line.className = `line ${kind}`;
  const k = document.createElement("span");
  k.className = "k";
  k.textContent = kind;
  line.appendChild(k);
  line.appendChild(document.createTextNode(text));
  log.appendChild(line);
  if (atBottom) log.scrollTop = log.scrollHeight;
}
function appendLog(evt: LogEvent) {
  writeLogLine($("log"), evt.kind, evt.text);
}

// ---- Notifications ----
async function notify(title: string, body: string) {
  let granted = await isPermissionGranted();
  if (!granted) granted = (await requestPermission()) === "granted";
  if (granted) sendNotification({ title, body });
}

// ---- Verdict (default mode) ----
function classifyVerdict(v: string | null): "ship" | "needs" | "block" | null {
  if (!v) return null;
  const u = v.toUpperCase();
  if (u.includes("SHIP")) return "ship";
  if (u.includes("NEEDS")) return "needs";
  if (u.includes("BLOCK")) return "block";
  return null;
}
function showVerdict(verdict: string | null) {
  const el = $("verdict");
  const cls = classifyVerdict(verdict);
  el.className = "verdict";
  if (!cls) {
    el.classList.add("hidden");
    return;
  }
  el.classList.add(cls);
  el.textContent =
    cls === "ship" ? "VERDICT: SHIP — ready for your review"
      : cls === "needs" ? "VERDICT: NEEDS WORK — see review.md"
      : "VERDICT: BLOCK — see review.md";
}
function hideVerdict() {
  $("verdict").className = "verdict hidden";
}

// ---- Effort (global, shared by both modes) ----
function currentEffort(): string {
  const idx = Number(($("effort") as HTMLInputElement).value) - 1;
  return EFFORT_LEVELS[Math.max(0, Math.min(EFFORT_LEVELS.length - 1, idx))];
}
function updateEffortLabel() {
  $("effort-label").textContent = currentEffort();
  localStorage.setItem("foreman.effort", ($("effort") as HTMLInputElement).value);
}

// ---- Usage (default mode, tokens only) ----
function resetRunUsage() {
  $("usage-run").textContent = "running…";
}
function defaultUsage(u: UsageEvent) {
  if (u.is_final) {
    $("usage-run").textContent = `${fmtTokens(u.input_tokens)} in · ${fmtTokens(u.output_tokens)} out`;
    sessionTokens += u.input_tokens + u.output_tokens;
    localStorage.setItem("foreman.sessionTokens", String(sessionTokens));
    renderSession();
  } else {
    $("usage-run").textContent = `${fmtTokens(u.output_tokens)} out (so far)`;
  }
}
function renderSession() {
  $("usage-session").textContent = `${fmtTokens(sessionTokens)} tok`;
}

// ---- Default-mode event handlers ----
function defaultStage(p: StageEvent) {
  setStageState(p.agent, "done");
  const idx = STAGE_ORDER.indexOf(p.agent);
  if (idx >= 0 && idx < STAGE_ORDER.length - 1) setStageState(STAGE_ORDER[idx + 1], "running");
  refreshStatus();
  showFile(HANDOFF_FOR[p.agent]);
}
function defaultDone(p: DoneEvent) {
  setRunning(false);
  document.querySelectorAll<HTMLElement>("#mode-default .agent.running").forEach((el) => {
    el.classList.remove("running");
    el.classList.add("blocked");
    el.querySelector(".agent-state")!.textContent = "stopped";
  });
  showVerdict(p.verdict);
  refreshStatus();
  notify(
    "Foreman — pipeline finished",
    classifyVerdict(p.verdict) ? `Verdict: ${p.verdict}` : "Pipeline stopped — check the handoff files.",
  );
}

// ---- Parallel / overnight mode ----
function shortSlug(s: string): string {
  const slug = s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  return slug || "run"; // Rust caps the length; keep meaningful tokens like "99"
}
function updateStartBtn() {
  ($("start-overnight") as HTMLButtonElement).disabled = !project || queue.length === 0 || overnightActive;
}
function addToQueue() {
  const ta = $("queue-input") as HTMLTextAreaElement;
  const req = ta.value.trim();
  if (!req) {
    ta.focus();
    return;
  }
  queue.push(req);
  ta.value = "";
  renderQueueList();
  updateStartBtn();
}
function renderQueueList() {
  const list = $("queue-list");
  list.innerHTML = "";
  queue.forEach((req, i) => {
    const item = document.createElement("div");
    item.className = "queue-item";
    const t = document.createElement("span");
    t.className = "q-text";
    t.textContent = req;
    const x = document.createElement("button");
    x.textContent = "×";
    x.title = "remove";
    x.onclick = () => {
      queue.splice(i, 1);
      renderQueueList();
      updateStartBtn();
    };
    item.append(t, x);
    list.appendChild(item);
  });
}
function startOvernight() {
  if (!project || queue.length === 0) return;
  overnightActive = true;
  updateStartBtn();
  ($("stop-all") as HTMLButtonElement).disabled = false;
  pump();
}
async function stopAll() {
  await invoke("cancel_all").catch(() => {});
  overnightActive = false;
  for (const run of runs.values()) {
    if (run.status === "working" || run.status === "queued") {
      run.status = "stopped";
      renderRunCard(run);
    }
  }
  activeCount = 0;
  ($("stop-all") as HTMLButtonElement).disabled = true;
  updateStartBtn();
}
function pump() {
  if (!overnightActive) return;
  const n = Math.max(1, Math.min(6, Number(($("concurrency") as HTMLInputElement).value) || 2));
  while (activeCount < n && queue.length > 0) {
    const req = queue.shift()!;
    renderQueueList();
    void startOneRun(req);
  }
  if (queue.length === 0 && activeCount === 0) {
    overnightActive = false;
    updateStartBtn();
    ($("stop-all") as HTMLButtonElement).disabled = true;
    const done = [...runs.values()].filter((r) => r.status === "done").length;
    notify("Foreman — overnight finished", `${done} run(s) complete. Review the verdicts in the morning.`);
  }
}
async function startOneRun(request: string) {
  if (!project) return;
  runCounter += 1;
  // Unique per run (session stamp + counter) so re-prompting the same feature always makes
  // a NEW branch — never collides with, reuses, or clobbers a previous run's worktree.
  const id = `${shortSlug(request).slice(0, 26)}-${SESSION}${runCounter}`;
  const run: Run = {
    id,
    request,
    worktree: null,
    branch: null,
    stages: { planner: "idle", coder: "idle", tester: "idle", reviewer: "idle" },
    verdict: null,
    status: "queued",
    tokens: 0,
    tokensFinal: false,
    log: [],
  };
  runs.set(id, run);
  activeCount += 1;
  renderRunCard(run);
  if (!selectedRun) selectRun(id);
  try {
    const wt = await invoke<WorktreeInfo>("create_worktree", { repo: project, slug: id });
    run.worktree = wt.path;
    run.branch = wt.branch;
    renderRunCard(run);
    await invoke("run_pipeline", {
      runId: id,
      project: wt.path,
      request,
      permissionMode: "bypassPermissions",
      effort: currentEffort(),
      autonomous: true,
      cleanFirst: false,
    });
    run.status = "working";
    run.stages.planner = "running";
    renderRunCard(run);
    pRunLog({ run_id: id, kind: "system", text: `▶ ${request}`, raw: "" });
  } catch (e) {
    run.status = "stopped";
    pRunLog({ run_id: id, kind: "stderr", text: `run error: ${e}`, raw: "" });
    renderRunCard(run);
    activeCount = Math.max(0, activeCount - 1);
    pump();
  }
}
function pillClass(run: Run): string {
  if (run.status === "queued") return "queued";
  if (run.status === "working") return "working";
  if (run.status === "stopped") return "stopped";
  return classifyVerdict(run.verdict) || "stopped";
}
function pillText(run: Run): string {
  if (run.status === "queued") return "queued";
  if (run.status === "working") return "working";
  if (run.status === "stopped") return "stopped";
  return run.verdict || "done";
}
function renderRunCard(run: Run) {
  const list = $("runs-list");
  if (!run.el) {
    if (list.querySelector(".muted")) list.innerHTML = "";
    const card = document.createElement("div");
    card.className = "run-card";
    card.dataset.id = run.id;
    card.onclick = () => selectRun(run.id);
    list.appendChild(card);
    run.el = card;
  }
  run.el.classList.toggle("selected", selectedRun === run.id);
  const dots = STAGE_ORDER.map((a) => `<span class="md ${run.stages[a] === "idle" ? "" : run.stages[a]}"></span>`).join("");
  const tok = run.tokens ? `${fmtTokens(run.tokens)} tok${run.tokensFinal ? "" : "…"}` : "";
  run.el.innerHTML = `
    <div class="run-req">${escapeHtml(run.request)}</div>
    <div class="mini-stages">${dots}</div>
    <div class="run-meta">
      <span class="run-branch">${run.branch ? escapeHtml(run.branch) : "creating worktree…"}</span>
      <span class="run-tokens">${tok}</span>
      <span class="run-verdict ${pillClass(run)}">${escapeHtml(pillText(run))}</span>
    </div>`;
}
function selectRun(id: string) {
  selectedRun = id;
  const run = runs.get(id);
  if (!run) return;
  document.querySelectorAll(".run-card").forEach((c) => c.classList.toggle("selected", (c as HTMLElement).dataset.id === id));
  $("detail-title").textContent = run.request.slice(0, 90);
  renderPFileTabs(run);
  $("p-log").innerHTML = "";
  for (const l of run.log) writeLogLine($("p-log"), l.kind, l.text);
  if (run.worktree) showPFile(run, run.verdict ? "review.md" : "spec.md");
  else $("p-file-view").innerHTML = `<p class="muted">Worktree is being created…</p>`;
}
function renderPFileTabs(run: Run) {
  const tabs = $("p-filetabs");
  tabs.innerHTML = "";
  for (const name of HANDOFFS) {
    const btn = document.createElement("button");
    btn.textContent = name;
    btn.disabled = !run.worktree;
    if (name === pActiveFile) btn.classList.add("active");
    btn.onclick = () => showPFile(run, name);
    tabs.appendChild(btn);
  }
}
async function showPFile(run: Run, name: string) {
  if (!run.worktree) return;
  pActiveFile = name;
  document.querySelectorAll("#p-filetabs button").forEach((b) => b.classList.toggle("active", b.textContent === name));
  try {
    $("p-file-view").innerHTML = await marked.parse(await invoke<string>("read_handoff", { project: run.worktree, name }));
  } catch {
    $("p-file-view").innerHTML = `<p class="muted">${name} not produced yet.</p>`;
  }
}
function pRunLog(p: LogEvent) {
  const run = runs.get(p.run_id);
  if (!run) return;
  if (!p.text && p.kind !== "stderr") return;
  run.log.push({ kind: p.kind, text: p.text });
  if (run.log.length > 500) run.log.shift();
  if (selectedRun === p.run_id) writeLogLine($("p-log"), p.kind, p.text);
}
function pRunStage(p: StageEvent) {
  const run = runs.get(p.run_id);
  if (!run) return;
  run.stages[p.agent] = "done";
  const idx = STAGE_ORDER.indexOf(p.agent);
  if (idx >= 0 && idx < STAGE_ORDER.length - 1) run.stages[STAGE_ORDER[idx + 1]] = "running";
  if (run.status === "queued") run.status = "working";
  renderRunCard(run);
  if (selectedRun === p.run_id && run.worktree) showPFile(run, HANDOFF_FOR[p.agent]);
}
function pRunUsage(p: UsageEvent) {
  const run = runs.get(p.run_id);
  if (!run) return;
  run.tokens = p.is_final ? p.input_tokens + p.output_tokens : p.output_tokens;
  run.tokensFinal = p.is_final;
  renderRunCard(run);
}
function pRunDone(p: DoneEvent) {
  const run = runs.get(p.run_id);
  if (!run) return;
  run.verdict = p.verdict;
  run.status = "done";
  for (const a of STAGE_ORDER) {
    if (run.stages[a] === "running") run.stages[a] = p.verdict ? "done" : "blocked";
  }
  renderRunCard(run);
  activeCount = Math.max(0, activeCount - 1);
  if (selectedRun === run.id && run.worktree) showPFile(run, "review.md");
  pump();
}

// ---- Mode switching ----
function setMode(mode: "default" | "parallel") {
  appMode = mode;
  localStorage.setItem("foreman.mode", mode);
  ($("mode-default") as HTMLElement).hidden = mode !== "default";
  ($("mode-parallel") as HTMLElement).hidden = mode !== "parallel";
  document.querySelectorAll(".menu-item").forEach((m) => m.classList.toggle("active", (m as HTMLElement).dataset.mode === mode));
  $("mode-tag").textContent = mode === "parallel" ? "overnight" : "agent pipeline";
  $("menu").classList.add("hidden");
}

// ---- Shipper window (5th agent) ----
async function openShipper() {
  const existing = await WebviewWindow.getByLabel("shipper");
  if (existing) {
    await existing.setFocus();
    return;
  }
  new WebviewWindow("shipper", {
    url: "shipper.html",
    title: "Foreman — Shipper",
    width: 760,
    height: 760,
    minWidth: 520,
    minHeight: 480,
  });
}

// ---- Event wiring (routed by run_id) ----
listen<LogEvent>("pipeline-log", (e) => (runs.has(e.payload.run_id) ? pRunLog(e.payload) : appendLog(e.payload)));
listen<StageEvent>("pipeline-stage", (e) => (runs.has(e.payload.run_id) ? pRunStage(e.payload) : defaultStage(e.payload)));
listen<UsageEvent>("pipeline-usage", (e) => (runs.has(e.payload.run_id) ? pRunUsage(e.payload) : defaultUsage(e.payload)));
listen<DoneEvent>("pipeline-done", (e) => (runs.has(e.payload.run_id) ? pRunDone(e.payload) : defaultDone(e.payload)));

// ---- Boot ----
$("menu-btn").addEventListener("click", (e) => {
  e.stopPropagation();
  $("menu").classList.toggle("hidden");
});
$("menu").addEventListener("click", (e) => e.stopPropagation());
document.addEventListener("click", () => $("menu").classList.add("hidden"));
document.querySelectorAll(".menu-item").forEach((m) =>
  m.addEventListener("click", () => setMode((m as HTMLElement).dataset.mode as "default" | "parallel")),
);

$("choose-project").addEventListener("click", chooseProject);
$("init-btn").addEventListener("click", initPipeline);
$("run-btn").addEventListener("click", runPipeline);
$("cancel-btn").addEventListener("click", cancel);
$("clear-log").addEventListener("click", () => ($("log").innerHTML = ""));
$("open-finder").addEventListener("click", () => {
  if (project) revealItemInDir(project).catch(() => {});
});
$("open-shipper").addEventListener("click", openShipper);
$("effort").addEventListener("input", updateEffortLabel);
document.querySelectorAll<HTMLSelectElement>(".model-select").forEach((sel) => {
  sel.addEventListener("change", async () => {
    if (!project) return;
    const agent = sel.dataset.agent!;
    try {
      await invoke("set_agent_model", { project, agent, model: sel.value });
      appendLog({ run_id: DEFAULT_RUN, kind: "system", text: `${agent} → ${sel.value}`, raw: "" });
    } catch (err) {
      appendLog({ run_id: DEFAULT_RUN, kind: "stderr", text: `set model error: ${err}`, raw: "" });
      refreshStatus();
    }
  });
});

$("queue-add").addEventListener("click", addToQueue);
$("start-overnight").addEventListener("click", startOvernight);
$("stop-all").addEventListener("click", stopAll);
$("p-clear-log").addEventListener("click", () => ($("p-log").innerHTML = ""));

const savedEffort = localStorage.getItem("foreman.effort");
if (savedEffort) ($("effort") as HTMLInputElement).value = savedEffort;
updateEffortLabel();
renderSession();
setMode(appMode);

if (project) {
  setProject(project);
} else {
  appendLog({ run_id: DEFAULT_RUN, kind: "system", text: "choose a repo to begin", raw: "" });
}
