//! Changes to the board. The MCP server, the assistant panel and the board
//! view all go through these, so a move logs the same way wherever it comes from.

use crate::model::{self, find_track, str_list, Item};
use crate::store::Board;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Update { id: String, fields: Map<String, Value> },
    Move { id: String, #[serde(default)] track: Option<String>, stage: String },
    Add { item: Item },
    Log { id: String, text: String, #[serde(default)] date: Option<String>, #[serde(default)] next: Option<String>, #[serde(default)] status: Option<String> },
    Remove { id: String },
}

/// A change waiting to be applied, with a one-line description for people.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Pending {
    #[serde(flatten)]
    pub op: Op,
    pub summary: String,
}

impl Op {
    pub fn target(&self) -> &str {
        match self {
            Op::Update { id, .. } | Op::Move { id, .. } | Op::Log { id, .. } | Op::Remove { id } => id,
            Op::Add { item } => &item.id,
        }
    }
}

/// Find an item by id, then by exact title, then by a unique partial title.
pub fn resolve(board: &Board, key: &str) -> Result<String, String> {
    let k = key.trim();
    if k.is_empty() {
        return Err("No item named. Pass its id (from list_items).".into());
    }
    if board.get(k).is_some() {
        return Ok(k.to_string());
    }
    let lk = k.to_lowercase();
    if let Some(p) = board.projects.iter().find(|p| p.title.to_lowercase() == lk) {
        return Ok(p.id.clone());
    }
    let hits: Vec<&Item> = board.projects.iter().filter(|p| p.title.to_lowercase().contains(&lk) || p.id.contains(&lk)).collect();
    match hits.len() {
        1 => Ok(hits[0].id.clone()),
        0 => Err(format!("No item matches \"{k}\". Use list_items to find its id.")),
        _ => Err(format!(
            "\"{k}\" matches several items: {}. Pass the id.",
            hits.iter().take(8).map(|p| format!("{} ({})", p.title, p.id)).collect::<Vec<_>>().join("; ")
        )),
    }
}

const STRING_FIELDS: &[&str] = &["title", "kind", "byline", "words", "status", "next", "files", "dueLabel", "dueDate", "touched"];

fn set_string(it: &mut Item, k: &str, v: String) {
    match k {
        "title" => it.title = v,
        "kind" => it.kind = v,
        "byline" => it.byline = v,
        "words" => it.words = v,
        "status" => it.status = v,
        "next" => it.next = v,
        "files" => it.files = v,
        "dueLabel" => it.due_label = v,
        "dueDate" => it.due_date = v,
        "touched" => it.touched = v,
        _ => {}
    }
}

fn check_date(k: &str, v: &str) -> Result<(), String> {
    if (k == "dueDate" || k == "touched") && !v.is_empty() && !model::is_iso_date(v) {
        return Err(format!("{k} must be a YYYY-MM-DD date or an empty string."));
    }
    Ok(())
}

/// Accepts camelCase or snake_case names for the two date-and-label fields.
fn canon(k: &str) -> &str {
    match k {
        "due_label" => "dueLabel",
        "due_date" => "dueDate",
        other => other,
    }
}

