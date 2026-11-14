//! Board tools shared by the MCP server and the built-in assistant.
//!
//! In direct mode a write goes straight to the board file. In staged mode
//! (used by the assistant panel) writes are applied to an in-memory copy so
//! later reads see them, and recorded as pending changes for the app to apply
//! or put in front of the user for confirmation.

use crate::lists;
use crate::model::{self, Item, TRACKS};
use crate::ops::{self, Op, Pending};
use crate::store::{Board, Store};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::PathBuf;

/// What a caller may see and do. The owner's own sessions have full access;
/// an access key made for another tool can be limited to reading and to
/// some tracks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scope {
    pub read_only: bool,
    /// None means every track.
    pub tracks: Option<Vec<String>>,
}

impl Scope {
    pub fn full() -> Scope {
        Scope::default()
    }
    pub fn is_full(&self) -> bool {
        !self.read_only && self.tracks.is_none()
    }
    pub fn allows(&self, track: &str) -> bool {
        self.tracks.as_ref().is_none_or(|t| t.iter().any(|x| x == track))
    }
    fn check_track(&self, track: &str) -> Result<(), String> {
        if self.allows(track) {
            Ok(())
        } else {
            Err(format!("This access key doesn't cover the {} track.", model::track(track).map(|t| t.label).unwrap_or(track)))
        }
    }
}

/// Tools that only read the board.
pub const READ_TOOLS: &[&str] = &["board_summary", "list_items", "get_item", "list_tracks"];

pub struct ToolDef {
    pub name: &'static str,
    pub description: String,
    pub schema: Value,
}

fn track_keys() -> Vec<&'static str> {
    TRACKS.iter().map(|t| t.key).collect()
}

fn field_props() -> Value {
    json!({
        "title": {"type": "string"},
        "kind": {"type": "string", "description": "What it is, e.g. \"M/M hockey romance\" or \"desktop tool\""},
        "byline": {"type": "string", "description": "Pen name or author name it is published under"},
        "words": {"type": "string", "description": "Length in words, free text, e.g. \"68k words, 27 chapters\""},
        "status": {"type": "string", "description": "Where it stands, one or two sentences"},
        "next": {"type": "string", "description": "The next concrete step"},
        "open": {"type": "array", "items": {"type": "string"}, "description": "Decisions waiting on the owner. Replaces the whole list."},
        "decisions": {"type": "array", "items": {"type": "string"}, "description": "Standing decisions a new work session must not reopen. Replaces the whole list."},
        "files": {"type": "string", "description": "Where the files live: folder path and which computer"},
        "links": {"type": "array", "items": {"type": "string"}, "description": "Pointers to related records elsewhere (notes, decisions, sources, repositories), one per entry: a web address or \"label | address\". Replaces the whole list."},
        "dueLabel": {"type": "string", "description": "What the due date is for, e.g. \"Release day\""},
        "dueDate": {"type": "string", "description": "Due date as YYYY-MM-DD, or \"\" to clear"},
        "touched": {"type": "string", "description": "Date last worked on, YYYY-MM-DD"}
    })
}

