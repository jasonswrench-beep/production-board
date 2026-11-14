//! Records, tracks and stages. Field names match the board's JSON files so a
//! seed or export file imports unchanged.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub struct Track {
    pub key: &'static str,
    pub label: &'static str,
    pub short: &'static str,
    pub stages: &'static [(&'static str, &'static str)],
}

/// Display order of the tracks on the board.
pub const TRACKS: &[Track] = &[
    Track { key: "fiction", label: "Fiction", short: "Fiction", stages: &[("plot", "Plot and outline"), ("draft", "Drafting"), ("revise", "Revising"), ("edit", "Editing"), ("prod", "Production"), ("out", "Published")] },
    Track { key: "nonfiction", label: "Craft books and textbooks", short: "Nonfiction", stages: &[("plan", "Planning"), ("draft", "Drafting"), ("revise", "Revising"), ("edit", "Editing"), ("prod", "Production"), ("out", "Published")] },
    Track { key: "products", label: "Products", short: "Product", stages: &[("idea", "Idea"), ("make", "Making"), ("pack", "Packaging"), ("sale", "On sale")] },
    Track { key: "software", label: "Software and plugins", short: "Software", stages: &[("spec", "Spec and design"), ("build", "Building"), ("working", "Working build"), ("live", "In use")] },
    Track { key: "teaching", label: "Teaching", short: "Teaching", stages: &[("plan", "Planning"), ("active", "Teaching now"), ("wrap", "Wrapping up"), ("done", "Done")] },
    Track { key: "research", label: "Research", short: "Research", stages: &[("design", "Design and approval"), ("data", "Data collection"), ("analysis", "Analysis"), ("write", "Writing up"), ("done", "Done")] },
    Track { key: "chair", label: "Department chair", short: "Chair", stages: &[("plan", "Upcoming"), ("active", "In progress"), ("waiting", "Waiting on others"), ("done", "Done")] },
    Track { key: "service", label: "Service", short: "Service", stages: &[("plan", "Upcoming"), ("active", "Active"), ("wrap", "Wrapping up"), ("done", "Done")] },
    Track { key: "social", label: "Social media to-do", short: "Social", stages: &[("todo", "To do"), ("drafted", "Drafted"), ("scheduled", "Scheduled"), ("posted", "Posted")] },
];

pub const LOG_KEEP: usize = 12;

pub fn track(key: &str) -> Option<&'static Track> {
    TRACKS.iter().find(|t| t.key == key)
}

/// Resolve a track from a key or label, case-insensitively.
pub fn find_track(s: &str) -> Option<&'static Track> {
    let q = s.trim().to_lowercase();
    if q == "academic" {
        return track("teaching");
    }
    TRACKS.iter().find(|t| t.key == q || t.label.to_lowercase() == q || t.short.to_lowercase() == q)
}

impl Track {
    pub fn stage_index(&self, stage: &str) -> Option<usize> {
        self.stages.iter().position(|s| s.0 == stage)
    }
    /// Resolve a stage from a key or label, case-insensitively.
    pub fn find_stage(&self, s: &str) -> Option<&'static str> {
        let q = s.trim().to_lowercase();
        self.stages.iter().find(|st| st.0 == q || st.1.to_lowercase() == q).map(|st| st.0)
    }
    pub fn final_stage(&self) -> &'static str {
        self.stages[self.stages.len() - 1].0
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct LogEntry {
    pub d: String,
    pub t: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub track: String,
    pub stage: String,
    pub kind: String,
    pub byline: String,
    pub words: String,
    pub status: String,
    pub next: String,
    pub open: Vec<String>,
    pub decisions: Vec<String>,
    pub files: String,
    /// Pointers to related records elsewhere (notes, decisions, sources,
    /// repositories), one per line: a web address or "label | address".
    pub links: Vec<String>,
    pub due_label: String,
    pub due_date: String,
    pub touched: String,
    pub log: Vec<LogEntry>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

pub fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        Some(Value::String(t)) => t.lines().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        _ => vec![],
    }
}