fn lower_eq(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Apply field changes. Returns the names of the fields that changed.
fn apply_fields(it: &mut Item, fields: &Map<String, Value>) -> Result<Vec<String>, String> {
    let mut changed = vec![];
    for (raw, v) in fields {
        let k = canon(raw);
        if STRING_FIELDS.contains(&k) {
            let s = match v {
                Value::String(s) => s.trim().to_string(),
                Value::Null => String::new(),
                other => return Err(format!("{k} must be a string, got {other}")),
            };
            check_date(k, &s)?;
            if k == "title" && s.is_empty() {
                return Err("title can't be empty.".into());
            }
            set_string(it, k, s);
            changed.push(k.to_string());
        } else {
            match k {
                "open" => {
                    it.open = str_list(Some(v));
                    changed.push("open".into());
                }
                "decisions" => {
                    it.decisions = str_list(Some(v));
                    changed.push("decisions".into());
                }
                "add_open" => {
                    it.open.extend(str_list(Some(v)));
                    changed.push("open".into());
                }
                "add_decisions" => {
                    it.decisions.extend(str_list(Some(v)));
                    changed.push("decisions".into());
                }
                "links" => {
                    it.links = str_list(Some(v));
                    changed.push("links".into());
                }
                "add_links" => {
                    for l in str_list(Some(v)) {
                        if !it.links.contains(&l) {
                            it.links.push(l);
                        }
                    }
                    changed.push("links".into());
                }
                "remove_links" => {
                    for r in str_list(Some(v)) {
                        let before = it.links.len();
                        it.links.retain(|l| l != &r);
                        if it.links.len() == before {
                            let hits: Vec<usize> = it.links.iter().enumerate().filter(|(_, l)| l.contains(r.as_str())).map(|(i, _)| i).collect();
                            if hits.len() == 1 {
                                it.links.remove(hits[0]);
                            } else {
                                return Err(format!("\"{r}\" doesn't match exactly one link on {}. Links: {:?}", it.title, it.links));
                            }
                        }
                    }
                    changed.push("links".into());
                }
                "resolve_open" => {
                    for r in str_list(Some(v)) {
                        let before = it.open.len();
                        it.open.retain(|o| !lower_eq(o, &r));
                        if it.open.len() == before {
                            // Fall back to a unique partial match.
                            let lr = r.to_lowercase();
                            let hits: Vec<usize> = it.open.iter().enumerate().filter(|(_, o)| o.to_lowercase().contains(&lr)).map(|(i, _)| i).collect();
                            if hits.len() == 1 {
                                it.open.remove(hits[0]);
                            } else {
                                return Err(format!("\"{r}\" doesn't match exactly one open decision on {}. Open decisions: {:?}", it.title, it.open));
                            }
                        }
                    }
                    changed.push("open".into());
                }
                "id" | "log" => {}
                "track" | "stage" => {} // handled by the caller as a move
                other => return Err(format!("Unknown field \"{other}\".")),
            }
        }
    }
    changed.dedup();
    Ok(changed)
}

fn resolve_target(it: &Item, track: Option<&str>, stage: &str) -> Result<(&'static str, &'static str), String> {
    let t = match track.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => find_track(t).ok_or_else(|| format!("Unknown track \"{t}\". Tracks: {}", model::TRACKS.iter().map(|t| t.key).collect::<Vec<_>>().join(", ")))?,
        None => it.track_def(),
    };
    let st = t.find_stage(stage).ok_or_else(|| {
        format!("\"{stage}\" isn't a stage of {}. Stages: {}", t.label, t.stages.iter().map(|s| format!("{} ({})", s.0, s.1)).collect::<Vec<_>>().join(", "))
    })?;
    Ok((t.key, st))
}

/// Change track and/or stage the way a drag on the board does: stamp
/// `touched` with today and log the move. Returns the log line, or None
/// when nothing changed.
pub fn move_item(it: &mut Item, track: &str, stage: &str, today: &str) -> Option<String> {
    if it.track == track && it.stage == stage {
        return None;
    }
    let line = if it.track == track {
        format!("Stage: {} to {}", it.stage_label(), model::stage_label(track, stage))
    } else {
        format!("Moved to {}: {}", model::track(track).map(|t| t.label).unwrap_or(track), model::stage_label(track, stage))
    };
    it.track = track.to_string();
    it.stage = stage.to_string();
    it.touched = today.to_string();
    it.push_log(today, &line);
    Some(line)
}

/// Build a new item from fields. Requires a title; the track defaults to Fiction.
pub fn new_item(board: &Board, fields: &Map<String, Value>, today: &str) -> Result<Item, String> {
    let title = fields.get("title").and_then(Value::as_str).map(str::trim).unwrap_or("");
    if title.is_empty() {
        return Err("add_item needs a title.".into());
    }
    let track = match fields.get("track").and_then(Value::as_str) {
        Some(t) => find_track(t).ok_or_else(|| format!("Unknown track \"{t}\"."))?,
        None => &model::TRACKS[0],
    };
    let stage = match fields.get("stage").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        Some(s) => track.find_stage(s).ok_or_else(|| format!("\"{s}\" isn't a stage of {}.", track.label))?,
        None => track.stages[0].0,
    };
    let mut it = Item { id: board.unique_id(&model::slug(title)), title: title.to_string(), track: track.key.into(), stage: stage.into(), touched: today.into(), ..Default::default() };
    let rest: Map<String, Value> = fields.iter().filter(|(k, _)| !matches!(k.as_str(), "track" | "stage" | "id")).map(|(k, v)| (k.clone(), v.clone())).collect();
    apply_fields(&mut it, &rest)?;
    it.push_log(today, "Added to the board");
    it.normalize();
    Ok(it)
}

