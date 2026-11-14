//! The board on a server. One small program that:
//!
//! * keeps the server's copy of the board and acts as the sync hub for the
//!   desktop app (POST /api/sync),
//! * serves the board as a web page behind a password (the same screens as
//!   the desktop window, minus the assistant and settings),
//! * offers the board's tools to agent sessions over HTTP (POST /mcp).
//!
//! It speaks plain HTTP and expects a reverse proxy in front for HTTPS.
//!
//!   production-board-server --data-dir /data --web-dir /app/web [--listen 0.0.0.0:8080]
//!   production-board-server --data-dir /data --import seed.json
//!
//! The main access key, for sync and agent sessions, is given as its SHA-256
//! in the PB_KEY_SHA256 environment variable, so the key itself is never
//! stored on the server. The web password is set from the desktop app, which
//! can also make extra keys with limited access for other tools (see keys.rs).
//!
//! What each caller may reach:
//!
//!   main key        everything: /mcp, sync, change feed, web password, keys
//!   limited key     /mcp (its tracks; read-only unless allowed to change),
//!                   POST /api/changes and POST /api/ping
//!   web sign-in     the web page and its board commands

mod keys;

use board_core::commands;
use board_core::store::{read_board_file, Store};
use board_core::sync::{Hub, SyncRequest};
use board_core::tools::{Ctx, Scope};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server};

const LOGIN_PAGE: &str = include_str!("login.html");
const MAX_BODY: usize = 8 * 1024 * 1024;
const SESSION_DAYS: u64 = 30;
const COOKIE: &str = "pb_session";

struct App {
    hub: Mutex<Hub>,
    store: Store,
    web_dir: PathBuf,
    key_hash: Option<[u8; 32]>,
    keys: Mutex<keys::Keys>,
    auth: Mutex<Auth>,
}

#[derive(Default)]
struct Auth {
    /// SHA-256 of each session token -> expiry (unix seconds).
    sessions: HashMap<String, u64>,
    failures: VecDeque<Instant>,
}

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn random_token() -> String {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("no random source");
    hex(&b)
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------- password and sessions ----------

impl App {
    fn password_file(&self) -> PathBuf {
        self.store.dir.join("web-password.txt")
    }
    fn sessions_file(&self) -> PathBuf {
        self.store.dir.join("web-sessions.json")
    }
    fn password_set(&self) -> bool {
        self.password_file().is_file()
    }

    fn load_sessions(&self) {
        let map: HashMap<String, u64> = std::fs::read_to_string(self.sessions_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let now = now_secs();
        self.auth.lock().unwrap().sessions = map.into_iter().filter(|(_, exp)| *exp > now).collect();
    }

    fn save_sessions(&self, a: &Auth) {
        let _ = std::fs::write(self.sessions_file(), serde_json::to_string(&a.sessions).unwrap());
    }

    fn set_password(&self, pw: &str) -> Result<(), String> {
        use argon2::password_hash::{PasswordHasher, SaltString};
        if pw.chars().count() < 10 {
            return Err("Use at least 10 characters for the web password.".into());
        }
        let mut salt = [0u8; 16];
        getrandom::getrandom(&mut salt).map_err(|e| e.to_string())?;
        let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
        let hash = argon2::Argon2::default().hash_password(pw.as_bytes(), &salt).map_err(|e| e.to_string())?.to_string();
        std::fs::write(self.password_file(), hash).map_err(|e| format!("Couldn't save the password: {e}"))?;
        // A new password signs out every browser.
        let mut a = self.auth.lock().unwrap();
        a.sessions.clear();
        self.save_sessions(&a);
        Ok(())
    }

    fn check_password(&self, pw: &str) -> bool {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        let Ok(stored) = std::fs::read_to_string(self.password_file()) else { return false };
        let Ok(parsed) = PasswordHash::new(stored.trim()) else { return false };
        argon2::Argon2::default().verify_password(pw.as_bytes(), &parsed).is_ok()
    }

    fn new_session(&self) -> String {
        let token = random_token();
        let mut a = self.auth.lock().unwrap();
        a.sessions.insert(hex(&sha256(token.as_bytes())), now_secs() + SESSION_DAYS * 86400);
        self.save_sessions(&a);
        token
    }

    fn end_session(&self, token: &str) {
        let mut a = self.auth.lock().unwrap();
        a.sessions.remove(&hex(&sha256(token.as_bytes())));
        self.save_sessions(&a);
    }

    fn session_ok(&self, token: &str) -> bool {
        let a = self.auth.lock().unwrap();
        a.sessions.get(&hex(&sha256(token.as_bytes()))).is_some_and(|exp| *exp > now_secs())
    }

    /// Who a bearer key belongs to: the main key, a limited key, or nobody.
    fn key_access(&self, key: &str) -> Option<Access> {
        let h = sha256(key.trim().as_bytes());
        if self.key_hash.as_ref().is_some_and(|m| same(&h, m)) {
            return Some(Access { main: true, scope: Scope::full(), name: "main key".into() });
        }
        if self.key_hash.is_none() {
            return None;
        }
        let e = self.keys.lock().unwrap().find(&hex(&h), &board_core::today())?;
        Some(Access { main: false, scope: e.scope(), name: e.name })
    }

    /// Ten wrong passwords in fifteen minutes pause sign-in for everyone.
    fn locked_out(&self) -> bool {
        let mut a = self.auth.lock().unwrap();
        let cutoff = Instant::now() - Duration::from_secs(15 * 60);
        while a.failures.front().is_some_and(|t| *t < cutoff) {
            a.failures.pop_front();
        }
        a.failures.len() >= 10
    }

    fn note_failure(&self) {
        self.auth.lock().unwrap().failures.push_back(Instant::now());
    }
}

// ---------- HTTP plumbing ----------

struct Req {
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn bearer(&self) -> Option<&str> {
        let h = self.header("Authorization")?;
        h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))
    }
    fn cookie(&self, name: &str) -> Option<String> {
        self.header("Cookie")?.split(';').map(str::trim).find_map(|kv| kv.strip_prefix(&format!("{name}=")).map(String::from))
    }
    fn json(&self) -> Result<Value, Reply> {
        if self.body.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&self.body).map_err(|_| Reply::error(400, "The request wasn't valid JSON."))
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
    ctype: &'static str,
    headers: Vec<(String, String)>,
}