pub fn tool_defs(staged: bool) -> Vec<ToolDef> {
    let tracks = TRACKS.iter().map(|t| format!("{} ({}): {}", t.key, t.label, t.stages.iter().map(|s| s.0).collect::<Vec<_>>().join(" > "))).collect::<Vec<_>>().join("; ");
    let mut update_props = field_props();
    let up = update_props.as_object_mut().unwrap();
    up.insert("add_open".into(), json!({"type": "array", "items": {"type": "string"}, "description": "Append decisions waiting on the owner"}));
    up.insert("resolve_open".into(), json!({"type": "array", "items": {"type": "string"}, "description": "Remove open decisions that have been settled (exact or unique partial text). Usually also add_decisions with the outcome."}));
    up.insert("add_decisions".into(), json!({"type": "array", "items": {"type": "string"}, "description": "Append standing decisions"}));
    up.insert("add_links".into(), json!({"type": "array", "items": {"type": "string"}, "description": "Append links (duplicates are skipped)"}));
    up.insert("remove_links".into(), json!({"type": "array", "items": {"type": "string"}, "description": "Remove links (exact or unique partial text)"}));

    let mut add_props = field_props();
    let ap = add_props.as_object_mut().unwrap();
    ap.insert("track".into(), json!({"type": "string", "enum": track_keys()}));
    ap.insert("stage".into(), json!({"type": "string", "description": "Stage key or label within the track; defaults to the first stage"}));

    let mut v = vec![
        ToolDef {
            name: "board_summary",
            description: "Overview of the board: how many items are in flight, how many decisions wait on the owner, and the three watch lists (closest to finished, due dates, gone cold). Start here for questions like what to pick up next.".into(),
            schema: json!({"type": "object", "properties": {}}),
        },
        ToolDef {
            name: "list_items",
            description: "List items on the board in compact form, optionally filtered by track, stage or a search term. Finished items are left out unless include_finished is true.".into(),
            schema: json!({"type": "object", "properties": {
                "track": {"type": "string", "enum": track_keys()},
                "stage": {"type": "string", "description": "Stage key or label within the track"},
                "query": {"type": "string", "description": "Case-insensitive text matched against title, kind, byline, status and next step"},
                "include_finished": {"type": "boolean"}
            }}),
        },
        ToolDef {
            name: "get_item",
            description: "Full record for one item, including open decisions, standing decisions, file location and recent activity.".into(),
            schema: json!({"type": "object", "properties": {"id": {"type": "string", "description": "Item id, or its exact title"}}, "required": ["id"]}),
        },
        ToolDef {
            name: "update_item",
            description: "Change fields on one item. Only the fields given change. To change stage use move_item. To record that work happened use log_activity.".into(),
            schema: json!({"type": "object", "properties": {"id": {"type": "string"}, "fields": {"type": "object", "properties": update_props, "additionalProperties": false}}, "required": ["id", "fields"]}),
        },
        ToolDef {
            name: "move_item",
            description: format!("Move an item to another stage, and optionally to another track. Stamps the item as worked on today and logs the move. Tracks and stages: {tracks}"),
            schema: json!({"type": "object", "properties": {
                "id": {"type": "string"},
                "stage": {"type": "string", "description": "Stage key or label"},
                "track": {"type": "string", "enum": track_keys(), "description": "Only when moving to another track"}
            }, "required": ["id", "stage"]}),
        },
        ToolDef {
            name: "add_item",
            description: "Put a new item on the board. Social media chores go in the social track, one item per post or chore.".into(),
            schema: json!({"type": "object", "properties": add_props, "required": ["title", "track"]}),
        },
        ToolDef {
            name: "log_activity",
            description: "Record work done on an item: adds a line to its activity log, stamps the date it was last worked on, and can update its next step and where it stands. Call this at the end of a work session on a project.".into(),
            schema: json!({"type": "object", "properties": {
                "id": {"type": "string"},
                "text": {"type": "string", "description": "What was done, one short line"},
                "date": {"type": "string", "description": "YYYY-MM-DD; defaults to today"},
                "next": {"type": "string", "description": "New next step"},
                "status": {"type": "string", "description": "New \"where it stands\""}
            }, "required": ["id", "text"]}),
        },
        ToolDef {
            name: "list_tracks",
            description: "The tracks and their stages, in order.".into(),
            schema: json!({"type": "object", "properties": {}}),
        },
    ];
    if staged {
        v.push(ToolDef {
            name: "remove_item",
            description: "Take an item off the board. The user always confirms removals before they happen.".into(),
            schema: json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        });
    }
    v
}

pub struct StagedState {
    pub board: Board,
    pub pending: Vec<Pending>,
    sink: Option<PathBuf>,
}

pub struct Ctx {
    pub store: Store,
    pub staged: Option<StagedState>,
    pub scope: Scope,
}

impl Ctx {
    pub fn direct(store: Store) -> Ctx {
        Ctx { store, staged: None, scope: Scope::full() }
    }
    pub fn scoped(store: Store, scope: Scope) -> Ctx {
        Ctx { store, staged: None, scope }
    }
    pub fn staged(store: Store, sink: Option<PathBuf>) -> Result<Ctx, String> {
        let board = store.load()?;
        Ok(Ctx { store, staged: Some(StagedState { board, pending: vec![], sink }), scope: Scope::full() })
    }