impl Op {
    /// Apply to a board. Returns a one-line description of what changed.
    pub fn apply(&self, board: &mut Board, today: &str) -> Result<String, String> {
        match self {
            Op::Update { id, fields } => {
                let it = board.get_mut(id).ok_or_else(|| format!("No item with id {id}."))?;
                let before = it.clone();
                let mut changed = apply_fields(it, fields)?;
                let track = fields.get("track").and_then(Value::as_str);
                let stage = fields.get("stage").and_then(Value::as_str);
                if track.is_some() || stage.is_some() {
                    let st = stage.map(str::to_string).unwrap_or_else(|| it.stage.clone());
                    let (tk, sk) = match resolve_target(it, track, &st) {
                        Ok(x) => x,
                        // A new track without a matching stage starts at its first stage.
                        Err(e) => match track.and_then(find_track) {
                            Some(t) if stage.is_none() => (t.key, t.stages[0].0),
                            _ => return Err(e),
                        },
                    };
                    if move_item(it, tk, sk, today).is_some() {
                        changed.push("stage".into());
                    }
                }
                if *it == before {
                    return Ok(format!("{}: no change", it.title));
                }
                it.normalize();
                Ok(format!("Update {}: {}", it.title, changed.join(", ")))
            }
            Op::Move { id, track, stage } => {
                let it = board.get_mut(id).ok_or_else(|| format!("No item with id {id}."))?;
                let (tk, sk) = resolve_target(it, track.as_deref(), stage)?;
                let title = it.title.clone();
                Ok(match move_item(it, tk, sk, today) {
                    Some(line) => format!("{title}: {line}"),
                    None => format!("{title} is already in {}", it.stage_label()),
                })
            }
            Op::Add { item } => {
                let mut it = item.clone();
                it.id = board.unique_id(&it.id);
                it.normalize();
                let line = format!("Add \"{}\" to {}: {}", it.title, it.track_def().label, it.stage_label());
                board.put(it);
                Ok(line)
            }
            Op::Log { id, text, date, next, status } => {
                let it = board.get_mut(id).ok_or_else(|| format!("No item with id {id}."))?;
                let d = date.as_deref().map(str::trim).filter(|d| !d.is_empty()).unwrap_or(today).to_string();
                if !model::is_iso_date(&d) {
                    return Err("date must be YYYY-MM-DD.".into());
                }
                let text = text.trim();
                if text.is_empty() {
                    return Err("log_activity needs text.".into());
                }
                it.push_log(&d, text);
                if it.touched.is_empty() || d > it.touched {
                    it.touched = d.clone();
                }
                if let Some(n) = next {
                    it.next = n.trim().to_string();
                }
                if let Some(s) = status {
                    it.status = s.trim().to_string();
                }
                Ok(format!("Log on {}: {text}", it.title))
            }
            Op::Remove { id } => {
                let it = board.remove(id).ok_or_else(|| format!("No item with id {id}."))?;
                Ok(format!("Remove \"{}\" from the board", it.title))
            }
        }
    }
}

/// What it takes to put the board back after a batch of changes.
pub type Snapshot = Vec<(String, Option<Item>)>;

pub struct Applied {
    pub snapshot: Snapshot,
    pub done: Vec<String>,
    pub failed: Vec<String>,
}

/// Apply changes in order, keeping the state of every touched item first.
pub fn apply_all(board: &mut Board, ops: &[Op], today: &str) -> Applied {
    let mut snapshot: Snapshot = vec![];
    let mut done = vec![];
    let mut failed = vec![];
    for op in ops {
        let ids_before: Vec<String> = board.projects.iter().map(|p| p.id.clone()).collect();
        let target = op.target().to_string();
        let prev = board.get(&target).cloned();
        match op.apply(board, today) {
            Ok(line) => {
                done.push(line);
                if let Op::Add { .. } = op {
                    // The added item may have been given a fresh id.
                    for p in &board.projects {
                        if !ids_before.contains(&p.id) && !snapshot.iter().any(|(i, _)| i == &p.id) {
                            snapshot.push((p.id.clone(), None));
                        }
                    }
                } else if !snapshot.iter().any(|(i, _)| i == &target) {
                    snapshot.push((target, prev));
                }
            }
            Err(e) => failed.push(e),
        }
    }
    Applied { snapshot, done, failed }
}

pub fn restore(board: &mut Board, snapshot: &Snapshot) {
    for (id, prev) in snapshot {
        match prev {
            Some(it) => board.put(it.clone()),
            None => {
                board.remove(id);
            }
        }
    }
}

/// Deletions, and changes touching more than three items, wait for the user.
pub fn needs_confirmation(ops: &[Op]) -> bool {
    let mut ids: Vec<&str> = ops.iter().map(Op::target).collect();
    ids.sort();
    ids.dedup();
    ops.iter().any(|o| matches!(o, Op::Remove { .. })) || ids.len() > 3
}
