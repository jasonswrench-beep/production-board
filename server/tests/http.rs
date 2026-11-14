//! Starts the real server and talks to it the way the desktop app, a
//! browser and an agent session do.

use board_core::store::{read_board_file, Store};
use board_core::sync::http;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

const KEY: &str = "test-access-key-0123456789";

struct Running {
    child: Child,
    url: String,
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn temp(name: &str) -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let d = std::env::temp_dir().join(format!("pb-http-{}-{n}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start() -> Running {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let web = temp("web");
    std::fs::write(web.join("index.html"), "<!doctype html><title>board page</title>").unwrap();
    let hash: String = Sha256::digest(KEY.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let child = Command::new(env!("CARGO_BIN_EXE_production-board-server"))
        .args(["--data-dir", temp("data").to_str().unwrap(), "--web-dir", web.to_str().unwrap(), "--listen", &format!("127.0.0.1:{port}")])
        .env("PB_KEY_SHA256", hash)
        .spawn()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    for _ in 0..100 {
        if ureq::get(&format!("{url}/health")).call().is_ok() {
            return Running { child, url };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server didn't start");
}

fn seed_into(store: &Store) {
    let seed = read_board_file(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src-tauri/tests/fixtures/seed.json")).unwrap();
    store.modify(|b| {
        for it in seed.projects {
            b.put(it);
        }
        Ok(())
    })
    .unwrap();
}

fn status(r: Result<ureq::Response, ureq::Error>) -> u16 {
    match r {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(c, _)) => c,
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn desktop_sync_agent_tools_and_web_sign_in() {
    let s = start();
    let url = s.url.clone();

    // Without the key nothing but the sign-in page is reachable.
    assert_eq!(status(ureq::post(&format!("{url}/api/sync")).send_json(json!({"since": 0, "push": []}))), 401);
    assert_eq!(status(ureq::post(&format!("{url}/mcp")).send_json(json!({}))), 401);
    assert!(http::sync(&Store { dir: temp("x") }, &url, "wrong-key").unwrap_err().contains("access key"));

    // Desktop A sends its board; desktop B pulls it.
    let a = Store { dir: temp("a") };
    let b = Store { dir: temp("b") };
    seed_into(&a);
    assert_eq!(http::sync(&a, &url, KEY).unwrap().sent, 67);
    assert_eq!(http::sync(&b, &url, KEY).unwrap().received, 67);

    // An agent session moves a card through the HTTP tools endpoint.
    let rpc = |body: Value| -> Value { ureq::post(&format!("{url}/mcp")).set("Authorization", &format!("Bearer {KEY}")).send_json(body).unwrap().into_json().unwrap() };
    let init = rpc(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "production-board");
    let tools = rpc(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "move_item"));
    let moved = rpc(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "move_item", "arguments": {"id": "ichabod", "stage": "Editing"}}}));
    assert_eq!(moved["result"]["isError"], false, "{moved}");

    // Both desktops pick the move up.
    assert_eq!(http::sync(&a, &url, KEY).unwrap().received, 1);
    assert_eq!(http::sync(&b, &url, KEY).unwrap().received, 1);
    assert_eq!(b.load().unwrap().get("ichabod").unwrap().stage, "edit");

    // Web sign-in: no password yet, then one set from the desktop with the key.
    let login = |pw: &str| ureq::post(&format!("{url}/api/login")).set("Origin", &url).send_json(json!({"password": pw}));
    assert_eq!(status(login("whatever-password")), 409);
    assert!(http::post(&url, KEY, "/api/set-password", &json!({"password": "short"})).unwrap_err().contains("10 characters"));
    http::post(&url, KEY, "/api/set-password", &json!({"password": "a long web password"})).unwrap();
    assert_eq!(status(login("not the password")), 401);
    let ok = login("a long web password").unwrap();
    let cookie = ok.header("Set-Cookie").unwrap().split(';').next().unwrap().to_string();
    assert!(ok.header("Set-Cookie").unwrap().contains("HttpOnly"));

    // The page needs the session; the board commands work with it.
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    assert_eq!(status(agent.get(&format!("{url}/")).call()), 302);
    let page = agent.get(&format!("{url}/")).set("Cookie", &cookie).call().unwrap().into_string().unwrap();
    assert!(page.contains("board page"));
    let board: Value = agent.post(&format!("{url}/api/call/load_board")).set("Cookie", &cookie).set("Origin", &url).send_json(json!({})).unwrap().into_json().unwrap();
    assert_eq!(board["projects"].as_array().unwrap().len(), 67);

    // A web edit syncs down to the desktop.
    agent.post(&format!("{url}/api/call/move_item")).set("Cookie", &cookie).set("Origin", &url).send_json(json!({"id": "ichabod", "track": "fiction", "stage": "prod"})).unwrap();
    assert_eq!(http::sync(&a, &url, KEY).unwrap().received, 1);
    assert_eq!(a.load().unwrap().get("ichabod").unwrap().stage, "prod");

    // Another site can't use the session to make changes.
    assert_eq!(status(agent.post(&format!("{url}/api/call/delete_item")).set("Cookie", &cookie).set("Origin", "https://evil.example").send_json(json!({"id": "ichabod"}))), 403);
    // Paths outside the web folder aren't served.
    assert_eq!(status(agent.get(&format!("{url}/../data/board.json")).set("Cookie", &cookie).call()), 404);
    assert_eq!(status(agent.get(&format!("{url}/.sync-state.json")).set("Cookie", &cookie).call()), 404);

    // Signing out ends the session.
    agent.post(&format!("{url}/api/logout")).set("Cookie", &cookie).send_json(json!({})).unwrap();
    assert_eq!(status(agent.post(&format!("{url}/api/call/load_board")).set("Cookie", &cookie).set("Origin", &url).send_json(json!({}))), 401);
}

#[test]
fn limited_access_keys() {
    let s = start();
    let url = s.url.clone();
    seed_into(&Store { dir: temp("seed") });
    let a = Store { dir: temp("a") };
    seed_into(&a);
    http::sync(&a, &url, KEY).unwrap();

    // Only the main key manages keys.
    let made = http::post(&url, KEY, "/api/keys/create", &json!({"name": "Notes tool", "write": false, "tracks": ["software", "Research"]})).unwrap();
    let ro = made["key"].as_str().unwrap().to_string();
    assert_eq!(made["tracks"], json!(["software", "research"]));
    assert!(http::post(&url, &ro, "/api/keys/list", &json!({})).is_err());
    assert!(http::post(&url, KEY, "/api/keys/create", &json!({"name": "", "tracks": []})).unwrap_err().contains("name"));
    assert!(http::post(&url, KEY, "/api/keys/create", &json!({"name": "x", "tracks": ["nope"]})).unwrap_err().contains("Unknown track"));
    let listed = http::post(&url, KEY, "/api/keys/list", &json!({})).unwrap();
    assert_eq!(listed["keys"].as_array().unwrap().len(), 1);
    assert!(listed.to_string().find("sha256").is_none(), "hashes stay on the server");

    // A limited key can't sync, set the password or use the web page's commands.
    assert!(http::sync(&Store { dir: temp("ro") }, &url, &ro).is_err());
    assert!(http::post(&url, &ro, "/api/set-password", &json!({"password": "a long web password"})).is_err());
    assert!(http::post(&url, &ro, "/api/call/delete_item", &json!({"id": "ichabod"})).is_err());

    // Ping says what the key covers.
    let ping = http::post(&url, &ro, "/api/ping", &json!({})).unwrap();
    assert_eq!(ping["items"], 17);
    assert_eq!(ping["access"]["write"], false);
    assert!(ping.get("passwordSet").is_none());

    // The tools show only its tracks and refuse changes.
    let rpc = |key: &str, id: u64, name: &str, args: Value| -> Value {
        ureq::post(&format!("{url}/mcp")).set("Authorization", &format!("Bearer {key}"))
            .send_json(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}}))
            .unwrap().into_json::<Value>().unwrap()["result"].clone()
    };
    let tools: Value = ureq::post(&format!("{url}/mcp")).set("Authorization", &format!("Bearer {ro}")).send_json(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap().into_json().unwrap();
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"get_item") && !names.contains(&"move_item") && !names.contains(&"update_item"), "{names:?}");
    let list = rpc(&ro, 2, "list_items", json!({"include_finished": true}));
    let listed: Value = serde_json::from_str(list["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(listed["items"].as_array().unwrap().iter().all(|i| i["track"] == "software" || i["track"] == "research"));
    assert_eq!(rpc(&ro, 3, "get_item", json!({"id": "ichabod"}))["isError"], true, "fiction is out of scope");
    assert_eq!(rpc(&ro, 4, "get_item", json!({"id": "apa-paper-feedback"}))["isError"], false);
    let refused = rpc(&ro, 5, "log_activity", json!({"id": "apa-paper-feedback", "text": "x"}));
    assert_eq!(refused["isError"], true);
    assert!(refused["content"][0]["text"].as_str().unwrap().contains("only read"));

    // A key that may change cards still stays inside its tracks.
    let rw = http::post(&url, KEY, "/api/keys/create", &json!({"name": "Planner", "write": true, "tracks": ["software"]})).unwrap()["key"].as_str().unwrap().to_string();
    assert_eq!(rpc(&rw, 6, "log_activity", json!({"id": "apa-paper-feedback", "text": "Linked the spec", "next": "Review the spec"}))["isError"], false);
    assert_eq!(rpc(&rw, 7, "update_item", json!({"id": "apa-paper-feedback", "fields": {"add_links": ["Spec | https://example.org/spec"]}}))["isError"], false);
    assert_eq!(rpc(&rw, 8, "move_item", json!({"id": "apa-paper-feedback", "track": "fiction", "stage": "plot"}))["isError"], true);
    assert_eq!(rpc(&rw, 9, "add_item", json!({"title": "Out of scope", "track": "teaching"}))["isError"], true);
    assert_eq!(rpc(&rw, 10, "add_item", json!({"title": "Brain adapter", "track": "software"}))["isError"], false);
    assert_eq!(rpc(&rw, 11, "update_item", json!({"id": "ichabod", "fields": {"next": "x"}}))["isError"], true);

    // The change feed: from zero it lists everything in scope; later, only what changed.
    let all = http::post(&url, &ro, "/api/changes", &json!({"since": 0})).unwrap();
    assert_eq!(all["changes"].as_array().unwrap().len(), 18);
    let seq = all["seq"].as_u64().unwrap();
    rpc(&rw, 12, "update_item", json!({"id": "brain-adapter", "fields": {"next": "Draft the contract"}}));
    rpc(KEY, 13, "update_item", json!({"id": "ichabod", "fields": {"next": "Not for the notes tool"}}));
    let later = http::post(&url, &ro, "/api/changes", &json!({"since": seq})).unwrap();
    let ch = later["changes"].as_array().unwrap();
    assert_eq!(ch.len(), 1, "{later}");
    assert_eq!(ch[0]["item"]["next"], "Draft the contract");

    // Deletions in scope show up as null; the desktop gets the links too.
    let a_seq = http::sync(&a, &url, KEY).unwrap();
    assert!(a_seq.received >= 3);
    assert_eq!(a.load().unwrap().get("apa-paper-feedback").unwrap().links, vec!["Spec | https://example.org/spec".to_string()]);
    a.modify(|b| { b.remove("brain-adapter"); Ok(()) }).unwrap();
    http::sync(&a, &url, KEY).unwrap();
    let after = http::post(&url, &ro, "/api/changes", &json!({"since": later["seq"]})).unwrap();
    assert_eq!(after["changes"][0]["id"], "brain-adapter");
    assert!(after["changes"][0]["item"].is_null());

    // Revoking a key shuts it out at once.
    let id = http::post(&url, KEY, "/api/keys/list", &json!({})).unwrap()["keys"][0]["id"].as_str().unwrap().to_string();
    http::post(&url, KEY, "/api/keys/revoke", &json!({"id": id})).unwrap();
    assert!(http::post(&url, &ro, "/api/ping", &json!({})).is_err());
    assert_eq!(status(ureq::post(&format!("{url}/mcp")).set("Authorization", &format!("Bearer {ro}")).send_json(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))), 401);
}