    /// The board as this caller may see it.
    fn board(&self) -> Result<Board, String> {
        let mut b = match &self.staged {
            Some(s) => s.board.clone(),
            None => self.store.load()?,
        };
        b.projects.retain(|p| self.scope.allows(&p.track));
        Ok(b)
    }

    /// Resolve an id or title among the items this caller may see.
    fn resolve(&self, key: &str) -> Result<String, String> {
        ops::resolve(&self.board()?, key)
    }

    fn write(&mut self, make: impl Fn(&Board) -> Result<Op, String>) -> Result<String, String> {
        let today = crate::today();
        match &mut self.staged {
            None => self.store.modify(|b| {
                let op = make(b)?;
                op.apply(b, &today)
            }),
            Some(st) => {
                let op = make(&st.board)?;
                let summary = op.apply(&mut st.board, &today)?;
                let p = Pending { op, summary: summary.clone() };
                if let Some(path) = &st.sink {
                    let line = serde_json::to_string(&p).map_err(|e| e.to_string())?;
                    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("Couldn't record the change: {e}"))?;
                    writeln!(f, "{line}").map_err(|e| e.to_string())?;
                }
                st.pending.push(p);
                Ok(format!("{summary}\n(Staged: the board app applies this when your turn ends; removals and changes to more than three items wait for the user's confirmation.)"))
            }
        }
    }

    /// Run one tool. Ok is the text result; Err is an error message for the caller.
    pub fn call(&mut self, name: &str, args: &Value) -> Result<String, String> {
        let a = args.as_object().cloned().unwrap_or_default();
        let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
        if self.scope.read_only && !READ_TOOLS.contains(&name) {
            return Err("This access key can only read the board.".into());
        }
        match name {
            "board_summary" => Ok(summary_text(&self.board()?)),
            "list_tracks" => {
                let mut v = model::tracks_json();
                if let Some(a) = v.as_array_mut() {
                    a.retain(|t| self.scope.allows(t["key"].as_str().unwrap_or("")));
                }
                Ok(serde_json::to_string_pretty(&v).unwrap())
            }
            "list_items" => {
                let b = self.board()?;
                let track = s("track").map(|t| model::find_track(&t).map(|t| t.key.to_string()).ok_or(format!("Unknown track \"{t}\"."))).transpose()?;
                let stage = s("stage");
                let q = s("query").map(|q| q.to_lowercase()).filter(|q| !q.is_empty());
                let fin = a.get("include_finished").and_then(Value::as_bool).unwrap_or(false);
                let rows: Vec<Value> = b
                    .projects
                    .iter()
                    .filter(|p| track.as_ref().is_none_or(|t| &p.track == t))
                    .filter(|p| stage.as_ref().is_none_or(|st| p.track_def().find_stage(st) == Some(p.stage.as_str())))
                    .filter(|p| fin || !p.is_final())
                    .filter(|p| q.as_ref().is_none_or(|q| [&p.title, &p.kind, &p.byline, &p.status, &p.next].iter().any(|f| f.to_lowercase().contains(q.as_str()))))
                    .map(compact)
                    .collect();
                Ok(serde_json::to_string(&json!({"count": rows.len(), "items": rows})).unwrap())
            }
            "get_item" => {
                let b = self.board()?;
                let id = ops::resolve(&b, &s("id").unwrap_or_default())?;
                Ok(serde_json::to_string_pretty(&full(b.get(&id).unwrap())).unwrap())
            }
            "update_item" => {
                let id = self.resolve(&s("id").unwrap_or_default())?;
                let fields = a.get("fields").and_then(Value::as_object).cloned().ok_or("update_item needs a fields object.")?;
                if let Some(t) = fields.get("track").and_then(Value::as_str) {
                    self.scope.check_track(model::find_track(t).map(|t| t.key).unwrap_or(t))?;
                }
                self.write(|_| Ok(Op::Update { id: id.clone(), fields: fields.clone() }))
            }
            "move_item" => {
                let id = self.resolve(&s("id").unwrap_or_default())?;
                let stage = s("stage").ok_or("move_item needs a stage.")?;
                let track = s("track");
                if let Some(t) = track.as_deref().filter(|t| !t.trim().is_empty()) {
                    self.scope.check_track(model::find_track(t).map(|t| t.key).unwrap_or(t))?;
                }
                self.write(|_| Ok(Op::Move { id: id.clone(), track: track.clone(), stage: stage.clone() }))
            }
            "add_item" => {
                let today = crate::today();
                let track = match s("track") {
                    Some(t) => model::find_track(&t).map(|t| t.key).unwrap_or(""),
                    None => TRACKS[0].key,
                };
                if !track.is_empty() {
                    self.scope.check_track(track)?;
                }
                self.write(|b| Ok(Op::Add { item: ops::new_item(b, &a, &today)? }))
            }
            "log_activity" => {
                let id = self.resolve(&s("id").unwrap_or_default())?;
                let text = s("text").ok_or("log_activity needs text.")?;
                let (date, next, status) = (s("date"), s("next"), s("status"));
                self.write(|_| Ok(Op::Log { id: id.clone(), text: text.clone(), date: date.clone(), next: next.clone(), status: status.clone() }))
            }
            "remove_item" if self.staged.is_some() => {
                let id = self.resolve(&s("id").unwrap_or_default())?;
                self.write(|_| Ok(Op::Remove { id: id.clone() }))
            }
            other => Err(format!("Unknown tool \"{other}\".")),
        }
    }
}

