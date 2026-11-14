//! The assistant panel's two connection modes.
//!
//! Installed coding agent: runs the agent's own unmodified executable in
//! non-interactive mode with only the board's MCP tools, using the agent's own
//! sign-in. The app never sees or stores account credentials.
//!
//! API key: calls the model API directly with the user's key from the system's
//! credential store, running the same board tools in-process.
//!
//! Either way, writes are staged and handed back to the app.

use crate::ops::Pending;
use crate::settings::{self, Settings};
use crate::store::Store;
use crate::tools::{tool_defs, Ctx};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The agent's executable name and the folder its desktop install keeps
/// versioned copies in. Only used to find the program; nothing is modified.
const AGENT_PROGRAM: &str = "claude";
const AGENT_DESKTOP_DIR: &[&str] = &["Claude", "claude-code"];
const MCP_NAME: &str = "board";
const RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub struct Outcome {
    pub reply: String,
    pub pending: Vec<Pending>,
    pub session: Option<String>,
}

pub fn system_prompt() -> String {
    let tracks = crate::model::TRACKS
        .iter()
        .map(|t| format!("- {} ({}): {}", t.label, t.key, t.stages.iter().map(|s| s.1).collect::<Vec<_>>().join(" > ")))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are the assistant built into Production Board, a desktop app where one person tracks everything they have in flight: novels, craft books and textbooks, products, software, teaching, research, department chair work, service work and social media chores. Today is {today}.\n\n\
Tracks and their stages, in order (the last stage of each track means finished):\n{tracks}\n\n\
Each item has: title, track, stage, kind, byline (pen name), words, status (where it stands), next (next step), open (decisions waiting on the owner), decisions (standing decisions that must not be reopened), files, links, dueDate (a due date, YYYY-MM-DD) and dueLabel (what the date is for), touched (date last worked on) and a short activity log.\n\n\
How to work:\n\
- Use the board tools to read the board before answering; never guess at item contents. Use board_summary for questions about what to pick up or what is urgent.\n\
- When asked to change something, make the change with the tools. Your changes are staged: the app applies them when you finish, and asks the owner to confirm removals and changes touching more than three items. Say plainly what you changed.\n\
- Moving an item stamps it as worked on today. Use log_activity to record work done.\n\
- Treat an item's standing decisions as settled. When an open decision is settled, resolve it and add the outcome as a standing decision.\n\
- Social media chores go in the social track, one item per post or chore, linked to the book in the title or kind.\n\
- Department chair work goes in the chair track. Never write personnel matters or anything identifying a student onto a card; the card says only that the work exists.\n\
- Keep replies short and plain: a few sentences or a short list. No headings. Refer to items by title.\n\
- You can only see and change the board. You cannot open manuscripts or other files.",
        today = crate::today()
    )
}

// ---------- finding the installed agent ----------

fn version_key(name: &str) -> Vec<u64> {
    name.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse().unwrap_or(0)).collect()
}

