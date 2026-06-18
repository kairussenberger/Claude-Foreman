// Foreman — a cockpit for the four-agent Claude Code pipeline.
//
// The Rust side does three jobs (the v1 priorities):
//   1. Folder management — install the .claude/agents + .claude/commands + .pipeline scaffold into a target repo.
//   2. Handoff files     — read/list/clean the .pipeline/*.md files the agents hand off through.
//   3. Pipeline          — spawn `claude -p "/ship ..."` headless, stream its output, and track stage progress.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

// --- The pipeline assets, embedded in the binary. Installed into target repos on init. ---

const PLANNER_MD: &str = include_str!("../templates/planner.md");
const CODER_MD: &str = include_str!("../templates/coder.md");
const TESTER_MD: &str = include_str!("../templates/tester.md");
const REVIEWER_MD: &str = include_str!("../templates/reviewer.md");
const SHIP_MD: &str = include_str!("../templates/ship.md");

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
        Asset { rel: ".claude/commands/ship.md", contents: SHIP_MD },
    ]
}

// The four handoff files, in pipeline order, each tied to the agent that produces it.
const STAGES: [(&str, &str); 4] = [
    ("planner", "spec.md"),
    ("coder", "changes.md"),
    ("tester", "test-results.md"),
    ("reviewer", "review.md"),
];

// --- Shared state: the currently running child process, so we can cancel it. ---

#[derive(Default)]
struct RunState {
    child: Arc<Mutex<Option<Child>>>,
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
struct PipelineStatus {
    initialized: bool,
    agents_present: Vec<String>,
    agents_missing: Vec<String>,
    has_ship_command: bool,
    handoffs: Vec<HandoffFile>,
}

#[derive(Clone, Serialize)]
struct LogEvent {
    kind: String,
    text: String,
    raw: String,
}

#[derive(Clone, Serialize)]
struct StageEvent {
    agent: String,
    file: String,
}

#[derive(Clone, Serialize)]
struct DoneEvent {
    code: Option<i32>,
    verdict: Option<String>,
}

// --- Helpers ---

fn modified_ms(path: &Path) -> Option<u64> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let dur = mtime.duration_since(UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as u64)
}

/// Resolve the absolute path to the `claude` binary through a login shell, so the app
/// finds it even when launched from Finder (where PATH is minimal). Falls back to "claude".
fn resolve_claude() -> String {
    if let Ok(out) = Command::new("/bin/zsh")
        .args(["-lc", "command -v claude"])
        .output()
    {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return p;
            }
        }
    }
    "claude".to_string()
}

fn valid_permission_mode(mode: &str) -> &str {
    match mode {
        "default" | "acceptEdits" | "plan" | "bypassPermissions" => mode,
        _ => "acceptEdits",
    }
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
                            if let Some(sub) = block
                                .pointer("/input/subagent_type")
                                .and_then(|x| x.as_str())
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
            let cost = v
                .get("total_cost_usd")
                .and_then(|x| x.as_f64())
                .map(|c| format!("  (${c:.4})"))
                .unwrap_or_default();
            ("result".into(), format!("{res}{cost}"))
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

// --- Commands ---

/// (1) Folder management: install the agent + command + pipeline scaffold into a repo.
#[tauri::command]
fn init_pipeline(project: String, force: bool) -> Result<InitResult, String> {
    let root = PathBuf::from(&project);
    if !root.is_dir() {
        return Err(format!("Not a directory: {project}"));
    }
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
    Ok(InitResult { project, files })
}

/// (2) Report which agents/command/handoffs exist for a project.
#[tauri::command]
fn pipeline_status(project: String) -> Result<PipelineStatus, String> {
    let root = PathBuf::from(&project);
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for agent in ["planner", "coder", "tester", "reviewer"] {
        let p = root.join(format!(".claude/agents/{agent}.md"));
        if p.exists() {
            present.push(agent.to_string());
        } else {
            missing.push(agent.to_string());
        }
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
                size: if exists {
                    fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
                } else {
                    0
                },
                modified_ms: if exists { modified_ms(&p) } else { None },
            }
        })
        .collect();
    Ok(PipelineStatus {
        initialized: missing.is_empty() && has_ship_command,
        agents_present: present,
        agents_missing: missing,
        has_ship_command,
        handoffs,
    })
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

