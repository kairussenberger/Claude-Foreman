// Foreman — a cockpit for the four-agent Claude Code pipeline.
//
// Jobs:
//   1. Folder management — install the .claude/agents + .claude/commands + .pipeline scaffold.
//   2. Handoff files     — read/list/clean the .pipeline/*.md files agents hand off through.
//   3. Pipeline          — spawn `claude -p "/ship ..."` headless, stream output, track stages.
//   4. Parallel runs     — each keyed by a run id; overnight runs each get a git worktree.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};

// --- The pipeline assets, embedded in the binary. Installed into target repos/worktrees. ---

const PLANNER_MD: &str = include_str!("../templates/planner.md");
const CODER_MD: &str = include_str!("../templates/coder.md");
const TESTER_MD: &str = include_str!("../templates/tester.md");
const REVIEWER_MD: &str = include_str!("../templates/reviewer.md");
const FAST_CODER_MD: &str = include_str!("../templates/fast-coder.md");
const FAST_REVIEWER_MD: &str = include_str!("../templates/fast-reviewer.md");
const SHIP_MD: &str = include_str!("../templates/ship.md");
const SHIP_AUTO_MD: &str = include_str!("../templates/ship-auto.md");
const SHIP_FAST_MD: &str = include_str!("../templates/ship-fast.md");
const SETTINGS_JSON: &str = include_str!("../templates/settings.json");

struct Asset {
    rel: &'static str,
    contents: &'static str,
}

fn assets() -> Vec<Asset> {
    vec![
        Asset { rel: ".claude/agents/planner.md", contents: PLANNER_MD },
        Asset { rel: ".claude/agents/coder.md", contents: CODER_MD },
        Asset { rel: ".claude/agents/tester.md", contents: TESTER_MD },
        Asset { rel: ".claude/agents/reviewer.md", contents: REVIEWER_MD },
        Asset { rel: ".claude/agents/fast-coder.md", contents: FAST_CODER_MD },
        Asset { rel: ".claude/agents/fast-reviewer.md", contents: FAST_REVIEWER_MD },
        Asset { rel: ".claude/commands/ship.md", contents: SHIP_MD },
        Asset { rel: ".claude/commands/ship-auto.md", contents: SHIP_AUTO_MD },
        Asset { rel: ".claude/commands/ship-fast.md", contents: SHIP_FAST_MD },
        Asset { rel: ".claude/settings.json", contents: SETTINGS_JSON },
    ]
}

// The four handoff files, in pipeline order, each tied to the agent that produces it.
const STAGES: [(&str, &str); 4] = [
    ("planner", "spec.md"),
    ("coder", "changes.md"),
    ("tester", "test-results.md"),
    ("reviewer", "review.md"),
];

// The four CORE agents (default pipeline). `initialized`/doctor readiness keys off these.
const AGENTS: [&str; 4] = ["planner", "coder", "tester", "reviewer"];
// Everything Foreman installs + lets you edit / set a model on / delegate to (core + the
// fast pipeline's own independent agents).
const ALL_AGENTS: [&str; 6] = ["planner", "coder", "tester", "reviewer", "fast-coder", "fast-reviewer"];

// Models offered in the per-agent picker. Aliases resolve to the latest version;
// `inherit` means "use the session model". Full ids (claude-opus-4-8, …) also work.
const ALLOWED_MODELS: [&str; 5] = ["opus", "sonnet", "haiku", "fable", "inherit"];

// --- Shared state: in-flight child processes, keyed by run id (so runs can be parallel). ---

#[derive(Default)]
struct RunState {
    children: Arc<Mutex<HashMap<String, Child>>>,
}

// --- Serializable payloads sent to the frontend. ---

#[derive(Serialize)]
struct FileResult {
    path: String,
    action: String, // "created" | "updated" | "skipped"
}

#[derive(Serialize)]
struct InitResult {
    project: String,
    files: Vec<FileResult>,
}

#[derive(Serialize)]
struct HandoffFile {
    name: String,
    exists: bool,
    size: u64,
    modified_ms: Option<u64>,
}

#[derive(Serialize)]
struct AgentInfo {
    name: String,
    present: bool,
    model: Option<String>,
}

#[derive(Serialize)]
struct PipelineStatus {
    initialized: bool,
    agents: Vec<AgentInfo>,
    has_ship_command: bool,
    handoffs: Vec<HandoffFile>,
}

#[derive(Serialize)]
struct WorktreeInfo {
    path: String,
    branch: String,
}

#[derive(Serialize)]
struct SkillInfo {
    name: String,
    description: String,
}

#[derive(Serialize)]
struct DoctorCheck {
    name: String,
    ok: bool,
    detail: String,
}

// Every run-scoped event carries `run_id` so the UI can route it. "default" = the
// single-run (non-parallel) mode.
#[derive(Clone, Serialize)]
struct LogEvent {
    run_id: String,
    kind: String,
    text: String,
    raw: String,
}

#[derive(Clone, Serialize)]
struct StageEvent {
    run_id: String,
    agent: String,
    file: String,
    phase: String, // "running" (agent delegated) | "done" (handoff file produced)
}

