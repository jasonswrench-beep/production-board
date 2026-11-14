//! The board commands behind the board's own screens, shared by the desktop
//! window and the web page so both behave identically.

use crate::model::{self, Item};
use crate::ops::Op;
use crate::store::Store;
use crate::{lists, today, today_date};
use serde_json::{json, Value};

pub fn board_payload(store: &Store) -> Result<Value, String> {
    let b = store.load()?;
    let summary = lists::summary(&b, today_date());
    Ok(json!({ "projects": b.projects, "summary": summary, "exists": store.exists(), "today": today() }))
}

/// Save an item from the edit dialog. The dialog sends the whole record.
pub fn save_item(store: &Store, item: &Value, is_new: bool) -> Result<String, String> {
    store.modify(|b| {
        let mut id = item.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if is_new || id.is_empty() {
            let base = model::slug(item.get("title").and_then(Value::as_str).unwrap_or("project"));
            id = b.unique_id(&base);
        } else if b.get(&id).is_none() {
            return Err("That item is no longer on the board.".into());
        }
        b.put(Item::from_value(&id, item));
        Ok(id)
    })
}

pub fn move_item(store: &Store, id: &str, track: &str, stage: &str) -> Result<(), String> {
    store.modify(|b| Op::Move { id: id.to_string(), track: Some(track.to_string()), stage: stage.to_string() }.apply(b, &today()).map(|_| ()))
}

pub fn delete_item(store: &Store, id: &str) -> Result<(), String> {
    store.modify(|b| b.remove(id).map(|_| ()).ok_or_else(|| "That item is no longer on the board.".to_string()))
}

pub fn export_json(store: &Store) -> Result<String, String> {
    let b = store.load()?;
    Ok(serde_json::to_string_pretty(&b.to_json("exported")).unwrap())
}
