pub mod assistant;
pub mod settings;
pub mod syncer;

pub use board_core::{commands, lists, mcp, model, ops, store, sync, today, today_date, tools};

use ops::{Op, Pending, Snapshot};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use store::{Board, Store};
use syncer::Syncer;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;


#[derive(Default)]
struct Conversation {
    history: Vec<Value>,
    session: Option<String>,
    busy: bool,
}

struct AppState {
    store: Store,
    convo: Mutex<Conversation>,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<u32>>>,
    undo: Mutex<HashMap<u64, Snapshot>>,
    next_undo: Mutex<u64>,
}

#[tauri::command]
fn app_info(state: State<AppState>) -> Value {
    json!({
        "dataDir": state.store.dir.to_string_lossy(),
        "dataFile": state.store.board_path().to_string_lossy(),
        "backupsDir": state.store.backups_dir().to_string_lossy(),
        "exists": state.store.exists(),
        "exe": std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        "version": env!("CARGO_PKG_VERSION"),
        "today": today(),
        "tracks": model::tracks_json(),
    })
}

#[tauri::command]
fn load_board(state: State<AppState>) -> Result<Value, String> {
    commands::board_payload(&state.store)
}

/// Save an item from the edit dialog. The dialog sends the whole record.
#[tauri::command]
fn save_item(state: State<AppState>, item: Value, is_new: bool) -> Result<String, String> {
    commands::save_item(&state.store, &item, is_new)
}

#[tauri::command]
fn move_item(state: State<AppState>, id: String, track: String, stage: String) -> Result<(), String> {
    commands::move_item(&state.store, &id, &track, &stage)
}

#[tauri::command]
fn delete_item(state: State<AppState>, id: String) -> Result<(), String> {
    commands::delete_item(&state.store, &id)
}

#[tauri::command]
fn start_empty(state: State<AppState>) -> Result<(), String> {
    state.store.create_empty()
}

/// A seed file in the usual download folder, for the first-run offer.
#[tauri::command]
fn find_seed() -> Option<String> {
    let p = dirs::download_dir()?.join("production-board-seed.json");
    p.is_file().then(|| p.to_string_lossy().to_string())
}

/// Import a board, seed or export file. Items with an id already on the
/// board are replaced; the rest are added.
#[tauri::command]
fn import_file(app: AppHandle, state: State<AppState>, path: Option<String>) -> Result<Value, String> {
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => match app.dialog().file().add_filter("Board file", &["json"]).set_title("Import a board file").blocking_pick_file() {
            Some(f) => f.into_path().map_err(|e| e.to_string())?,
            None => return Ok(json!({"cancelled": true})),
        },
    };
    let incoming = store::read_board_file(&path)?;
    let n = incoming.projects.len();
    let (added, replaced) = state.store.modify(|b| {
        let (mut a, mut r) = (0, 0);
        for it in incoming.projects {
            if b.get(&it.id).is_some() {
                r += 1;
            } else {
                a += 1;
            }
            b.put(it);
        }
        Ok((a, r))
    })?;
    Ok(json!({"count": n, "added": added, "replaced": replaced}))
}

#[tauri::command]
fn export_board(app: AppHandle, state: State<AppState>) -> Result<Value, String> {
    let name = format!("production-board-export-{}.json", today());
    let Some(f) = app.dialog().file().add_filter("Board file", &["json"]).set_file_name(&name).set_title("Export the board").blocking_save_file() else {
        return Ok(json!({"cancelled": true}));
    };
    let p = f.into_path().map_err(|e| e.to_string())?;
    let n = state.store.load()?.projects.len();
    std::fs::write(&p, commands::export_json(&state.store)?).map_err(|e| format!("Couldn't write {}: {e}", p.display()))?;
    Ok(json!({"path": p.to_string_lossy(), "count": n}))
}