fn compact(p: &Item) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(p.id));
    m.insert("title".into(), json!(p.title));
    m.insert("track".into(), json!(p.track));
    m.insert("stage".into(), json!(p.stage_label()));
    for (k, v) in [("byline", &p.byline), ("next", &p.next), ("touched", &p.touched), ("dueDate", &p.due_date), ("dueLabel", &p.due_label)] {
        if !v.is_empty() {
            m.insert(k.into(), json!(v));
        }
    }
    if !p.open.is_empty() {
        m.insert("openDecisions".into(), json!(p.open.len()));
    }
    Value::Object(m)
}

fn full(p: &Item) -> Value {
    let mut v = serde_json::to_value(p).unwrap();
    let o = v.as_object_mut().unwrap();
    o.insert("trackLabel".into(), json!(p.track_def().label));
    o.insert("stageLabel".into(), json!(p.stage_label()));
    o.insert("finished".into(), json!(p.is_final()));
    if let Some(d) = lists::days_since(&p.touched, crate::today_date()) {
        o.insert("daysSinceWorkedOn".into(), json!(d));
    }
    v
}

pub fn summary_text(b: &Board) -> String {
    let s = lists::summary(b, crate::today_date());
    let title = |id: &str| b.get(id).map(|p| format!("{} [{}; {} / {}]", p.title, p.id, p.track_def().short, p.stage_label())).unwrap_or_else(|| id.to_string());
    let mut out = format!("Today is {}. {} items in flight; {} decisions waiting on the owner.\n", crate::today(), s.in_flight, s.decisions_waiting);
    out.push_str("\nClosest to finished (books one or two stages from published):\n");
    for r in &s.closest {
        let p = b.get(&r.id).unwrap();
        out.push_str(&format!("- {}{}\n", title(&r.id), if p.open.is_empty() { String::new() } else { format!(", {} to decide", p.open.len()) }));
    }
    out.push_str("\nDue dates:\n");
    for r in &s.due {
        let p = b.get(&r.id).unwrap();
        let when = match r.days {
            None => "no date".to_string(),
            Some(d) if d > 0 => format!("{d} days past"),
            Some(0) => "today".to_string(),
            Some(d) => format!("in {} days", -d),
        };
        out.push_str(&format!("- {}: {} ({when})\n", title(&r.id), if p.due_label.is_empty() { &p.due_date } else { &p.due_label }));
    }
    out.push_str("\nGone cold (not logged in 45 days or more):\n");
    for r in &s.cold {
        out.push_str(&format!("- {}: {} days\n", title(&r.id), r.days.unwrap_or(0)));
    }
    if s.closest.is_empty() && s.due.is_empty() && s.cold.is_empty() {
        out.push_str("(all three lists are empty)\n");
    }
    out
}
