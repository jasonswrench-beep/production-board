//! A small MCP server over stdio (newline-delimited JSON-RPC 2.0).
//!
//!   production-board.exe --mcp                      changes go straight to the board
//!   production-board.exe --mcp --stage-to <file>    changes are recorded for the app to apply
//!   production-board.exe --mcp --data-dir <folder>  use another board folder

use crate::store::Store;
use crate::tools::{tool_defs, Ctx, READ_TOOLS};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const PROTOCOL: &str = "2025-06-18";

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

pub fn run_from_args(args: &[String]) -> i32 {
    let store = match arg_after(args, "--data-dir") {
        Some(d) => Store { dir: PathBuf::from(d) },
        None => Store::open_default(),
    };
    let stage = arg_after(args, "--stage-to").map(PathBuf::from);
    let ctx = match stage {
        Some(p) => match Ctx::staged(store, Some(p)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        },
        None => Ctx::direct(store),
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(ctx, stdin.lock(), stdout.lock());
    0
}

pub fn serve(mut ctx: Ctx, input: impl BufRead, mut output: impl Write) {
    let staged = ctx.staged.is_some();
    for line in input.lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(batch)) => {
                let out: Vec<Value> = batch.iter().filter_map(|m| handle(&mut ctx, m, staged)).collect();
                if out.is_empty() { None } else { Some(Value::Array(out)) }
            }
            Ok(msg) => handle(&mut ctx, &msg, staged),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("Parse error: {e}")}})),
        };
        if let Some(r) = reply {
            if writeln!(output, "{}", r).is_err() || output.flush().is_err() {
                break;
            }
        }
    }
}

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub fn handle(ctx: &mut Ctx, msg: &Value, staged: bool) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = msg.get("id").cloned();
    // Notifications get no reply.
    let id = id?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL);
            ok(&id, json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "production-board", "title": "Production Board", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Tools for the owner's production board: every project in flight by track and stage. At the end of a work session on a project, call log_activity with what was done and the new next step, and move_item if its stage changed. Never reopen an item's standing decisions."
            }))
        }
        "ping" => ok(&id, json!({})),
        "tools/list" => {
            let read_only = ctx.scope.read_only;
            let tools: Vec<Value> = tool_defs(staged).into_iter().filter(|t| !read_only || READ_TOOLS.contains(&t.name)).map(|t| json!({"name": t.name, "description": t.description, "inputSchema": t.schema})).collect();
            ok(&id, json!({"tools": tools}))
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match ctx.call(name, &args) {
                Ok(text) => ok(&id, json!({"content": [{"type": "text", "text": text}], "isError": false})),
                Err(e) => ok(&id, json!({"content": [{"type": "text", "text": e}], "isError": true})),
            }
        }
        "resources/list" => ok(&id, json!({"resources": []})),
        "prompts/list" => ok(&id, json!({"prompts": []})),
        _ => err(&id, -32601, &format!("Method not found: {method}")),
    })
}