#[tauri::command]
fn reveal_data(state: State<AppState>) -> Result<(), String> {
    let p = state.store.board_path();
    if !p.exists() {
        std::fs::create_dir_all(&state.store.dir).ok();
    }
    #[cfg(target_os = "macos")]
    let mut c = {
        let mut c = std::process::Command::new("open");
        if p.exists() {
            c.arg("-R").arg(&p);
        } else {
            c.arg(&state.store.dir);
        }
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut c = {
        let mut c = std::process::Command::new("explorer");
        if p.exists() {
            c.arg(format!("/select,{}", p.display()));
        } else {
            c.arg(&state.store.dir);
        }
        c
    };
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

// ---------- settings ----------

#[tauri::command]
fn get_settings() -> Value {
    let s = settings::load();
    let agent = assistant::detect_agent(&s.agent_path).map(|p| p.to_string_lossy().to_string());
    json!({ "settings": s, "agentFound": agent, "hasKey": settings::get_key().is_some() })
}

#[tauri::command]
fn set_settings(new: settings::Settings) -> Result<(), String> {
    settings::save(&new)
}

#[tauri::command]
fn set_api_key(key: String) -> Result<(), String> {
    if key.trim().len() < 20 {
        return Err("That doesn't look like a complete API key.".into());
    }
    settings::set_key(&key)
}

#[tauri::command]
fn clear_api_key() -> Result<(), String> {
    settings::clear_key()
}

#[tauri::command]
fn pick_agent(app: AppHandle) -> Option<String> {
    app.dialog().file().add_filter("Program", &["exe", "cmd"]).set_title("Find the coding agent's program").blocking_pick_file().and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
async fn test_agent(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = assistant::detect_agent(&path).ok_or("The coding agent wasn't found.")?;
        assistant::run_agent_cmd(&p, &["--version"])
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn agent_status(path: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || match assistant::detect_agent(&path) {
        None => json!({"found": null, "signedIn": null}),
        Some(p) => json!({"found": p.to_string_lossy(), "signedIn": assistant::signed_in(&p)}),
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn agent_sign_in(path: String) -> Result<(), String> {
    let p = assistant::detect_agent(&path).ok_or("The coding agent wasn't found.")?;
    assistant::open_sign_in(&p)
}

#[tauri::command]
async fn connect_agent(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = assistant::detect_agent(&path).ok_or("The coding agent wasn't found.")?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        assistant::connect_globally(&p, &exe)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------- assistant ----------

#[tauri::command]
async fn assistant_send(app: AppHandle, text: String) -> Result<Value, String> {
    let st = app.state::<AppState>();
    let s = settings::load();
    let (history, session) = {
        let mut c = st.convo.lock().unwrap();
        if c.busy {
            return Err("The assistant is still working on the last request.".into());
        }
        c.busy = true;
        (c.history.clone(), c.session.clone())
    };
    st.cancel.store(false, Ordering::SeqCst);
    let store = st.store.clone();
    let cancel = st.cancel.clone();
    let child = st.child.clone();
    let mode = s.mode.clone();
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<(assistant::Outcome, Vec<Value>), String> {
        match mode.as_str() {
            "agent" => {
                let set_pid = |p: Option<u32>| *child.lock().unwrap() = p;
                assistant::run_agent(&store, &s, &text, session.as_deref(), cancel, &set_pid).map(|o| (o, history))
            }
            "api" => {
                let mut h = history;
                assistant::run_api(&store, &s, &mut h, &text, cancel).map(|o| (o, h))
            }
            _ => Err("The assistant is switched off. Choose a connection in Settings.".into()),
        }
    })
    .await
    .map_err(|e| e.to_string());
    let mut c = st.convo.lock().unwrap();
    c.busy = false;
    let (out, history) = res??;
    c.history = history;
    if out.session.is_some() {
        c.session = out.session.clone();
    }
    let raw: Vec<Op> = out.pending.iter().map(|p| p.op.clone()).collect();
    Ok(json!({ "reply": out.reply, "pending": out.pending, "needsConfirm": ops::needs_confirmation(&raw) }))
}

#[tauri::command]
fn assistant_stop(state: State<AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn assistant_reset(state: State<AppState>) -> Result<(), String> {
    let mut c = state.convo.lock().unwrap();
    if c.busy {
        return Err("Wait for the current request to finish, or stop it.".into());
    }
    *c = Conversation::default();
    Ok(())
}

/// Apply staged changes. Returns an undo token and what happened.
#[tauri::command]
fn apply_changes(state: State<AppState>, changes: Vec<Pending>) -> Result<Value, String> {
    let ops: Vec<Op> = changes.into_iter().map(|p| p.op).collect();
    let applied = state.store.modify(|b| Ok(ops::apply_all(b, &ops, &today())))?;
    let token = {
        let mut n = state.next_undo.lock().unwrap();
        *n += 1;
        *n
    };
    state.undo.lock().unwrap().insert(token, applied.snapshot);
    Ok(json!({"undo": token, "done": applied.done, "failed": applied.failed}))
}

#[tauri::command]
fn undo_changes(state: State<AppState>, token: u64) -> Result<(), String> {
    let snap = state.undo.lock().unwrap().remove(&token).ok_or("Those changes can no longer be undone.")?;
    state.store.modify(|b: &mut Board| {
        ops::restore(b, &snap);
        Ok(())
    })
}

// ---------- sync ----------

#[tauri::command]
fn sync_status(state: State<AppState>) -> Value {
    syncer::status(&state.store)
}

#[tauri::command]
async fn sync_now(app: AppHandle) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<Syncer>().run_now().map(|r| json!(r))).await.map_err(|e| e.to_string())?
}

/// Check the server and key, then save them. Sync starts right away.
#[tauri::command]
async fn sync_configure(app: AppHandle, server: String, key: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let r = configure_sync(&server, &key)?;
        app.state::<Syncer>().trigger();
        Ok(r)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn configure_sync(server: &str, key: &str) -> Result<Value, String> {
    let server = sync::http::base_url(server);
    if key.trim().len() < 20 {
        return Err("That doesn't look like a complete access key.".into());
    }
    let ping = sync::http::post(&server, key, "/api/ping", &json!({}))?;
    if ping.get("access").is_some() {
        return Err("That's a limited access key, made for another tool. Sync needs the server's main access key.".into());
    }
    settings::set_sync_key(key)?;
    let mut s = settings::load();
    s.sync_server = server;
    settings::save(&s)?;
    Ok(ping)
}

#[tauri::command]
fn sync_disable(app: AppHandle) -> Result<(), String> {
    let mut s = settings::load();
    s.sync_server.clear();
    settings::save(&s)?;
    settings::clear_sync_key()?;
    let _ = app.emit("sync-status", json!({"configured": false}));
    Ok(())
}

#[tauri::command]
async fn sync_set_web_password(password: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (server, key) = settings::sync_config().ok_or("Set up sync first.")?;
        sync::http::post(&server, &key, "/api/set-password", &json!({ "password": password })).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn sync_resolve(sync: State<Syncer>, id: String, keep_mine: bool) -> Result<(), String> {
    sync.resolve(&id, keep_mine)
}

/// Limited access keys on the server, for other tools: list, create or revoke.
#[tauri::command]
async fn sync_keys(action: String, args: Value) -> Result<Value, String> {
    if !matches!(action.as_str(), "list" | "create" | "revoke") {
        return Err(format!("Unknown key action \"{action}\"."));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let (server, key) = settings::sync_config().ok_or("Set up sync first.")?;
        sync::http::post(&server, &key, &format!("/api/keys/{action}"), &args)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The access key, for connecting an agent session elsewhere to the server.
#[tauri::command]
fn sync_key() -> Option<String> {
    settings::get_sync_key()
}

/// `production-board.exe --configure-sync <server>` with the access key on
/// stdin: sets up sync without opening the window.
pub fn configure_sync_from_args(args: &[String]) -> i32 {
    let Some(server) = args.iter().position(|a| a == "--configure-sync").and_then(|i| args.get(i + 1)) else {
        eprintln!("usage: --configure-sync <server address>, with the access key on standard input");
        return 2;
    };
    let mut key = String::new();
    if std::io::stdin().read_line(&mut key).is_err() {
        return 2;
    }
    match configure_sync(server, &key) {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// Tell the window when the board file changes on disk, for example after
/// an agent session updates it through the MCP server.
fn watch(app: AppHandle, store: Store) {
    std::thread::spawn(move || {
        let mut last = store.modified_time();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(900));
            let now = store.modified_time();
            if now != last {
                last = now;
                let _ = app.emit("board-changed", ());
                if let Some(s) = app.try_state::<Syncer>() {
                    s.trigger();
                }
            }
        }
    });
}

pub fn run() {
    let store = Store::open_default();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            store: store.clone(),
            convo: Mutex::new(Conversation::default()),
            cancel: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
            undo: Mutex::new(HashMap::new()),
            next_undo: Mutex::new(0),
        })
        .setup(move |app| {
            app.manage(Syncer::start(app.handle().clone(), store.clone()));
            watch(app.handle().clone(), store.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info, load_board, save_item, move_item, delete_item, start_empty, find_seed, import_file, export_board, reveal_data,
            get_settings, set_settings, set_api_key, clear_api_key, pick_agent, test_agent, agent_status, agent_sign_in, connect_agent,
            assistant_send, assistant_stop, assistant_reset, apply_changes, undo_changes,
            sync_status, sync_now, sync_configure, sync_disable, sync_set_web_password, sync_resolve, sync_key, sync_keys
        ])
        .run(tauri::generate_context!())
        .expect("error while running Production Board");
}