impl Reply {
    fn json(status: u16, v: Value) -> Reply {
        Reply { status, body: v.to_string().into_bytes(), ctype: "application/json; charset=utf-8", headers: vec![] }
    }
    fn error(status: u16, msg: &str) -> Reply {
        Reply::json(status, json!({ "error": msg }))
    }
    fn html(body: &str) -> Reply {
        Reply { status: 200, body: body.as_bytes().to_vec(), ctype: "text/html; charset=utf-8", headers: vec![("Cache-Control".into(), "no-store".into())] }
    }
    fn redirect(to: &str) -> Reply {
        Reply { status: 302, body: vec![], ctype: "text/plain", headers: vec![("Location".into(), to.into())] }
    }
    fn with(mut self, k: &str, v: String) -> Reply {
        self.headers.push((k.into(), v));
        self
    }
}

fn read_req(r: &mut Request) -> Result<Req, Reply> {
    let headers: Vec<(String, String)> = r.headers().iter().map(|h| (h.field.as_str().as_str().to_string(), h.value.as_str().to_string())).collect();
    let mut body = Vec::new();
    r.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body).map_err(|_| Reply::error(400, "Couldn't read the request."))?;
    if body.len() > MAX_BODY {
        return Err(Reply::error(413, "That request is too large."));
    }
    let path = r.url().split('?').next().unwrap_or("/").to_string();
    Ok(Req { method: r.method().clone(), path, headers, body })
}

fn send(r: Request, reply: Reply) {
    let mut resp = Response::from_data(reply.body).with_status_code(reply.status);
    let mut add = |k: &str, v: &str| {
        if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    };
    add("Content-Type", reply.ctype);
    add("X-Content-Type-Options", "nosniff");
    add("Referrer-Policy", "no-referrer");
    add("X-Frame-Options", "DENY");
    add("Strict-Transport-Security", "max-age=31536000");
    add("X-Robots-Tag", "noindex");
    if reply.ctype.starts_with("text/html") {
        add("Content-Security-Policy", "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'");
    }
    for (k, v) in &reply.headers {
        add(k, v);
    }
    let _ = r.respond(resp);
}

