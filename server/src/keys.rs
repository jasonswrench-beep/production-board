//! Extra access keys, made from the desktop app for other tools.
//!
//! The server's main key (PB_KEY_SHA256) has full access: sync, the web
//! password, and managing these keys. Each extra key is limited: it can be
//! read-only, and it can be limited to some tracks. A limited key reaches the
//! board's agent tools (/mcp), the change feed (/api/changes) and /api/ping,
//! and nothing else.
//!
//! Only the SHA-256 of each key is kept, in access-keys.json in the data
//! folder. The key itself is shown once, when it's made.

use board_core::model::{self, TRACKS};
use board_core::tools::Scope;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

pub const MAX_KEYS: usize = 50;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyEntry {
    pub id: String,
    pub name: String,
    pub sha256: String,
    /// Can add, change and move cards. False means read-only.
    pub write: bool,
    /// Track keys this key covers; empty means every track.
    pub tracks: Vec<String>,
    pub created: String,
    pub last_used: String,
}

impl KeyEntry {
    pub fn scope(&self) -> Scope {
        Scope { read_only: !self.write, tracks: if self.tracks.is_empty() { None } else { Some(self.tracks.clone()) } }
    }

    /// The entry as shown to the owner: everything but the hash.
    pub fn public(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "write": self.write, "tracks": self.tracks,
            "created": self.created, "lastUsed": self.last_used,
        })
    }
}

pub struct Keys {
    path: PathBuf,
    pub list: Vec<KeyEntry>,
}

impl Keys {
    pub fn load(dir: &std::path::Path) -> Keys {
        let path = dir.join("access-keys.json");
        let list = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Keys { path, list }
    }

    fn save(&self) -> Result<(), String> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&self.list).unwrap()).map_err(|e| format!("Couldn't save the access keys: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("Couldn't save the access keys: {e}"))
    }

    /// Find the entry whose hash matches, and note today as its last use.
    pub fn find(&mut self, hash_hex: &str, today: &str) -> Option<KeyEntry> {
        let i = self.list.iter().position(|k| crate::same(k.sha256.as_bytes(), hash_hex.as_bytes()))?;
        if self.list[i].last_used != today {
            self.list[i].last_used = today.to_string();
            let _ = self.save();
        }
        Some(self.list[i].clone())
    }

    /// Make a new key. Returns the entry and the key itself, which isn't kept.
    pub fn create(&mut self, args: &Value, today: &str) -> Result<(KeyEntry, String), String> {
        let name = args.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if name.is_empty() {
            return Err("Give the key a name, such as the tool that will use it.".into());
        }
        if name.chars().count() > 60 {
            return Err("Keep the key's name to 60 characters.".into());
        }
        if self.list.len() >= MAX_KEYS {
            return Err(format!("There are already {MAX_KEYS} access keys. Revoke one you no longer use first."));
        }
        let write = args.get("write").and_then(Value::as_bool).unwrap_or(false);
        let mut tracks = Vec::new();
        for t in args.get("tracks").and_then(Value::as_array).cloned().unwrap_or_default() {
            let t = t.as_str().unwrap_or("");
            let def = model::find_track(t).ok_or_else(|| format!("Unknown track \"{t}\". Tracks: {}", TRACKS.iter().map(|t| t.key).collect::<Vec<_>>().join(", ")))?;
            if !tracks.iter().any(|x| x == def.key) {
                tracks.push(def.key.to_string());
            }
        }
        // Every track listed is the same as no limit.
        if tracks.len() == TRACKS.len() {
            tracks.clear();
        }
        let key = format!("pbk_{}", crate::random_token());
        let entry = KeyEntry {
            id: format!("k{}", &crate::random_token()[..10]),
            name,
            sha256: crate::hex(&crate::sha256(key.as_bytes())),
            write,
            tracks,
            created: today.to_string(),
            last_used: String::new(),
        };
        self.list.push(entry.clone());
        self.save()?;
        Ok((entry, key))
    }

    pub fn revoke(&mut self, id: &str) -> Result<(), String> {
        let before = self.list.len();
        self.list.retain(|k| k.id != id);
        if self.list.len() == before {
            return Err("That access key isn't on the server.".into());
        }
        self.save()
    }
}