pub fn is_iso_date(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() && s.len() == 10
}

impl Item {
    /// Build a clean record from loose JSON, the way the web prototype did:
    /// unknown tracks fall back to Fiction, "academic" becomes Teaching, and
    /// a stage that doesn't belong to the track becomes its first stage.
    pub fn from_value(id: &str, v: &Value) -> Item {
        let mut it = Item {
            id: id.to_string(),
            title: s(v, "title"),
            track: s(v, "track"),
            stage: s(v, "stage"),
            kind: s(v, "kind"),
            byline: s(v, "byline"),
            words: s(v, "words"),
            status: s(v, "status"),
            next: s(v, "next"),
            open: str_list(v.get("open")),
            decisions: str_list(v.get("decisions")),
            files: s(v, "files"),
            links: str_list(v.get("links")),
            due_label: s(v, "dueLabel"),
            due_date: s(v, "dueDate"),
            touched: s(v, "touched"),
            log: match v.get("log") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|e| {
                        let t = e.get("t").and_then(Value::as_str)?;
                        Some(LogEntry { d: e.get("d").and_then(Value::as_str).unwrap_or("").to_string(), t: t.to_string() })
                    })
                    .collect(),
                _ => vec![],
            },
        };
        it.normalize();
        it
    }

    pub fn normalize(&mut self) {
        let t = find_track(&self.track).unwrap_or(&TRACKS[0]);
        self.track = t.key.to_string();
        self.stage = t.find_stage(&self.stage).unwrap_or(t.stages[0].0).to_string();
        self.title = self.title.trim().to_string();
        if self.title.is_empty() {
            self.title = "Untitled".into();
        }
        if !self.due_date.is_empty() && !is_iso_date(&self.due_date) {
            self.due_date.clear();
        }
        if !self.touched.is_empty() && !is_iso_date(&self.touched) {
            self.touched.clear();
        }
        self.open.retain(|x| !x.trim().is_empty());
        self.decisions.retain(|x| !x.trim().is_empty());
        self.links.retain(|x| !x.trim().is_empty());
        self.links.dedup();
        if self.log.len() > LOG_KEEP {
            self.log.drain(0..self.log.len() - LOG_KEEP);
        }
    }

    pub fn track_def(&self) -> &'static Track {
        track(&self.track).unwrap_or(&TRACKS[0])
    }
    pub fn stage_index(&self) -> usize {
        self.track_def().stage_index(&self.stage).unwrap_or(0)
    }
    pub fn stage_label(&self) -> &'static str {
        self.track_def().stages[self.stage_index()].1
    }
    pub fn is_final(&self) -> bool {
        self.stage_index() == self.track_def().stages.len() - 1
    }
    pub fn push_log(&mut self, d: &str, t: &str) {
        self.log.push(LogEntry { d: d.to_string(), t: t.to_string() });
        if self.log.len() > LOG_KEEP {
            self.log.drain(0..self.log.len() - LOG_KEEP);
        }
    }
}

pub fn stage_label(track_key: &str, stage: &str) -> &'static str {
    let t = track(track_key).unwrap_or(&TRACKS[0]);
    t.stages[t.stage_index(stage).unwrap_or(0)].1
}

/// Tracks and stages for the web layer.
pub fn tracks_json() -> Value {
    Value::Array(
        TRACKS
            .iter()
            .map(|t| {
                serde_json::json!({
                    "key": t.key, "label": t.label, "short": t.short,
                    "stages": t.stages.iter().map(|s| serde_json::json!([s.0, s.1])).collect::<Vec<_>>()
                })
            })
            .collect(),
    )
}

pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let mut s: String = out.trim_matches('-').chars().take(48).collect();
    s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "project".into()
    } else {
        s
    }
}