/// (3) Run the pipeline: spawn `claude -p "/ship ..."` headless and stream events.
/// Returns immediately; progress arrives via "pipeline-log", "pipeline-stage",
/// "pipeline-done", and "pipeline-error" events.
#[tauri::command]
fn run_pipeline(
    app: AppHandle,
    state: State<RunState>,
    project: String,
    request: String,
    permission_mode: String,
    clean_first: bool,
) -> Result<(), String> {
    let root = PathBuf::from(&project);
    if !root.is_dir() {
        return Err(format!("Not a directory: {project}"));
    }
    if state.child.lock().unwrap().is_some() {
        return Err("a pipeline run is already in progress".into());
    }
    if clean_first {
        clean_pipeline(project.clone())?;
    }
    fs::create_dir_all(root.join(".pipeline")).map_err(|e| e.to_string())?;

    let claude = resolve_claude();
    let mode = valid_permission_mode(&permission_mode).to_string();
    let prompt = format!("/ship {request}");

    let mut child = Command::new(&claude)
        .current_dir(&root)
        .arg("-p")
        .arg(&prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--permission-mode")
        .arg(&mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to launch claude ({claude}): {e}"))?;

    // Reader: stdout (stream-json events).
    if let Some(stdout) = child.stdout.take() {
        let app = app.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                let (kind, text) = classify(&line);
                let _ = app.emit("pipeline-log", LogEvent { kind, text, raw: line });
            }
        });
    }

    // Reader: stderr (warnings / errors).
    if let Some(stderr) = child.stderr.take() {
        let app = app.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.trim().is_empty() {
                    continue;
                }
                let _ = app.emit(
                    "pipeline-log",
                    LogEvent { kind: "stderr".into(), text: line.clone(), raw: line },
                );
            }
        });
    }

    *state.child.lock().unwrap() = Some(child);

    // Watcher: poll for stage handoff files + process completion.
    let child_slot = state.child.clone();
    let watch_app = app.clone();
    let watch_project = project.clone();
    thread::spawn(move || {
        let mut seen = [false; 4];
        loop {
            for (i, (agent, file)) in STAGES.iter().enumerate() {
                if !seen[i]
                    && Path::new(&watch_project)
                        .join(".pipeline")
                        .join(file)
                        .exists()
                {
                    seen[i] = true;
                    let _ = watch_app.emit(
                        "pipeline-stage",
                        StageEvent { agent: agent.to_string(), file: file.to_string() },
                    );
                }
            }

            let mut guard = child_slot.lock().unwrap();
            let finished = match guard.as_mut() {
                Some(c) => match c.try_wait() {
                    Ok(Some(status)) => Some(status.code()),
                    Ok(None) => None,
                    Err(_) => Some(None),
                },
                None => Some(None), // cancelled / taken
            };
            if let Some(code) = finished {
                *guard = None;
                drop(guard);
                // Final stage sweep in case files appeared at the very end.
                for (i, (agent, file)) in STAGES.iter().enumerate() {
                    if !seen[i]
                        && Path::new(&watch_project)
                            .join(".pipeline")
                            .join(file)
                            .exists()
                    {
                        let _ = watch_app.emit(
                            "pipeline-stage",
                            StageEvent { agent: agent.to_string(), file: file.to_string() },
                        );
                    }
                }
                let verdict = read_verdict(&watch_project);
                let _ = watch_app.emit("pipeline-done", DoneEvent { code, verdict });
                break;
            }
            drop(guard);
            thread::sleep(Duration::from_millis(350));
        }
    });

    Ok(())
}

/// (3) Kill the in-progress run.
#[tauri::command]
fn cancel_pipeline(state: State<RunState>) -> Result<(), String> {
    let mut guard = state.child.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(RunState::default())
        .invoke_handler(tauri::generate_handler![
            init_pipeline,
            pipeline_status,
            read_handoff,
            clean_pipeline,
            run_pipeline,
            cancel_pipeline
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
