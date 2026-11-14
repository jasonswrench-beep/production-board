//! Drives the real executable as an MCP server over stdio.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Server {
    fn start(dir: &Path, extra: &[&str]) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_production-board"))
            .arg("--mcp")
            .arg("--data-dir")
            .arg(dir)
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the MCP server");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut s = Server { child, stdin, stdout, next: 1 };
        let init = s.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "production-board");
        s.notify("notifications/initialized");
        s
    }
    fn notify(&mut self, method: &str) {
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "method": method})).unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"));
        assert_eq!(v["id"], id);
        v
    }
    /// Call a tool; returns (text, isError).
    fn call(&mut self, name: &str, args: Value) -> (String, bool) {
        let v = self.request("tools/call", json!({"name": name, "arguments": args}));
        (v["result"]["content"][0]["text"].as_str().unwrap().to_string(), v["result"]["isError"].as_bool().unwrap())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn temp_board(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pb-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/seed.json"), dir.join("board.json")).unwrap();
    dir
}

fn on_disk(dir: &Path, id: &str) -> Value {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("board.json")).unwrap()).unwrap();
    v["projects"].as_array().unwrap().iter().find(|p| p["id"] == id).cloned().unwrap()
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[test]
fn lists_items_and_moves_one() {
    let dir = temp_board("move");
    let mut s = Server::start(&dir, &[]);

    let tools = s.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for t in ["list_items", "get_item", "update_item", "move_item", "add_item", "log_activity", "board_summary"] {
        assert!(names.contains(&t), "missing tool {t}");
    }
    assert!(!names.contains(&"remove_item"), "removal is only offered to the assistant panel");

    let (text, err) = s.call("list_items", json!({"track": "fiction", "stage": "Revising"}));
    assert!(!err, "{text}");
    let listed: Value = serde_json::from_str(&text).unwrap();
    let ids: Vec<&str> = listed["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"ichabod"));
    assert_eq!(listed["count"].as_u64().unwrap() as usize, ids.len());

    let (text, err) = s.call("move_item", json!({"id": "ichabod", "stage": "Editing"}));
    assert!(!err, "{text}");
    assert!(text.contains("Revising to Editing"), "{text}");

    let it = on_disk(&dir, "ichabod");
    assert_eq!(it["stage"], "edit");
    assert_eq!(it["touched"], today());
    assert_eq!(it["log"].as_array().unwrap().last().unwrap()["t"], "Stage: Revising to Editing");
    // Everything else about the record is unchanged.
    assert_eq!(it["decisions"].as_array().unwrap().len(), 4);

    // Moving across tracks, by title rather than id.
    let (text, err) = s.call("move_item", json!({"id": "Ten Years", "track": "social", "stage": "todo"}));
    assert!(!err, "{text}");
    assert_eq!(on_disk(&dir, "ten-years")["track"], "social");

    // Bad input comes back as a tool error, not a crash.
    let (text, err) = s.call("move_item", json!({"id": "ichabod", "stage": "Teleported"}));
    assert!(err);
    assert!(text.contains("isn't a stage"));
}

#[test]
fn logs_activity_adds_and_updates() {
    let dir = temp_board("log");
    let mut s = Server::start(&dir, &[]);
    let (t, e) = s.call("log_activity", json!({"id": "six-packed", "text": "Phase 1 Batch 1 done", "next": "Run Batch 2"}));
    assert!(!e, "{t}");
    let it = on_disk(&dir, "six-packed");
    assert_eq!(it["next"], "Run Batch 2");
    assert_eq!(it["touched"], today());

    let (t, e) = s.call("update_item", json!({"id": "six-packed", "fields": {"resolve_open": ["Worlds or Olympics"], "add_decisions": ["The event is the Worlds"]}}));
    assert!(!e, "{t}");
    let it = on_disk(&dir, "six-packed");
    assert_eq!(it["open"].as_array().unwrap().len(), 3);
    assert!(it["decisions"].as_array().unwrap().iter().any(|d| d == "The event is the Worlds"));

    let (t, e) = s.call("add_item", json!({"title": "Christmas Plus-One launch post", "track": "social", "next": "Draft the copy"}));
    assert!(!e, "{t}");
    let it = on_disk(&dir, "christmas-plus-one-launch-post");
    assert_eq!(it["stage"], "todo");
    assert_eq!(it["log"][0]["t"], "Added to the board");
}

#[test]
fn staged_mode_records_changes_without_writing() {
    let dir = temp_board("staged");
    let stage = dir.join("staged.jsonl");
    let before = std::fs::read_to_string(dir.join("board.json")).unwrap();
    let mut s = Server::start(&dir, &["--stage-to", stage.to_str().unwrap()]);

    let (t, e) = s.call("move_item", json!({"id": "ichabod", "stage": "edit"}));
    assert!(!e, "{t}");
    // Later reads in the same session see the staged change.
    let (t, _) = s.call("get_item", json!({"id": "ichabod"}));
    assert!(t.contains("\"stageLabel\": \"Editing\""), "{t}");
    let (t, e) = s.call("remove_item", json!({"id": "pyteran"}));
    assert!(!e, "{t}");
    drop(s);

    assert_eq!(std::fs::read_to_string(dir.join("board.json")).unwrap(), before, "staged mode must not touch the board");
    let lines: Vec<Value> = std::fs::read_to_string(&stage).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["op"], "move");
    assert_eq!(lines[1]["op"], "remove");
}
