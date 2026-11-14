//! The board lives in one JSON file. Every write takes an exclusive lock on a
//! sibling lock file, re-reads the board, applies the change and replaces the
//! file atomically, so the app and any number of MCP server processes can
//! share it safely. The first write of each day also copies the previous
//! file into the backups folder.

use crate::model::Item;
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const BOARD_FILE: &str = "board.json";
const BACKUPS_KEEP: usize = 30;

#[derive(Clone, Debug, Default)]
pub struct Board {
    pub projects: Vec<Item>,
}

impl Board {
    pub fn get(&self, id: &str) -> Option<&Item> {
        self.projects.iter().find(|p| p.id == id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Item> {
        self.projects.iter_mut().find(|p| p.id == id)
    }
    pub fn index(&self, id: &str) -> Option<usize> {
        self.projects.iter().position(|p| p.id == id)
    }
    /// Insert or replace by id.
    pub fn put(&mut self, item: Item) {
        match self.index(&item.id) {
            Some(i) => self.projects[i] = item,
            None => self.projects.push(item),
        }
    }
    pub fn remove(&mut self, id: &str) -> Option<Item> {
        self.index(id).map(|i| self.projects.remove(i))
    }
    pub fn unique_id(&self, base: &str) -> String {
        if self.get(base).is_none() {
            return base.to_string();
        }
        (2..).map(|n| format!("{base}-{n}")).find(|c| self.get(c).is_none()).unwrap()
    }

    /// Parse a board, seed or export file: `{ "projects": [...] }` or a bare array.
    pub fn from_json(v: &Value) -> Result<Board, String> {
        let arr = match v {
            Value::Array(a) => a,
            Value::Object(o) => match o.get("projects") {
                Some(Value::Array(a)) => a,
                _ => return Err("The file has no \"projects\" list.".into()),
            },
            _ => return Err("The file isn't a board file.".into()),
        };
        let mut b = Board::default();
        for p in arr {
            if !p.is_object() {
                continue;
            }
            let raw = p.get("id").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let base = if raw.is_empty() { crate::model::slug(p.get("title").and_then(Value::as_str).unwrap_or("project")) } else { raw.to_string() };
            let id = b.unique_id(&base);
            b.projects.push(Item::from_value(&id, p));
        }
        Ok(b)
    }

    pub fn to_json(&self, stamp_key: &str) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("format".into(), json!("production-board"));
        m.insert("version".into(), json!(1));
        m.insert(stamp_key.into(), json!(crate::today()));
        m.insert("projects".into(), json!(self.projects));
        Value::Object(m)
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    /// `PB_DATA_DIR` overrides the folder (used by tests); otherwise the
    /// board lives in Documents\Production Board.
    pub fn default_dir() -> PathBuf {
        if let Ok(d) = std::env::var("PB_DATA_DIR") {
            if !d.trim().is_empty() {
                return PathBuf::from(d);
            }
        }
        dirs::document_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))).join("Production Board")
    }
    pub fn open_default() -> Store {
        Store { dir: Self::default_dir() }
    }
    pub fn board_path(&self) -> PathBuf {
        self.dir.join(BOARD_FILE)
    }
    pub fn backups_dir(&self) -> PathBuf {
        self.dir.join("backups")
    }
    fn lock_path(&self) -> PathBuf {
        self.dir.join(".board.lock")
    }
    pub fn exists(&self) -> bool {
        self.board_path().is_file()
    }

    pub fn load(&self) -> Result<Board, String> {
        let p = self.board_path();
        if !p.exists() {
            return Ok(Board::default());
        }
        read_board_file(&p)
    }

    fn lock(&self) -> Result<File, String> {
        fs::create_dir_all(&self.dir).map_err(|e| format!("Couldn't create {}: {e}", self.dir.display()))?;
        let f = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(self.lock_path()).map_err(|e| format!("Couldn't open the lock file: {e}"))?;
        f.lock().map_err(|e| format!("Couldn't lock the board: {e}"))?;
        Ok(f)
    }

    /// Lock, re-read, change, save. The closure's result is returned.
    pub fn modify<R>(&self, f: impl FnOnce(&mut Board) -> Result<R, String>) -> Result<R, String> {
        let _guard = self.lock()?;
        let mut board = self.load()?;
        let out = f(&mut board)?;
        self.write_locked(&board)?;
        Ok(out)
    }

    /// Like `modify`, but only writes when the board actually changed, so a
    /// background sync that finds nothing new doesn't touch the file.
    pub fn modify_if_changed<R>(&self, f: impl FnOnce(&mut Board) -> Result<R, String>) -> Result<R, String> {
        let _guard = self.lock()?;
        let mut board = self.load()?;
        let before = board.projects.clone();
        let existed = self.exists();
        let out = f(&mut board)?;
        if board.projects != before || (!existed && !board.projects.is_empty()) {
            self.write_locked(&board)?;
        }
        Ok(out)
    }

    /// Run `f` while holding the board lock, without writing the board.
    pub fn with_lock<R>(&self, f: impl FnOnce() -> Result<R, String>) -> Result<R, String> {
        let _guard = self.lock()?;
        f()
    }

    /// Create an empty board file if there is none.
    pub fn create_empty(&self) -> Result<(), String> {
        let _guard = self.lock()?;
        if !self.exists() {
            self.write_locked(&Board::default())?;
        }
        Ok(())
    }

    fn write_locked(&self, board: &Board) -> Result<(), String> {
        let path = self.board_path();
        self.daily_backup(&path);
        let text = serde_json::to_string_pretty(&board.to_json("saved")).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!("{BOARD_FILE}.tmp"));
        {
            let mut f = File::create(&tmp).map_err(|e| format!("Couldn't write the board: {e}"))?;
            f.write_all(text.as_bytes()).map_err(|e| format!("Couldn't write the board: {e}"))?;
            f.sync_all().ok();
        }
        // Replacing can briefly fail if a sync or virus scanner has the file open.
        let mut last = None;
        for _ in 0..20 {
            match fs::rename(&tmp, &path) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        Err(format!("Couldn't save the board: {}", last.map(|e| e.to_string()).unwrap_or_default()))
    }

    fn daily_backup(&self, path: &Path) {
        if !path.exists() {
            return;
        }
        let dir = self.backups_dir();
        let target = dir.join(format!("board-{}.json", crate::today()));
        if target.exists() {
            return;
        }
        if fs::create_dir_all(&dir).is_ok() && fs::copy(path, &target).is_ok() {
            if let Ok(rd) = fs::read_dir(&dir) {
                let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("board-") && n.ends_with(".json"))).collect();
                files.sort();
                while files.len() > BACKUPS_KEEP {
                    let _ = fs::remove_file(files.remove(0));
                }
            }
        }
    }

    pub fn modified_time(&self) -> Option<std::time::SystemTime> {
        fs::metadata(self.board_path()).and_then(|m| m.modified()).ok()
    }
}

pub fn read_board_file(p: &Path) -> Result<Board, String> {
    let text = fs::read_to_string(p).map_err(|e| format!("Couldn't read {}: {e}", p.display()))?;
    let text = text.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(text).map_err(|e| format!("{} isn't valid JSON: {e}", p.display()))?;
    Board::from_json(&v)
}