fn content_type(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

fn session_cookie(token: &str, max_age: u64) -> String {
    format!("{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}")
}

// ---------- routes ----------

struct Access {
    /// The server's main key, as opposed to a limited one.
    main: bool,
    scope: Scope,
    name: String,
}

enum Who {
    Nobody,
    Browser,
    Key(Access),
}

fn who(app: &App, req: &Req) -> Who {
    if let Some(k) = req.bearer() {
        return match app.key_access(k) {
            Some(a) => Who::Key(a),
            None => Who::Nobody,
        };
    }
    match req.cookie(COOKIE) {
        Some(t) if app.session_ok(&t) => Who::Browser,
        _ => Who::Nobody,
    }
}

/// Browser writes must come from this site's own page.
fn same_origin(req: &Req) -> bool {
    let Some(host) = req.header("Host") else { return false };
    match req.header("Origin") {
        Some(o) => o.strip_prefix("https://").or_else(|| o.strip_prefix("http://")) == Some(host),
        None => req.header("Sec-Fetch-Site").is_none_or(|s| s == "same-origin"),
    }
}

fn route(app: &App, req: &Req) -> Reply {
    let p = req.path.as_str();
    match (&req.method, p) {
        (Method::Get, "/health") => return Reply::json(200, json!({"ok": true})),
        (Method::Get, "/login") => return Reply::html(LOGIN_PAGE),
        (Method::Post, "/api/login") => return login(app, req),
        (Method::Post, "/api/logout") => {
            if let Some(t) = req.cookie(COOKIE) {
                app.end_session(&t);
            }
            return Reply::json(200, json!({"ok": true})).with("Set-Cookie", session_cookie("", 0));
        }
        _ => {}
    }
    let caller = who(app, req);
    if p == "/mcp" {
        return match (caller, &req.method) {
            (Who::Key(a), Method::Post) => mcp(app, req, a.scope),
            (Who::Key(_), _) => Reply::error(405, "Use POST."),
            _ => Reply::error(401, "This endpoint needs the board's access key.").with("WWW-Authenticate", "Bearer".into()),
        };
    }
    if let Some(rest) = p.strip_prefix("/api/") {
        if matches!(caller, Who::Nobody) {
            return Reply::error(401, "Sign in first.");
        }
        if matches!(caller, Who::Browser) && req.method == Method::Post && !same_origin(req) {
            return Reply::error(403, "That request didn't come from the board's own page.");
        }
        if let Who::Key(a) = &caller {
            if !a.main && !matches!(rest, "ping" | "changes") {
                return Reply::error(403, "This access key is limited to the board's tools, the change feed and ping.");
            }
        }
        return api(app, req, rest, &caller);
    }
    if req.method != Method::Get {
        return Reply::error(405, "Not allowed.");
    }
    if p == "/" || p == "/index.html" {
        if !matches!(caller, Who::Browser) {
            return Reply::redirect("/login");
        }
        return static_file(app, "index.html");
    }
    static_file(app, &p[1..])
}

fn static_file(app: &App, rel: &str) -> Reply {
    if rel.split('/').any(|seg| seg == ".." || seg.starts_with('.') || seg.contains('\\')) {
        return Reply::error(404, "Not found.");
    }
    let path = app.web_dir.join(rel);
    match std::fs::read(&path) {
        Ok(data) => {
            let cache = if rel.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-store" };
            Reply { status: 200, body: data, ctype: content_type(&path), headers: vec![("Cache-Control".into(), cache.into())] }
        }
        Err(_) => Reply::error(404, "Not found."),
    }
}

fn login(app: &App, req: &Req) -> Reply {
    if !same_origin(req) {
        return Reply::error(403, "Sign in from the board's own page.");
    }
    if !app.password_set() {
        return Reply::error(409, "No web password has been set yet. Set one in the desktop app: Settings, then Sync.");
    }
    if app.locked_out() {
        return Reply::error(429, "Too many wrong passwords. Wait 15 minutes and try again.");
    }
    let body = match req.json() {
        Ok(v) => v,
        Err(r) => return r,
    };
    let pw = body.get("password").and_then(Value::as_str).unwrap_or("");
    if !app.check_password(pw) {
        app.note_failure();
        return Reply::error(401, "That password isn't right.");
    }
    let token = app.new_session();
    Reply::json(200, json!({"ok": true})).with("Set-Cookie", session_cookie(&token, SESSION_DAYS * 86400))
}

fn api(app: &App, req: &Req, rest: &str, caller: &Who) -> Reply {
    let by_key = matches!(caller, Who::Key(a) if a.main);
    let scope = match caller {
        Who::Key(a) => a.scope.clone(),
        _ => Scope::full(),
    };
    let result = (|| -> Result<Value, String> {
        match (&req.method, rest) {
            (Method::Get, "version") => {
                let mut hub = app.hub.lock().unwrap();
                hub.reconcile()?;
                Ok(json!({"seq": hub.index.seq}))
            }
            (Method::Get, "export") => Err("use-export".into()),
            (Method::Post, "ping") => match caller {
                Who::Key(a) if !a.main => {
                    let n = app.store.load()?.projects.iter().filter(|p| a.scope.allows(&p.track)).count();
                    Ok(json!({"ok": true, "items": n, "version": env!("CARGO_PKG_VERSION"),
                        "access": {"name": a.name, "write": !a.scope.read_only, "tracks": a.scope.tracks.clone().unwrap_or_default()}}))
                }
                _ => Ok(json!({"ok": true, "items": app.store.load()?.projects.len(), "passwordSet": app.password_set(), "version": env!("CARGO_PKG_VERSION")})),
            },
            (Method::Post, "changes") => {
                let v: Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
                let since = v.get("since").and_then(Value::as_u64).unwrap_or(0);
                let (seq, changes) = app.hub.lock().unwrap().changes_since(since, &scope)?;
                Ok(json!({"seq": seq, "changes": changes}))
            }
            (Method::Post, "keys/list") if by_key => Ok(json!({"keys": app.keys.lock().unwrap().list.iter().map(|k| k.public()).collect::<Vec<_>>()})),
            (Method::Post, "keys/create") if by_key => {
                let v: Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
                let (entry, key) = app.keys.lock().unwrap().create(&v, &board_core::today())?;
                let mut out = entry.public();
                out["key"] = json!(key);
                Ok(out)
            }
            (Method::Post, "keys/revoke") if by_key => {
                let v: Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
                app.keys.lock().unwrap().revoke(v.get("id").and_then(Value::as_str).unwrap_or(""))?;
                Ok(json!({"ok": true}))
            }
            (Method::Post, "sync") if by_key => {
                let req: SyncRequest = serde_json::from_slice(&req.body).map_err(|e| format!("The sync request couldn't be read: {e}"))?;
                let resp = app.hub.lock().unwrap().handle(&req)?;
                Ok(serde_json::to_value(resp).unwrap())
            }
            (Method::Post, "set-password") if by_key => {
                let v: Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
                app.set_password(v.get("password").and_then(Value::as_str).unwrap_or(""))?;
                Ok(json!({"ok": true}))
            }
            (Method::Post, cmd) if cmd.starts_with("call/") => call(app, &cmd[5..], &serde_json::from_slice(&req.body).unwrap_or(json!({}))),
            _ => Err("not-found".into()),
        }
    })();
    match result {
        Ok(v) => Reply::json(200, v),
        Err(e) if e == "not-found" => Reply::error(404, "Not found."),
        Err(e) if e == "use-export" => match commands::export_json(&app.store) {
            Ok(text) => Reply { status: 200, body: text.into_bytes(), ctype: "application/json; charset=utf-8", headers: vec![("Content-Disposition".into(), format!("attachment; filename=\"production-board-export-{}.json\"", board_core::today()))] },
            Err(e) => Reply::error(500, &e),
        },
        Err(e) => Reply::error(400, &e),
    }
}

/// The web page's board commands. Each change is numbered for sync at once.
fn call(app: &App, cmd: &str, a: &Value) -> Result<Value, String> {
    let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut hub = app.hub.lock().unwrap();
    let out = match cmd {
        "app_info" => json!({
            "dataDir": "", "dataFile": "", "backupsDir": "", "exists": true, "exe": "", "web": true,
            "version": env!("CARGO_PKG_VERSION"), "today": board_core::today(), "tracks": board_core::model::tracks_json(),
        }),
        "load_board" => {
            let mut v = commands::board_payload(&app.store)?;
            v["exists"] = json!(true);
            v
        }
        "save_item" => json!(commands::save_item(&app.store, a.get("item").unwrap_or(&Value::Null), a.get("isNew").and_then(Value::as_bool).unwrap_or(false))?),
        "move_item" => {
            commands::move_item(&app.store, &s("id"), &s("track"), &s("stage"))?;
            Value::Null
        }
        "delete_item" => {
            commands::delete_item(&app.store, &s("id"))?;
            Value::Null
        }
        _ => return Err(format!("The web board doesn't offer \"{cmd}\".")),
    };
    hub.reconcile()?;
    Ok(out)
}

/// Streamable HTTP transport for the board's tools: one JSON-RPC message (or
/// batch) per POST, answered with JSON.
fn mcp(app: &App, req: &Req, scope: Scope) -> Reply {
    let msg: Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(e) => return Reply::json(400, json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("Parse error: {e}")}})),
    };
    let mut hub = app.hub.lock().unwrap();
    let mut ctx = Ctx::scoped(app.store.clone(), scope);
    let reply = match &msg {
        Value::Array(batch) => {
            let out: Vec<Value> = batch.iter().filter_map(|m| board_core::mcp::handle(&mut ctx, m, false)).collect();
            if out.is_empty() { None } else { Some(Value::Array(out)) }
        }
        m => board_core::mcp::handle(&mut ctx, m, false),
    };
    let _ = hub.reconcile();
    match reply {
        Some(v) => Reply::json(200, v),
        None => Reply { status: 202, body: vec![], ctype: "application/json", headers: vec![] },
    }
}

