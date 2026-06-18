import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
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
type PipelineStatus = {
  initialized: boolean;
  agents_present: string[];
  agents_missing: string[];
  has_ship_command: boolean;
  handoffs: HandoffFile[];
};
type LogEvent = { kind: string; text: string; raw: string };
type StageEvent = { agent: string; file: string };
type DoneEvent = { code: number | null; verdict: string | null };

const STAGE_ORDER = ["planner", "coder", "tester", "reviewer"];
const HANDOFF_FOR: Record<string, string> = {
  planner: "spec.md",
  coder: "changes.md",
  tester: "test-results.md",
  reviewer: "review.md",
};

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

let project: string | null = localStorage.getItem("foreman.project");
let running = false;
let activeFile: string | null = null;

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
  refreshStatus();
}

// ---- Status ----
async function refreshStatus() {
  if (!project) return;
  try {
    const status = await invoke<PipelineStatus>("pipeline_status", { project });
    renderStatus(status);
  } catch (e) {
    appendLog({ kind: "stderr", text: `status error: ${e}`, raw: "" });
  }
}

function renderStatus(status: PipelineStatus) {
  const grid = $("status-grid");
  grid.innerHTML = "";
  for (const agent of STAGE_ORDER) {
    const present = status.agents_present.includes(agent);
    const chip = document.createElement("span");
    chip.className = `chip ${present ? "ok" : "miss"}`;
    chip.textContent = `${present ? "✓" : "○"} ${agent}`;
    grid.appendChild(chip);
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
      kind: "system",
      text: `installed pipeline — ${created} written, ${res.files.length - created} skipped`,
      raw: "",
    });
    await refreshStatus();
  } catch (e) {
    appendLog({ kind: "stderr", text: `init error: ${e}`, raw: "" });
  }
}

// ---- Run ----
async function runPipeline() {
  if (!project || running) return;
  const request = ($("request") as HTMLTextAreaElement).value.trim();
  if (!request) {
    ($("request") as HTMLTextAreaElement).focus();
    return;
  }
  const permission_mode = ($("perm-mode") as HTMLSelectElement).value;
  const clean_first = ($("clean-first") as HTMLInputElement).checked;

  resetStages();
  hideVerdict();
  if (clean_first) $("log").innerHTML = "";
  setRunning(true);
  setStageState("planner", "running");

  try {
    await invoke("run_pipeline", { project, request, permissionMode: permission_mode, cleanFirst: clean_first });
    appendLog({ kind: "system", text: `▶ shipping: ${request}`, raw: "" });
  } catch (e) {
    appendLog({ kind: "stderr", text: `run error: ${e}`, raw: "" });
    setRunning(false);
  }
}

async function cancel() {
  try {
    await invoke("cancel_pipeline");
    appendLog({ kind: "system", text: "■ cancelled", raw: "" });
  } catch (e) {
    appendLog({ kind: "stderr", text: `cancel error: ${e}`, raw: "" });
  }
  setRunning(false);
}

function setRunning(state: boolean) {
  running = state;
  ($("run-btn") as HTMLButtonElement).disabled = state || !project;
  ($("cancel-btn") as HTMLButtonElement).disabled = !state;
  ($("choose-project") as HTMLButtonElement).disabled = state;
}

// ---- Stages ----
function resetStages() {
  for (const agent of STAGE_ORDER) setStageState(agent, "idle");
}
function setStageState(agent: string, state: "idle" | "running" | "done" | "blocked") {
  const el = document.querySelector<HTMLElement>(`.agent[data-agent="${agent}"]`);
  if (!el) return;
  el.classList.remove("running", "done", "blocked");
  if (state !== "idle") el.classList.add(state);
  const label =
    state === "idle" ? "idle" : state === "running" ? "working…" : state === "done" ? "done ✓" : "stopped";
  el.querySelector(".agent-state")!.textContent = label;
}

// ---- File viewer ----
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
  document.querySelectorAll(".filetabs button").forEach((b) => {
    b.classList.toggle("active", b.textContent === name);
  });
  try {
    const content = await invoke<string>("read_handoff", { project, name });
    $("file-view").innerHTML = await marked.parse(content);
  } catch (e) {
    $("file-view").innerHTML = `<p class="muted">Could not read ${name}: ${e}</p>`;
  }
}

// ---- Log ----
function appendLog(evt: LogEvent) {
  if (!evt.text && evt.kind !== "stderr") return;
  const log = $("log");
  const atBottom = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  const line = document.createElement("div");
  line.className = `line ${evt.kind}`;
  const k = document.createElement("span");
  k.className = "k";
  k.textContent = evt.kind;
  line.appendChild(k);
  line.appendChild(document.createTextNode(evt.text));
  log.appendChild(line);
  if (atBottom) log.scrollTop = log.scrollHeight;
}

// ---- Notifications ----
async function notify(title: string, body: string) {
  let granted = await isPermissionGranted();
  if (!granted) granted = (await requestPermission()) === "granted";
  if (granted) sendNotification({ title, body });
}

// ---- Done ----
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
    cls === "ship" ? "VERDICT: SHIP — ready for your review" :
    cls === "needs" ? "VERDICT: NEEDS WORK — see review.md" :
    "VERDICT: BLOCK — see review.md";
}
function hideVerdict() {
  $("verdict").className = "verdict hidden";
}

// ---- Event wiring ----
listen<LogEvent>("pipeline-log", (e) => appendLog(e.payload));

listen<StageEvent>("pipeline-stage", (e) => {
  const { agent } = e.payload;
  setStageState(agent, "done");
  const idx = STAGE_ORDER.indexOf(agent);
  if (idx >= 0 && idx < STAGE_ORDER.length - 1) setStageState(STAGE_ORDER[idx + 1], "running");
  refreshStatus();
  showFile(HANDOFF_FOR[agent]); // auto-open the freshly produced handoff
});

listen<DoneEvent>("pipeline-done", async (e) => {
  setRunning(false);
  const { verdict } = e.payload;
  // Any stage still "working" but with no handoff means the pipeline stopped early.
  document.querySelectorAll<HTMLElement>(".agent.running").forEach((el) => {
    el.classList.remove("running");
    el.classList.add("blocked");
    el.querySelector(".agent-state")!.textContent = "stopped";
  });
  showVerdict(verdict);
  await refreshStatus();
  const v = classifyVerdict(verdict);
  notify(
    "Foreman — pipeline finished",
    v ? `Verdict: ${verdict}` : "Pipeline stopped — check the handoff files.",
  );
});

// ---- Boot ----
$("choose-project").addEventListener("click", chooseProject);
$("init-btn").addEventListener("click", initPipeline);
$("run-btn").addEventListener("click", runPipeline);
$("cancel-btn").addEventListener("click", cancel);
$("clear-log").addEventListener("click", () => ($("log").innerHTML = ""));
$("open-finder").addEventListener("click", () => {
  if (project) revealItemInDir(project).catch(() => {});
});

if (project) {
  setProject(project);
} else {
  appendLog({ kind: "system", text: "choose a repo to begin", raw: "" });
}