pub fn detect_agent(configured: &str) -> Option<PathBuf> {
    let c = configured.trim().trim_matches('"');
    if !c.is_empty() {
        let p = PathBuf::from(c);
        return p.is_file().then_some(p);
    }
    #[cfg(windows)]
    let names = [format!("{AGENT_PROGRAM}.exe"), format!("{AGENT_PROGRAM}.cmd")];
    #[cfg(not(windows))]
    let names = [AGENT_PROGRAM.to_string()];
    // A Mac app opened from the Dock doesn't get the shell's PATH, so the usual
    // install folders are checked as well.
    let mut dirs_to_check: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(not(windows)) {
        dirs_to_check.extend(["/opt/homebrew/bin", "/usr/local/bin"].iter().map(PathBuf::from));
    }
    {
        for dir in dirs_to_check {
            for n in &names {
                let p = dir.join(n);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local").join("bin").join(&names[0]);
        if p.is_file() {
            return Some(p);
        }
    }
    // The desktop app keeps the agent in <roaming>\<vendor>\<agent>\<version>\<build>\.
    #[cfg(windows)]
    if let Some(roaming) = dirs::config_dir() {
        let base = AGENT_DESKTOP_DIR.iter().fold(roaming, |p, s| p.join(s));
        if let Ok(rd) = std::fs::read_dir(&base) {
            let mut versions: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
            versions.sort_by_key(|p| version_key(&p.file_name().unwrap_or_default().to_string_lossy()));
            for v in versions.iter().rev() {
                if let Ok(builds) = std::fs::read_dir(v) {
                    for b in builds.filter_map(|e| e.ok()) {
                        let p = b.path().join(&names[0]);
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
        }
    }
    None
}

fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Run the agent with arguments and return its combined output.
pub fn run_agent_cmd(program: &Path, args: &[&str]) -> Result<String, String> {
    let out = command(program).args(args).stdin(Stdio::null()).output().map_err(|e| format!("Couldn't start {}: {e}", program.display()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(if text.is_empty() { format!("Exited with {}", out.status) } else { text })
    }
}

/// Whether the agent reports being signed in. None when it can't tell.
pub fn signed_in(program: &Path) -> Option<bool> {
    let out = command(program).args(["auth", "status", "--json"]).stdin(Stdio::null()).output().ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    v.get("loggedIn").and_then(Value::as_bool)
}

/// Open the agent's own sign-in in a window of its own. The app takes no
/// part in it and never sees the credentials.
pub fn open_sign_in(program: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Run the sign-in in a Terminal window, since it needs one to talk to.
        let quoted = format!("'{}' auth login", program.to_string_lossy().replace('\'', "'\\''"));
        let script = format!("tell application \"Terminal\" to do script \"{}\"", quoted.replace('\\', "\\\\").replace('"', "\\\""));
        return Command::new("osascript")
            .args(["-e", &script, "-e", "tell application \"Terminal\" to activate"])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't open the agent's sign-in: {e}"));
    }
    #[allow(unreachable_code)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    c.creation_flags(0x0000_0010); // CREATE_NEW_CONSOLE
    c.args(["auth", "login"]).spawn().map(|_| ()).map_err(|e| format!("Couldn't open the agent's sign-in: {e}"))
}

/// Register the board's MCP server with the agent for every session (user scope).
pub fn connect_globally(program: &Path, exe: &Path) -> Result<String, String> {
    let _ = run_agent_cmd(program, &["mcp", "remove", "-s", "user", "production-board"]);
    let exe = exe.to_string_lossy().to_string();
    run_agent_cmd(program, &["mcp", "add", "-s", "user", "production-board", "--", &exe, "--mcp"])
}

// ---------- installed-agent mode ----------

fn read_pending(path: &Path) -> Vec<Pending> {
    std::fs::read_to_string(path).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

pub fn run_agent(store: &Store, s: &Settings, prompt: &str, session: Option<&str>, cancel: Arc<AtomicBool>, pid: &dyn Fn(Option<u32>)) -> Result<Outcome, String> {
    let program = detect_agent(&s.agent_path).ok_or("The coding agent wasn't found. Set its location in Settings.")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let run_dir = settings::config_dir().join("assistant");
    std::fs::create_dir_all(&run_dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
    let stage = run_dir.join(format!("staged-{stamp}.jsonl"));
    let mcp_cfg = run_dir.join("mcp.json");
    let sys_file = run_dir.join("system-prompt.txt");
    let cfg = json!({"mcpServers": {MCP_NAME: {
        "command": exe.to_string_lossy(),
        "args": ["--mcp", "--stage-to", stage.to_string_lossy(), "--data-dir", store.dir.to_string_lossy()]
    }}});
    std::fs::write(&mcp_cfg, serde_json::to_string_pretty(&cfg).unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&sys_file, system_prompt()).map_err(|e| e.to_string())?;

    let mut cmd = command(&program);
    cmd.current_dir(&run_dir)
        .arg("-p")
        .args(["--output-format", "json"])
        .arg("--mcp-config")
        .arg(&mcp_cfg)
        .arg("--strict-mcp-config")
        .args(["--tools", ""])
        .args(["--allowedTools", &format!("mcp__{MCP_NAME}")])
        .arg("--append-system-prompt-file")
        .arg(&sys_file);
    if let Some(sid) = session {
        cmd.args(["--resume", sid]);
    }
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Couldn't start the coding agent: {e}"))?;
    pid(Some(child.id()));
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(prompt.as_bytes()).map_err(|e| e.to_string())?;
    }
    let mut out = child.stdout.take().unwrap();
    let mut errp = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = errp.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(st);
        }
        if cancel.load(Ordering::SeqCst) || start.elapsed() > RUN_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(150));
    };
    pid(None);
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    let pending = read_pending(&stage);
    let _ = std::fs::remove_file(&stage);
    if status.is_none() {
        let why = if cancel.load(Ordering::SeqCst) { "Stopped." } else { "The agent took too long and was stopped." };
        return Ok(Outcome { reply: why.into(), pending, session: session.map(str::to_string) });
    }
    let result = stdout.lines().rev().filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()).find(|v| v.get("type").and_then(Value::as_str) == Some("result"));
    match result {
        Some(v) => {
            let text = v.get("result").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let sid = v.get("session_id").and_then(Value::as_str).map(str::to_string);
            if v.get("is_error").and_then(Value::as_bool) == Some(true) && pending.is_empty() {
                let msg = if text.is_empty() { v.get("subtype").and_then(Value::as_str).unwrap_or("error").to_string() } else { text };
                if msg.to_lowercase().contains("not logged in") || msg.contains("/login") {
                    return Err("The coding agent isn't signed in on this PC. Open Settings and choose Sign in to the agent.".into());
                }
                return Err(format!("The agent reported a problem: {msg}"));
            }
            Ok(Outcome { reply: if text.is_empty() { "Done.".into() } else { text }, pending, session: sid })
        }
        None => {
            let detail = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
            let detail: String = detail.chars().take(600).collect();
            Err(if detail.is_empty() { "The agent ended without a reply. Open it once on its own to check it is signed in.".into() } else { format!("The agent ended without a reply: {detail}") })
        }
    }
}

// ---------- API-key mode ----------

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_STEPS: usize = 16;

fn supports_fallbacks(model: &str) -> bool {
    ["claude-opus-5-5", "claude-opus-5", "claude-fable-5-2", "claude-fable-5-1", "claude-sonnet-5-5"].contains(&model)
}

fn api_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string)).unwrap_or(body);
            match code {
                401 => "The API key was rejected. Check it in Settings.".into(),
                429 => "The model API is rate limiting requests. Try again in a minute.".into(),
                _ => format!("The model API returned {code}: {msg}"),
            }
        }
        ureq::Error::Transport(t) => format!("Couldn't reach the model API: {t}"),
    }
}