// ---------- start-up ----------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(data) = arg_after(&args, "--data-dir") else {
        eprintln!("usage: production-board-server --data-dir <folder> [--web-dir <folder>] [--listen <addr:port>] | --import <file>");
        std::process::exit(2);
    };
    let store = Store { dir: PathBuf::from(data) };
    if let Some(file) = arg_after(&args, "--import") {
        let incoming = match read_board_file(Path::new(&file)) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let n = incoming.projects.len();
        let r = store.modify(|b| {
            for it in incoming.projects {
                b.put(it);
            }
            Ok(())
        });
        match r.and_then(|_| Hub::open(store.clone()).map(|_| ())) {
            Ok(()) => println!("imported {n} items"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let listen = arg_after(&args, "--listen").unwrap_or_else(|| "0.0.0.0:8080".into());
    let web_dir = PathBuf::from(arg_after(&args, "--web-dir").unwrap_or_else(|| "web".into()));
    let key_hash = std::env::var("PB_KEY_SHA256").ok().and_then(|h| unhex(&h));
    if key_hash.is_none() {
        eprintln!("warning: PB_KEY_SHA256 is not set; sync and agent sessions are disabled");
    }
    let hub = match Hub::open(store.clone()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("couldn't open the board: {e}");
            std::process::exit(1);
        }
    };
    let keys = Mutex::new(keys::Keys::load(&store.dir));
    let app = Arc::new(App { hub: Mutex::new(hub), store, web_dir, key_hash, keys, auth: Mutex::new(Auth::default()) });
    app.load_sessions();
    let server = match Server::http(&listen) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("couldn't listen on {listen}: {e}");
            std::process::exit(1);
        }
    };
    println!("production board server {} listening on {listen}", env!("CARGO_PKG_VERSION"));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let (server, app) = (server.clone(), app.clone());
            std::thread::spawn(move || loop {
                let Ok(mut r) = server.recv() else { break };
                let reply = match read_req(&mut r) {
                    Ok(req) => route(&app, &req),
                    Err(reply) => reply,
                };
                send(r, reply);
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}
