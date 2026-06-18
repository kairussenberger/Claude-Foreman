import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { marked } from "marked";

type ShipperEvent = { kind: string; text: string };

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const project = localStorage.getItem("foreman.project");
let sessionId: string | null = null;
let busy = false;

function classifyVerdict(v: string | null): "ship" | "needs" | "block" | null {
  if (!v) return null;
  const u = v.toUpperCase();
  if (u.includes("SHIP")) return "ship";
  if (u.includes("NEEDS")) return "needs";
  if (u.includes("BLOCK")) return "block";
  return null;
}

async function loadVerdict() {
  const badge = $("s-verdict");
  if (!project) {
    badge.className = "run-verdict queued";
    badge.textContent = "no repo";
    return;
  }
  try {
    const txt = await invoke<string>("read_handoff", { project, name: "review.md" });
    const line = txt.split("\n").find((l) => l.toUpperCase().includes("VERDICT:"));
    const v = line ? line.replace(/.*VERDICT:/i, "").trim() : null;
    badge.className = `run-verdict ${classifyVerdict(v) || "queued"}`;
    badge.textContent = v ? `VERDICT: ${v}` : "no verdict";
  } catch {
    badge.className = "run-verdict queued";
    badge.textContent = "no pipeline output yet";
  }
}

function addLine(kind: string, text: string, asHtml = false) {
  const chat = $("s-chat");
  const atBottom = chat.scrollHeight - chat.scrollTop - chat.clientHeight < 60;
  const el = document.createElement("div");
  el.className = `bubble ${kind}`;
  if (asHtml) el.innerHTML = text;
  else el.textContent = text;
  chat.appendChild(el);
  if (atBottom) chat.scrollTop = chat.scrollHeight;
}

function setBusy(b: boolean) {
  busy = b;
  ($("s-send") as HTMLButtonElement).disabled = b;
  ($("s-cancel") as HTMLButtonElement).disabled = !b;
}

async function send() {
  if (busy) return;
  if (!project) {
    addLine("stderr", "No repo selected — pick one in the main Foreman window first.");
    return;
  }
  const ta = $("s-prompt") as HTMLTextAreaElement;
  const prompt = ta.value.trim();
  if (!prompt) {
    ta.focus();
    return;
  }
  ta.value = "";
  addLine("you", prompt);
  setBusy(true);
  try {
    await invoke("ship_agent", { project, prompt, resume: sessionId });
  } catch (e) {
    addLine("stderr", `error: ${e}`);
    setBusy(false);
  }
}

listen<ShipperEvent>("shipper-log", async (e) => {
  const { kind, text } = e.payload;
  if (kind === "assistant" || kind === "result") {
    addLine(kind, await marked.parse(text), true);
  } else {
    addLine(kind, text);
  }
});
listen<string>("shipper-session", (e) => {
  sessionId = e.payload;
});
listen("shipper-done", () => setBusy(false));

$("s-send").addEventListener("click", send);
$("s-cancel").addEventListener("click", () => invoke("cancel_run", { runId: "shipper" }).catch(() => {}));
$("s-refresh").addEventListener("click", loadVerdict);
$("s-prompt").addEventListener("keydown", (e) => {
  const k = e as KeyboardEvent;
  if (k.key === "Enter" && (k.metaKey || k.ctrlKey)) {
    e.preventDefault();
    send();
  }
});

$("s-project").textContent = project || "no repo selected";
if (project) $("s-project").classList.remove("muted");
loadVerdict();