/// One user turn. `history` holds the conversation and is only extended if the turn succeeds.
pub fn run_api(store: &Store, s: &Settings, history: &mut Vec<Value>, prompt: &str, cancel: Arc<AtomicBool>) -> Result<Outcome, String> {
    let key = settings::get_key().ok_or("No API key is stored. Add one in Settings.")?;
    let mut ctx = Ctx::staged(store.clone(), None)?;
    let tools: Vec<Value> = tool_defs(true).into_iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect();
    let model = if s.model.trim().is_empty() { settings::DEFAULT_MODEL.to_string() } else { s.model.trim().to_string() };
    let mut msgs = history.clone();
    msgs.push(json!({"role": "user", "content": prompt}));
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(600)).build();
    let mut reply = String::new();
    for _ in 0..MAX_STEPS {
        if cancel.load(Ordering::SeqCst) {
            return Ok(Outcome { reply: "Stopped.".into(), pending: ctx.staged.take().map(|s| s.pending).unwrap_or_default(), session: None });
        }
        let mut body = json!({
            "model": model,
            "max_tokens": 16000,
            "system": system_prompt(),
            "tools": tools,
            "messages": msgs,
            "output_config": {"effort": if s.effort.is_empty() { "medium" } else { s.effort.as_str() }},
        });
        let mut req = agent.post(API_URL).set("x-api-key", &key).set("anthropic-version", API_VERSION).set("content-type", "application/json");
        if supports_fallbacks(&model) {
            body["fallbacks"] = json!("default");
            req = req.set("anthropic-beta", FALLBACK_BETA);
        }
        let resp: Value = req.send_json(body).map_err(api_error)?.into_json().map_err(|e| format!("Unreadable reply from the model API: {e}"))?;
        let content = resp.get("content").cloned().unwrap_or(json!([]));
        let stop = resp.get("stop_reason").and_then(Value::as_str).unwrap_or("");
        let text: Vec<String> = content.as_array().into_iter().flatten().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string)).collect();
        if !text.is_empty() {
            reply = text.join("\n").trim().to_string();
        }
        if stop == "refusal" {
            return Err("The model declined this request.".into());
        }
        msgs.push(json!({"role": "assistant", "content": content}));
        if stop != "tool_use" {
            break;
        }
        let results: Vec<Value> = content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .map(|b| {
                let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                let input = b.get("input").cloned().unwrap_or(json!({}));
                let (out, is_err) = match ctx.call(name, &input) {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                };
                json!({"type": "tool_result", "tool_use_id": b.get("id").cloned().unwrap_or(json!("")), "content": out, "is_error": is_err})
            })
            .collect();
        msgs.push(json!({"role": "user", "content": results}));
    }
    // Keep the conversation valid for the next turn: it must end on an assistant message.
    if msgs.last().and_then(|m| m.get("role")).and_then(Value::as_str) == Some("user") {
        msgs.push(json!({"role": "assistant", "content": [{"type": "text", "text": "(stopped after too many steps)"}]}));
    }
    *history = msgs;
    Ok(Outcome { reply: if reply.is_empty() { "Done.".into() } else { reply }, pending: ctx.staged.take().map(|s| s.pending).unwrap_or_default(), session: None })
}