#[derive(Clone, Serialize)]
struct DoneEvent {
    run_id: String,
    code: Option<i32>,
    verdict: Option<String>,
}

#[derive(Clone, Serialize)]
struct UsageEvent {
    run_id: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read: u64,
    cache_creation: u64,
    is_final: bool,
}

#[derive(Clone, Serialize)]
struct ShipperEvent {
    kind: String,
    text: String,
}

#[derive(Clone, Serialize)]
struct SessionEvent {
    run_id: String,
    session_id: String,
}

// --- Helpers ---

fn modified_ms(path: &Path) -> Option<u64> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let dur = mtime.duration_since(UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as u64)
}

/// Newest `claude` under ~/.nvm/versions/node/*/bin (npm-global installs live here).
fn newest_nvm_claude(home: &str) -> Option<String> {
    let base = PathBuf::from(home).join(".nvm/versions/node");
    let mut versions: Vec<PathBuf> =
        fs::read_dir(&base).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    versions.sort();
    for v in versions.iter().rev() {
        let c = v.join("bin/claude");
        if c.exists() {
            return Some(c.to_string_lossy().to_string());
        }
    }
    None
}

/// Find the `claude` binary. GUI/launchd launches get a minimal PATH that omits nvm/asdf,
/// so we (1) scan known install locations, then (2) ask an *interactive* login shell
/// (which sources ~/.zshrc where version managers live), then (3) fall back to "claude".
fn resolve_claude() -> String {
    let home = std::env::var("HOME").unwrap_or_default();

    let mut candidates = vec![
        format!("{home}/.local/bin/claude"),
        format!("{home}/.claude/local/claude"),
        format!("{home}/bin/claude"),
        "/opt/homebrew/bin/claude".to_string(),
        "/usr/local/bin/claude".to_string(),
    ];
    if let Some(p) = newest_nvm_claude(&home) {
        candidates.push(p);
    }
    for c in &candidates {
        if Path::new(c).exists() {
            return c.clone();
        }
    }

    // Interactive login shell sources ~/.zshrc (nvm/fnm/volta/asdf); plain -lc does not.
    for args in [["-ilc", "command -v claude"], ["-lc", "command -v claude"]] {
        if let Ok(out) = Command::new("/bin/zsh")
            .env("TERM", "xterm-256color")
            .args(args)
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(p) = s.lines().map(|l| l.trim()).filter(|l| l.starts_with('/')).last() {
                    return p.to_string();
                }
            }
        }
    }

    "claude".to_string()
}

