//! App settings (not the board). Kept in the per-user app config folder.
//! The model API key never goes here: it lives in the system's credential store (Credential Manager on Windows, the keychain on a Mac).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
const KEY_SERVICE: &str = "Production Board";
const KEY_USER: &str = "model-api-key";
const SYNC_USER: &str = "sync-access-key";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// "off", "agent" (installed command-line coding agent) or "api" (own API key).
    pub mode: String,
    /// Full path to the coding agent's executable. Empty means find it automatically.
    pub agent_path: String,
    pub model: String,
    pub effort: String,
    /// Address of the board server to sync with. Empty means sync is off.
    pub sync_server: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mode: "off".into(), agent_path: String::new(), model: DEFAULT_MODEL.into(), effort: "medium".into(), sync_server: String::new() }
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("PB_CONFIG_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("Production Board")
}

fn path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(s: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(config_dir()).map_err(|e| e.to_string())?;
    std::fs::write(path(), serde_json::to_string_pretty(s).unwrap()).map_err(|e| format!("Couldn't save settings: {e}"))
}

fn entry_for(user: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEY_SERVICE, user).map_err(|e| format!("The credential store isn't available: {e}"))
}

fn entry() -> Result<keyring::Entry, String> {
    entry_for(KEY_USER)
}

pub fn set_sync_key(key: &str) -> Result<(), String> {
    entry_for(SYNC_USER)?.set_password(key.trim()).map_err(|e| format!("Couldn't store the access key: {e}"))
}

pub fn get_sync_key() -> Option<String> {
    entry_for(SYNC_USER).ok()?.get_password().ok().filter(|k| !k.trim().is_empty())
}

pub fn clear_sync_key() -> Result<(), String> {
    match entry_for(SYNC_USER)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Couldn't remove the access key: {e}")),
    }
}

/// The server and key, when sync is set up.
pub fn sync_config() -> Option<(String, String)> {
    let s = load();
    let server = s.sync_server.trim().to_string();
    if server.is_empty() {
        return None;
    }
    get_sync_key().map(|k| (server, k))
}

pub fn set_key(key: &str) -> Result<(), String> {
    entry()?.set_password(key.trim()).map_err(|e| format!("Couldn't store the key: {e}"))
}

pub fn get_key() -> Option<String> {
    entry().ok()?.get_password().ok().filter(|k| !k.trim().is_empty())
}

pub fn clear_key() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Couldn't remove the key: {e}")),
    }
}