/// Resolve a binary via an interactive login shell (sources ~/.zshrc → nvm/fnm/etc.).
fn which_login(bin: &str) -> Option<String> {
    let out = Command::new("/bin/zsh")
        .env("TERM", "xterm-256color")
        .args(["-ilc", &format!("command -v {bin}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().map(|l| l.trim()).find(|l| l.starts_with('/')).map(|l| l.to_string())
}

fn valid_permission_mode(mode: &str) -> &str {
    match mode {
        "default" | "acceptEdits" | "plan" | "bypassPermissions" => mode,
        _ => "acceptEdits",
    }
}

fn valid_effort(e: &str) -> Option<&str> {
    match e {
        "low" | "medium" | "high" | "xhigh" | "max" => Some(e),
        _ => None,
    }
}

/// Read the `model:` value from an agent file's YAML frontmatter, if present.
fn read_agent_model(path: &Path) -> Option<String> {
    let txt = fs::read_to_string(path).ok()?;
    let mut in_fm = false;
    for line in txt.lines() {
        let t = line.trim();
        if t == "---" {
            if in_fm {
                break;
            }
            in_fm = true;
            continue;
        }
        if in_fm {
            if let Some(rest) = t.strip_prefix("model:") {
                let v = rest.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Turn one stream-json line into a (kind, human summary) pair for the log pane.
fn classify(line: &str) -> (String, String) {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return ("text".into(), line.to_string()),
    };
    let t = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    match t {
        "system" => {
            let sub = v.get("subtype").and_then(|x| x.as_str()).unwrap_or("system");
            ("system".into(), sub.to_string())
        }
        "assistant" => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                for block in content {
                    match block.get("type").and_then(|x| x.as_str()) {
                        Some("text") => {
                            if let Some(txt) = block.get("text").and_then(|x| x.as_str()) {
                                let txt = txt.trim();
                                if !txt.is_empty() {
                                    parts.push(txt.to_string());
                                }
                            }
                        }
                        Some("tool_use") => {
                            let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
                            if let Some(sub) =
                                block.pointer("/input/subagent_type").and_then(|x| x.as_str())
                            {
                                parts.push(format!("⟶ delegate to {sub}"));
                            } else {
                                parts.push(format!("⚙ {name}"));
                            }
                        }
                        _ => {}
                    }
                }
            }
            ("assistant".into(), parts.join("   ·   "))
        }
        "result" => {
            let res = v.get("result").and_then(|x| x.as_str()).unwrap_or("");
            ("result".into(), res.to_string())
        }
        other => (other.to_string(), String::new()),
    }
}

/// Parse the verdict from the `VERDICT:` line in review.md. Only the text *after*
/// `VERDICT:` on that line is inspected — scanning the whole document would falsely
/// match words like "blocking" or "needs" that appear in the review's prose.
fn read_verdict(project: &str) -> Option<String> {
    let txt = fs::read_to_string(Path::new(project).join(".pipeline/review.md")).ok()?;
    for line in txt.lines() {
        let upper = line.to_uppercase();
        let Some(idx) = upper.find("VERDICT:") else {
            continue;
        };
        let value = &upper[idx + "VERDICT:".len()..];
        for keyword in ["NEEDS WORK", "NEEDS-WORK", "BLOCK", "SHIP"] {
            if value.contains(keyword) {
                let canonical = if keyword.starts_with("NEEDS") { "NEEDS WORK" } else { keyword };
                return Some(canonical.to_string());
            }
        }
    }
    None
}

/// Write the agent + command + pipeline scaffold into a root directory.
fn install_assets(root: &Path, force: bool) -> Result<Vec<FileResult>, String> {
    let mut files = Vec::new();
    for a in assets() {
        let dest = root.join(a.rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let exists = dest.exists();
        if exists && !force {
            files.push(FileResult { path: a.rel.into(), action: "skipped".into() });
        } else {
            fs::write(&dest, a.contents).map_err(|e| e.to_string())?;
            files.push(FileResult {
                path: a.rel.into(),
                action: if exists { "updated" } else { "created" }.into(),
            });
        }
    }
    fs::create_dir_all(root.join(".pipeline")).map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join(".claude/skills")).map_err(|e| e.to_string())?;
    Ok(files)
}

/// Copy the repo's own .claude config (agents + /ship) into a worktree, preserving any
/// per-agent model edits. Falls back to the embedded default for any file the repo lacks.
fn copy_claude_config(from: &Path, to: &Path) -> Result<(), String> {
    for a in assets() {
        let src = from.join(a.rel);
        let dst = to.join(a.rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if src.exists() {
            fs::copy(&src, &dst).map_err(|e| e.to_string())?;
        } else {
            fs::write(&dst, a.contents).map_err(|e| e.to_string())?;
        }
    }
    fs::create_dir_all(to.join(".pipeline")).map_err(|e| e.to_string())?;
    Ok(())
}

/// Run a git command in `repo`, returning stdout (trimmed) or the stderr as an error.
fn git(repo: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git failed to launch: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Slug for branch/worktree names: lowercase alphanumerics, single dashes, max 40 chars.
fn sanitize_slug(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').chars().take(40).collect()
}

// --- Commands ---

/// (1) Folder management: install the scaffold into a repo.
#[tauri::command]
fn init_pipeline(project: String, force: bool) -> Result<InitResult, String> {
    let root = PathBuf::from(&project);
    if !root.is_dir() {
        return Err(format!("Not a directory: {project}"));
    }
    let files = install_assets(&root, force)?;
    Ok(InitResult { project, files })
}

/// (2) Report which agents/command/handoffs exist for a project, with each agent's model.
#[tauri::command]
fn pipeline_status(project: String) -> Result<PipelineStatus, String> {
    let root = PathBuf::from(&project);
    let mut agents = Vec::new();
    let mut all_present = true;
    for agent in ALL_AGENTS {
        let p = root.join(format!(".claude/agents/{agent}.md"));
        let present = p.exists();
        if AGENTS.contains(&agent) && !present {
            all_present = false; // default-mode readiness keys off the core four only
        }
        agents.push(AgentInfo {
            name: agent.to_string(),
            present,
            model: if present { read_agent_model(&p) } else { None },
        });
    }
    let has_ship_command = root.join(".claude/commands/ship.md").exists();
    let handoffs = STAGES
        .iter()
        .map(|(_, file)| {
            let p = root.join(".pipeline").join(file);
            let exists = p.exists();
            HandoffFile {
                name: file.to_string(),
                exists,
                size: if exists { fs::metadata(&p).map(|m| m.len()).unwrap_or(0) } else { 0 },
                modified_ms: if exists { modified_ms(&p) } else { None },
            }
        })
        .collect();
    Ok(PipelineStatus {
        initialized: all_present && has_ship_command,
        agents,
        has_ship_command,
        handoffs,
    })
}

/// (2) Set the model for one agent by rewriting its frontmatter `model:` line.
#[tauri::command]
fn set_agent_model(project: String, agent: String, model: String) -> Result<(), String> {
    if !ALL_AGENTS.contains(&agent.as_str()) {
        return Err(format!("unknown agent: {agent}"));
    }
    if !ALLOWED_MODELS.contains(&model.as_str()) {
        return Err(format!("unsupported model: {model}"));
    }
    let path = PathBuf::from(&project).join(format!(".claude/agents/{agent}.md"));
    let txt = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut out: Vec<String> = Vec::new();
    let mut in_fm = false;
    let mut replaced = false;
    for line in txt.lines() {
        let t = line.trim();
        if t == "---" {
            in_fm = !in_fm;
            out.push(line.to_string());
            continue;
        }
        if in_fm && !replaced && t.starts_with("model:") {
            out.push(format!("model: {model}"));
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !replaced {
        return Err("no `model:` field in agent frontmatter".into());
    }
    let mut content = out.join("\n");
    if txt.ends_with('\n') {
        content.push('\n');
    }
    fs::write(&path, content).map_err(|e| e.to_string())
}

/// (Config) Read a full agent prompt file for in-app editing.
#[tauri::command]
fn read_agent_file(project: String, agent: String) -> Result<String, String> {
    if !ALL_AGENTS.contains(&agent.as_str()) {
        return Err(format!("unknown agent: {agent}"));
    }
    let p = PathBuf::from(&project).join(format!(".claude/agents/{agent}.md"));
    fs::read_to_string(&p).map_err(|e| e.to_string())
}

/// (Config) Write an edited agent prompt file back to the repo.
#[tauri::command]
fn write_agent_file(project: String, agent: String, content: String) -> Result<(), String> {
    if !ALL_AGENTS.contains(&agent.as_str()) {
        return Err(format!("unknown agent: {agent}"));
    }
    let p = PathBuf::from(&project).join(format!(".claude/agents/{agent}.md"));
    fs::write(&p, content).map_err(|e| e.to_string())
}

/// (Config) Preflight checks for a project: claude CLI, node, git repo, pipeline installed.
#[tauri::command]
fn doctor(project: String) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    let claude = resolve_claude();
    let claude_ok = Path::new(&claude).exists();
    checks.push(DoctorCheck {
        name: "Claude CLI".into(),
        ok: claude_ok,
        detail: if claude_ok { claude } else { "not found — install / log in to Claude Code".into() },
    });

    match which_login("node") {
        Some(p) => checks.push(DoctorCheck { name: "Node.js".into(), ok: true, detail: p }),
        None => checks.push(DoctorCheck { name: "Node.js".into(), ok: false, detail: "not found on PATH".into() }),
    }

    let is_repo = git(&project, &["rev-parse", "--git-dir"]).is_ok();
    checks.push(DoctorCheck {
        name: "Git repo".into(),
        ok: is_repo,
        detail: if is_repo { "folder is a git repository".into() } else { "not a git repo — overnight & Shipper need git".into() },
    });

    let root = PathBuf::from(&project);
    let agents_ok = ALL_AGENTS.iter().all(|a| root.join(format!(".claude/agents/{a}.md")).exists())
        && root.join(".claude/commands/ship.md").exists();
    checks.push(DoctorCheck {
        name: "Pipeline installed".into(),
        ok: agents_ok,
        detail: if agents_ok { "agents + /ship present".into() } else { "click 'Install agents into repo'".into() },
    });

    checks
}

/// (2) Read one handoff file's contents for the viewer. Path-traversal guarded.
#[tauri::command]
fn read_handoff(project: String, name: String) -> Result<String, String> {
    if name.contains('/') || name.contains("..") {
        return Err("invalid handoff name".into());
    }
    let p = PathBuf::from(&project).join(".pipeline").join(&name);
    fs::read_to_string(&p).map_err(|e| e.to_string())
}

/// (2) Delete the handoff files so a new run starts clean.
#[tauri::command]
fn clean_pipeline(project: String) -> Result<(), String> {
    let dir = PathBuf::from(&project).join(".pipeline");
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().is_file() {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

// --- Skills: reusable Claude Code skills under .claude/skills/<name>/SKILL.md ---

const SKILL_TEMPLATE: &str = "---\nname: {name}\ndescription: What this skill does and WHEN Claude should use it — this line is the trigger Claude reads to decide whether to load the skill, so make it specific. e.g. \"Use when the user wants to … ; covers … .\"\n---\n\n# {name}\n\n## When to use\nThe situations, file types, or requests that should invoke this skill — mirror the trigger you wrote in the description above.\n\n## Instructions\nThe concrete steps, conventions, or checklist Claude should follow when this skill applies. Keep it focused — one skill, one job.\n";

fn valid_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("invalid skill name".into());
    }
    Ok(())
}

/// Best-effort read of a skill's `description:` from its SKILL.md frontmatter.
fn read_skill_description(path: &Path) -> String {
    let Ok(txt) = fs::read_to_string(path) else {
        return String::new();
    };
    let mut in_fm = false;
    for line in txt.lines() {
        let t = line.trim();
        if t == "---" {
            if in_fm {
                break;
            }
            in_fm = true;
            continue;
        }
        if in_fm {
            if let Some(rest) = t.strip_prefix("description:") {
                return rest.trim().trim_matches('"').to_string();
            }
        }
    }
    String::new()
}

/// (Skills) List the skills installed in a repo's `.claude/skills/`.
#[tauri::command]
fn list_skills(project: String) -> Result<Vec<SkillInfo>, String> {
    let dir = PathBuf::from(&project).join(".claude/skills");
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.path().is_dir() {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        out.push(SkillInfo { description: read_skill_description(&skill_md), name });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// (Skills) Read a skill's SKILL.md for editing.
#[tauri::command]
fn read_skill(project: String, name: String) -> Result<String, String> {
    valid_skill_name(&name)?;
    let p = PathBuf::from(&project).join(".claude/skills").join(&name).join("SKILL.md");
    fs::read_to_string(&p).map_err(|e| e.to_string())
}

/// (Skills) Write an edited SKILL.md back (creates the folder if needed).
#[tauri::command]
fn write_skill(project: String, name: String, content: String) -> Result<(), String> {
    valid_skill_name(&name)?;
    let dir = PathBuf::from(&project).join(".claude/skills").join(&name);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("SKILL.md"), content).map_err(|e| e.to_string())
}

/// (Skills) Create a new skill from the template. Errors if it already exists.
#[tauri::command]
fn create_skill(project: String, name: String) -> Result<(), String> {
    valid_skill_name(&name)?;
    let dir = PathBuf::from(&project).join(".claude/skills").join(&name);
    let skill_md = dir.join("SKILL.md");
    if skill_md.exists() {
        return Err(format!("skill '{name}' already exists"));
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(skill_md, SKILL_TEMPLATE.replace("{name}", &name)).map_err(|e| e.to_string())
}

/// (Skills) Delete a skill folder and its contents.
#[tauri::command]
fn delete_skill(project: String, name: String) -> Result<(), String> {
    valid_skill_name(&name)?;
    let dir = PathBuf::from(&project).join(".claude/skills").join(&name);
    if !dir.is_dir() {
        return Ok(());
    }
    fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}

/// (4) Create an isolated git worktree + branch for an overnight run, and install the
/// agent scaffold into it (a fresh worktree won't contain untracked .claude/agents).
#[tauri::command]
fn create_worktree(repo: String, slug: String) -> Result<WorktreeInfo, String> {
    git(&repo, &["rev-parse", "--git-dir"]).map_err(|_| "not a git repository".to_string())?;
    let slug = sanitize_slug(&slug);
    if slug.is_empty() {
        return Err("empty slug".into());
    }
    let repo_path = PathBuf::from(&repo);
    let repo_name = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .to_string();
    let parent = repo_path.parent().ok_or("repo has no parent directory")?;
    let wt_root = parent.join(".foreman-worktrees");
    fs::create_dir_all(&wt_root).map_err(|e| e.to_string())?;
    let wt_path = wt_root.join(format!("{repo_name}-{slug}"));
    let wt_path_str = wt_path.to_string_lossy().to_string();
    let branch = format!("foreman/{slug}");

    // Reuse a leftover worktree at our (deterministic, slug-keyed) path — e.g. a prior run
    // that created the worktree then failed early. Reset tracked files to HEAD (a no-op for
    // an early fail), reinstall agents, and clear the handoff folder for a fresh start.
    if wt_path.join(".git").exists() {
        let _ = git(&wt_path_str, &["reset", "--hard", "HEAD"]);
        copy_claude_config(&repo_path, &wt_path)?;
        let _ = clean_pipeline(wt_path_str.clone());
        return Ok(WorktreeInfo { path: wt_path_str, branch });
    }
    // A stale, non-worktree directory at our path would block `worktree add` — clear it.
    if wt_path.exists() {
        fs::remove_dir_all(&wt_path).map_err(|e| e.to_string())?;
    }
    git(&repo, &["worktree", "prune"]).ok();

    // The branch may already exist (created by a prior attempt). Reuse it only if it is
    // "empty" — its tip is still the base HEAD, i.e. no committed work to clobber.
    let branch_ref = format!("refs/heads/{branch}");
    if git(&repo, &["rev-parse", "--verify", "--quiet", &branch_ref]).is_ok() {
        let head = git(&repo, &["rev-parse", "HEAD"])?;
        let tip = git(&repo, &["rev-parse", &branch])?;
        if head != tip {
            return Err(format!(
                "branch '{branch}' already exists with committed work — remove it or rename the feature"
            ));
        }
        git(&repo, &["worktree", "add", &wt_path_str, &branch])?;
    } else {
        git(&repo, &["worktree", "add", "-b", &branch, &wt_path_str, "HEAD"])?;
    }
    copy_claude_config(&repo_path, &wt_path)?;
    Ok(WorktreeInfo { path: wt_path_str, branch })
}

/// (4) Remove a worktree (used by the "discard" action). The branch is kept.
#[tauri::command]
fn remove_worktree(repo: String, path: String, branch: Option<String>) -> Result<(), String> {
    git(&repo, &["worktree", "remove", &path, "--force"])?;
    if let Some(b) = branch {
        // Safe-delete: kept automatically if it has unmerged commits worth preserving.
        let _ = git(&repo, &["branch", "-d", &b]);
    }
    Ok(())
}

/// (3) Run the pipeline in `project`, keyed by `run_id`. Returns immediately; progress
/// arrives via run-tagged "pipeline-log" / "-stage" / "-usage" / "-done" events.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn run_pipeline(
    app: AppHandle,
    state: State<RunState>,
    run_id: String,
    project: String,
    request: String,
    permission_mode: String,
    effort: String,
    autonomous: bool,
    resume: Option<String>,
    clean_first: bool,
    fast: Option<bool>,
) -> Result<(), String> {
    let root = PathBuf::from(&project);
    if !root.is_dir() {
        return Err(format!("Not a directory: {project}"));
    }
    if state.children.lock().unwrap().contains_key(&run_id) {
        return Err(format!("run '{run_id}' already in progress"));
    }
    if clean_first {
        clean_pipeline(project.clone())?;
    }
    fs::create_dir_all(root.join(".pipeline")).map_err(|e| e.to_string())?;

    let claude = resolve_claude();
    let mode = valid_permission_mode(&permission_mode).to_string();
    let command = if autonomous {
        "ship-auto"
    } else if fast.unwrap_or(false) {
        "ship-fast"
    } else {
        "ship"
    };
    // Fresh run → `/ship[-auto] <request>`; resumed run → send the human's answer verbatim.
    let prompt = match &resume {
        Some(_) => request.clone(),
        None => format!("/{command} {request}"),
    };

    let mut cmd = Command::new(&claude);
    cmd.current_dir(&root)
        .arg("-p")
        .arg(&prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--permission-mode")
        .arg(&mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(level) = valid_effort(&effort) {
        cmd.arg("--effort").arg(level);
    }
    if let Some(sid) = &resume {
        cmd.arg("--resume").arg(sid);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch claude ({claude}): {e}"))?;

    // Reader: stdout (stream-json events) — also tallies token usage.
    if let Some(stdout) = child.stdout.take() {
        let app = app.clone();
        let rid = run_id.clone();
        thread::spawn(move || {
            let (mut it, mut ot, mut cr, mut cc) = (0u64, 0u64, 0u64, 0u64);
            let mut sent_session = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if !sent_session {
                        if let Some(sid) = v.get("session_id").and_then(|x| x.as_str()) {
                            let _ = app.emit(
                                "pipeline-session",
                                SessionEvent { run_id: rid.clone(), session_id: sid.to_string() },
                            );
                            sent_session = true;
                        }
                    }
                    match v.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                        "result" => {
                            let u = v.get("usage");
                            let field = |k: &str| {
                                u.and_then(|u| u.get(k))
                                    .or_else(|| v.get(k))
                                    .and_then(|x| x.as_u64())
                                    .unwrap_or(0)
                            };
                            let _ = app.emit(
                                "pipeline-usage",
                                UsageEvent {
                                    run_id: rid.clone(),
                                    input_tokens: field("input_tokens"),
                                    output_tokens: field("output_tokens"),
                                    cache_read: field("cache_read_input_tokens"),
                                    cache_creation: field("cache_creation_input_tokens"),
                                    is_final: true,
                                },
                            );
                        }
                        "assistant" => {
                            // A delegation (Task tool_use with subagent_type) is the accurate
                            // moment that agent actually starts — mark it "running" now.
                            if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                                for block in content {
                                    if block.get("type").and_then(|x| x.as_str()) == Some("tool_use") {
                                        if let Some(sub) =
                                            block.pointer("/input/subagent_type").and_then(|x| x.as_str())
                                        {
                                            if ALL_AGENTS.contains(&sub) {
                                                let _ = app.emit(
                                                    "pipeline-stage",
                                                    StageEvent {
                                                        run_id: rid.clone(),
                                                        agent: sub.to_string(),
                                                        file: String::new(),
                                                        phase: "running".into(),
                                                    },
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(u) = v.pointer("/message/usage") {
                                let field = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
                                it += field("input_tokens");
                                ot += field("output_tokens");
                                cr += field("cache_read_input_tokens");
                                cc += field("cache_creation_input_tokens");
                                let _ = app.emit(
                                    "pipeline-usage",
                                    UsageEvent {
                                        run_id: rid.clone(),
                                        input_tokens: it,
                                        output_tokens: ot,
                                        cache_read: cr,
                                        cache_creation: cc,
                                        is_final: false,
                                    },
                                );
                            }
                        }
                        _ => {}
                    }
                }
                let (kind, text) = classify(&line);
                let _ = app.emit(
                    "pipeline-log",
                    LogEvent { run_id: rid.clone(), kind, text, raw: line },
                );
            }
        });
    }

    // Reader: stderr (warnings / errors).
    if let Some(stderr) = child.stderr.take() {
        let app = app.clone();
        let rid = run_id.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                let _ = app.emit(
                    "pipeline-log",
                    LogEvent { run_id: rid.clone(), kind: "stderr".into(), text: line.clone(), raw: line },
                );
            }
        });
    }

    state.children.lock().unwrap().insert(run_id.clone(), child);

    // Watcher: poll for stage handoff files + process completion.
    let children = state.children.clone();
    let watch_app = app.clone();
    let watch_project = project.clone();
    let rid = run_id.clone();
    thread::spawn(move || {
        let mut seen = [false; 4];
        let emit_stage = |i: usize| {
            let (agent, file) = STAGES[i];
            let _ = watch_app.emit(
                "pipeline-stage",
                StageEvent {
                    run_id: rid.clone(),
                    agent: agent.to_string(),
                    file: file.to_string(),
                    phase: "done".into(),
                },
            );
        };
        loop {
            for (i, (_, file)) in STAGES.iter().enumerate() {
                if !seen[i] && Path::new(&watch_project).join(".pipeline").join(file).exists() {
                    seen[i] = true;
                    emit_stage(i);
                }
            }

            let mut map = children.lock().unwrap();
            let finished = match map.get_mut(&rid) {
                Some(c) => match c.try_wait() {
                    Ok(Some(status)) => Some(status.code()),
                    Ok(None) => None,
                    Err(_) => Some(None),
                },
                None => Some(None), // cancelled / taken
            };
            if let Some(code) = finished {
                map.remove(&rid);
                drop(map);
                for (i, (_, file)) in STAGES.iter().enumerate() {
                    if !seen[i] && Path::new(&watch_project).join(".pipeline").join(file).exists() {
                        emit_stage(i);
                    }
                }
                let verdict = read_verdict(&watch_project);
                let _ = watch_app.emit("pipeline-done", DoneEvent { run_id: rid.clone(), code, verdict });
                break;
            }
            drop(map);
            thread::sleep(Duration::from_millis(350));
        }
    });

    Ok(())
}

/// (3/4) Kill one in-progress run.
#[tauri::command]
fn cancel_run(state: State<RunState>, run_id: String) -> Result<(), String> {
    if let Some(mut child) = state.children.lock().unwrap().remove(&run_id) {
        let _ = child.kill();
    }
    Ok(())
}

/// (4) Kill every in-progress run.
#[tauri::command]
fn cancel_all(state: State<RunState>) -> Result<(), String> {
    let mut map = state.children.lock().unwrap();
    for (_, mut child) in map.drain() {
        let _ = child.kill();
    }
    Ok(())
}

// --- (5) The Shipper: an independent, promptable agent that acts on the pipeline's output. ---

const SHIPPER_PREAMBLE: &str = r#"You are the Shipper — the release/deploy assistant for this repository. A four-agent pipeline (planner → coder → tester → reviewer) has just produced changes in THIS working tree.

Orient yourself before acting:
- Read `.pipeline/review.md` for the Reviewer's VERDICT (SHIP / NEEDS WORK / BLOCK) and findings.
- Read `.pipeline/changes.md` for what the Coder changed, and run `git status` / `git diff` to see the actual uncommitted changes.

Then carry out the human's instruction below — e.g. commit, push, open a PR (use `gh`), merge, tag, or deploy. Rules:
- If the verdict is not SHIP, point that out first; still follow an explicit instruction.
- Never force-push or rewrite/delete history unless explicitly told to.
- When done, report exactly what you did: the commands you ran, the branch, the remote, and any PR or commit URL."#;

/// (5) Run the Shipper agent (locked to sonnet / medium effort) on the human's instruction.
/// Keyed by "shipper" in the run map. Pass `resume` (a session id) to continue the chat.
#[tauri::command]
fn ship_agent(
    app: AppHandle,
    state: State<RunState>,
    project: String,
    prompt: String,
    resume: Option<String>,
) -> Result<(), String> {
    let root = PathBuf::from(&project);
    if !root.is_dir() {
        return Err(format!("Not a directory: {project}"));
    }
    if state.children.lock().unwrap().contains_key("shipper") {
        return Err("the shipper is already working".into());
    }
    let claude = resolve_claude();
    // First message carries the role + context preamble; resumed turns just send the text.
    let full_prompt = if resume.is_some() {
        prompt.clone()
    } else {
        format!("{SHIPPER_PREAMBLE}\n\nInstruction: {prompt}")
    };

    let mut cmd = Command::new(&claude);
    cmd.current_dir(&root)
        .arg("-p")
        .arg(&full_prompt)
        .arg("--model")
        .arg("sonnet")
        .arg("--effort")
        .arg("medium")
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(sid) = &resume {
        cmd.arg("--resume").arg(sid);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch claude ({claude}): {e}"))?;

    if let Some(stdout) = child.stdout.take() {
        let app = app.clone();
        thread::spawn(move || {
            let mut sent_session = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if !sent_session {
                    if let Some(sid) = v.get("session_id").and_then(|x| x.as_str()) {
                        let _ = app.emit("shipper-session", sid.to_string());
                        sent_session = true;
                    }
                }
                match v.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                    "assistant" => {
                        if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                            for block in content {
                                match block.get("type").and_then(|x| x.as_str()) {
                                    Some("text") => {
                                        if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                                            let t = t.trim();
                                            if !t.is_empty() {
                                                let _ = app.emit(
                                                    "shipper-log",
                                                    ShipperEvent { kind: "assistant".into(), text: t.to_string() },
                                                );
                                            }
                                        }
                                    }
                                    Some("tool_use") => {
                                        let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
                                        let _ = app.emit(
                                            "shipper-log",
                                            ShipperEvent { kind: "tool".into(), text: format!("⚙ {name}") },
                                        );
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    "result" => {
                        if let Some(r) = v.get("result").and_then(|x| x.as_str()) {
                            let _ = app.emit("shipper-log", ShipperEvent { kind: "result".into(), text: r.to_string() });
                        }
                    }
                    _ => {}
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let app = app.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    let _ = app.emit("shipper-log", ShipperEvent { kind: "stderr".into(), text: line });
                }
            }
        });
    }

    state.children.lock().unwrap().insert("shipper".into(), child);

    let children = state.children.clone();
    let done_app = app.clone();
    thread::spawn(move || loop {
        let mut map = children.lock().unwrap();
        let finished = match map.get_mut("shipper") {
            Some(c) => matches!(c.try_wait(), Ok(Some(_)) | Err(_)),
            None => true,
        };
        if finished {
            map.remove("shipper");
            drop(map);
            let _ = done_app.emit("shipper-done", ());
            break;
        }
        drop(map);
        thread::sleep(Duration::from_millis(300));
    });

    Ok(())
}

// --- Skill Builder: a promptable agent that authors complete skills into .claude/skills/. ---

const SKILL_BUILDER_PREAMBLE: &str = r#"You are the Skill Builder for this repository — a specialist that authors complete, valid Claude Code skills.

A skill lives at `.claude/skills/<skill-name>/SKILL.md`, where `<skill-name>` is a short lowercase-hyphen slug. SKILL.md must have YAML frontmatter with at least:
- `name:` — exactly the folder slug.
- `description:` — ONE specific, trigger-oriented line: what the skill does AND when Claude should use it. This is the text Claude reads to decide whether to load the skill, so make it concrete (start with "Use when ...").
...plus a markdown body with the real instructions: a "When to use" section and concrete steps / conventions / checklists.

When the human asks you to add or change a skill:
1. Choose a clear slug, then create `.claude/skills/<slug>/SKILL.md` with correct frontmatter (name = the slug) and a focused, well-structured body.
2. If the skill genuinely needs supporting files — helper scripts, reference docs, templates/assets — create them under that skill's own folder (`scripts/`, `references/`, `assets/`) and reference them from SKILL.md by their real relative paths. Keep scripts dependency-light (standard library only) and actually runnable. Never reference a file you did not create.
3. Keep it focused: one skill, one job. Don't over-engineer.
4. Work ONLY inside `.claude/skills/`. Do not modify anything else in the repository, and never commit, push, or run destructive commands.
5. When you finish, report the skill name, every file you created, and a one-line summary of what the skill does and when it triggers."#;

/// Run the Skill Builder agent (sonnet / medium, bypassPermissions) on the human's request.
/// Keyed by "skill-builder" in the run map; writes a full skill folder under .claude/skills/.
#[tauri::command]
fn build_skill(app: AppHandle, state: State<RunState>, project: String, prompt: String) -> Result<(), String> {
    let root = PathBuf::from(&project);
    if !root.is_dir() {
        return Err(format!("Not a directory: {project}"));
    }
    if state.children.lock().unwrap().contains_key("skill-builder") {
        return Err("the skill builder is already working".into());
    }
    fs::create_dir_all(root.join(".claude/skills")).map_err(|e| e.to_string())?;
    let claude = resolve_claude();
    let full_prompt = format!("{SKILL_BUILDER_PREAMBLE}\n\nRequest: {prompt}");

    let mut cmd = Command::new(&claude);
    cmd.current_dir(&root)
        .arg("-p")
        .arg(&full_prompt)
        .arg("--model")
        .arg("sonnet")
        .arg("--effort")
        .arg("medium")
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to launch claude ({claude}): {e}"))?;

    if let Some(stdout) = child.stdout.take() {
        let app = app.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match v.get("type").and_then(|x| x.as_str()).unwrap_or("") {
                    "assistant" => {
                        if let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                            for block in content {
                                match block.get("type").and_then(|x| x.as_str()) {
                                    Some("text") => {
                                        if let Some(t) = block.get("text").and_then(|x| x.as_str()) {
                                            let t = t.trim();
                                            if !t.is_empty() {
                                                let _ = app.emit(
                                                    "skillbuild-log",
                                                    ShipperEvent { kind: "assistant".into(), text: t.to_string() },
                                                );
                                            }
                                        }
                                    }
                                    Some("tool_use") => {
                                        let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
                                        let _ = app.emit(
                                            "skillbuild-log",
                                            ShipperEvent { kind: "tool".into(), text: format!("⚙ {name}") },
                                        );
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    "result" => {
                        if let Some(r) = v.get("result").and_then(|x| x.as_str()) {
                            let _ = app.emit("skillbuild-log", ShipperEvent { kind: "result".into(), text: r.to_string() });
                        }
                    }
                    _ => {}
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let app = app.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    let _ = app.emit("skillbuild-log", ShipperEvent { kind: "stderr".into(), text: line });
                }
            }
        });
    }

    state.children.lock().unwrap().insert("skill-builder".into(), child);

    let children = state.children.clone();
    let done_app = app.clone();
    thread::spawn(move || loop {
        let mut map = children.lock().unwrap();
        let finished = match map.get_mut("skill-builder") {
            Some(c) => matches!(c.try_wait(), Ok(Some(_)) | Err(_)),
            None => true,
        };
        if finished {
            map.remove("skill-builder");
            drop(map);
            let _ = done_app.emit("skillbuild-done", ());
            break;
        }
        drop(map);
        thread::sleep(Duration::from_millis(300));
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(RunState::default())
        .setup(|app| {
            // Menu-bar tray: Open / Quit.
            let show = MenuItem::with_id(app, "show", "Open Foreman", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Foreman", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init_pipeline,
            pipeline_status,
            set_agent_model,
            read_agent_file,
            write_agent_file,
            doctor,
            read_handoff,
            clean_pipeline,
            create_worktree,
            remove_worktree,
            run_pipeline,
            cancel_run,
            cancel_all,
            ship_agent,
            list_skills,
            read_skill,
            write_skill,
            create_skill,
            delete_skill,
            build_skill
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
