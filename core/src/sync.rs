//! Two-way sync between a desktop board and the board server.
//!
//! The server is the hub. It numbers every change with a rising sequence
//! number and remembers, per item, the number of its latest change (deleted
//! items leave a numbered tombstone). A client remembers the last number it
//! has seen and, per item, the version it last agreed on with the server
//! (its "base").
//!
//! One round trip: the client sends every item that differs from its base,
//! each tagged with the base's number. The server accepts a change only if
//! the item hasn't changed on the server since that number; otherwise it
//! rejects it and the client merges. The reply also carries everything that
//! changed on the server since the client's last number.
//!
//! Merging is per field against the base: a field changed on one side only
//! takes that side's value; activity logs are combined; the later "touched"
//! date wins; decision lists combine additions and removals from both sides.
//! Only the same field changed differently on both sides, or a card edited
//! on one side and deleted on the other, becomes a conflict that waits for
//! the owner to choose. Nothing is ever overwritten silently.

use crate::model::{Item, LogEntry, LOG_KEEP};
use crate::store::{Board, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

// ---------- wire format ----------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PushChange {
    pub id: String,
    /// The server sequence number of the version this change was made from; 0 for a new item.
    pub base: u64,
    /// The new version, or null for a deletion.
    pub item: Option<Item>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SyncRequest {
    pub since: u64,
    pub push: Vec<PushChange>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Change {
    pub id: String,
    pub seq: u64,
    pub item: Option<Item>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Accepted {
    pub id: String,
    pub seq: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SyncResponse {
    pub seq: u64,
    pub changes: Vec<Change>,
    pub accepted: Vec<Accepted>,
    pub rejected: Vec<String>,
}

// ---------- server side ----------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Index {
    pub seq: u64,
    pub items: BTreeMap<String, u64>,
    pub deleted: BTreeMap<String, u64>,
    /// The track each deleted item was on, so a change feed limited to some
    /// tracks can report its deletion. Older tombstones may lack one.
    #[serde(default)]
    pub deleted_tracks: BTreeMap<String, String>,
}

impl Index {
    pub fn seq_of(&self, id: &str) -> u64 {
        self.items.get(id).or_else(|| self.deleted.get(id)).copied().unwrap_or(0)
    }
}

/// The server's copy of the board plus its change numbering. Every change to
/// the server's board, whatever made it (sync, the web page, an agent
/// session), is numbered by `reconcile`, which compares the board with the
/// last version it numbered.
pub struct Hub {
    pub store: Store,
    pub index: Index,
    known: HashMap<String, Item>,
}

impl Hub {
    fn index_path(store: &Store) -> PathBuf {
        store.dir.join("sync-index.json")
    }

    pub fn open(store: Store) -> Result<Hub, String> {
        let index: Index = fs::read_to_string(Self::index_path(&store)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let board = store.load()?;
        // Items the index already numbers are taken as known, so a restart
        // doesn't renumber the whole board.
        let known = board.projects.into_iter().filter(|it| index.items.contains_key(&it.id)).map(|it| (it.id.clone(), it)).collect();
        let mut hub = Hub { store, index, known };
        hub.reconcile()?;
        Ok(hub)
    }

    fn save_index(&self) -> Result<(), String> {
        let p = Self::index_path(&self.store);
        let tmp = p.with_extension("json.tmp");
        fs::create_dir_all(&self.store.dir).map_err(|e| e.to_string())?;
        fs::write(&tmp, serde_json::to_string(&self.index).unwrap()).map_err(|e| format!("Couldn't save the sync index: {e}"))?;
        fs::rename(&tmp, &p).map_err(|e| format!("Couldn't save the sync index: {e}"))
    }

    fn reconcile_board(&mut self, b: &Board) -> bool {
        let mut changed = false;
        for it in &b.projects {
            if self.known.get(&it.id) != Some(it) || !self.index.items.contains_key(&it.id) {
                self.index.seq += 1;
                self.index.items.insert(it.id.clone(), self.index.seq);
                self.index.deleted.remove(&it.id);
                self.index.deleted_tracks.remove(&it.id);
                self.known.insert(it.id.clone(), it.clone());
                changed = true;
            }
        }
        let present: HashSet<&str> = b.projects.iter().map(|p| p.id.as_str()).collect();
        let mut gone: Vec<String> = self.known.keys().filter(|k| !present.contains(k.as_str())).cloned().collect();
        gone.extend(self.index.items.keys().filter(|k| !present.contains(k.as_str()) && !self.known.contains_key(*k)).cloned());
        gone.sort();
        gone.dedup();
        for id in gone {
            self.index.seq += 1;
            self.index.items.remove(&id);
            self.index.deleted.insert(id.clone(), self.index.seq);
            match self.known.remove(&id) {
                Some(old) => {
                    self.index.deleted_tracks.insert(id.clone(), old.track);
                }
                None => {
                    self.index.deleted_tracks.remove(&id);
                }
            }
            changed = true;
        }
        changed
    }

    /// Number any changes made to the board since the last call.
    pub fn reconcile(&mut self) -> Result<(), String> {
        let b = self.store.load()?;
        if self.reconcile_board(&b) {
            self.save_index()?;
        }
        Ok(())
    }

    pub fn handle(&mut self, req: &SyncRequest) -> Result<SyncResponse, String> {
        self.reconcile()?;
        let seqs: HashMap<String, u64> = req.push.iter().map(|c| (c.id.clone(), self.index.seq_of(&c.id))).collect();
        let mut ok_ids = Vec::new();
        let mut rejected = Vec::new();
        self.store.modify_if_changed(|b| {
            for ch in &req.push {
                let incoming = ch.item.as_ref().map(|it| Item::from_value(&ch.id, &serde_json::to_value(it).unwrap()));
                if b.get(&ch.id) == incoming.as_ref() {
                    ok_ids.push(ch.id.clone());
                } else if seqs[&ch.id] == ch.base {
                    match incoming {
                        Some(it) => b.put(it),
                        None => {
                            b.remove(&ch.id);
                        }
                    }
                    ok_ids.push(ch.id.clone());
                } else {
                    rejected.push(ch.id.clone());
                }
            }
            Ok(())
        })?;
        self.reconcile()?;
        let board = self.store.load()?;
        let accepted = ok_ids.iter().map(|id| Accepted { id: id.clone(), seq: self.index.seq_of(id) }).collect();
        let mut ids: Vec<(&String, u64)> = self.index.items.iter().chain(self.index.deleted.iter()).filter(|(id, s)| **s > req.since || rejected.contains(id)).map(|(id, s)| (id, *s)).collect();
        ids.sort_by_key(|x| x.1);
        let changes = ids.into_iter().map(|(id, seq)| Change { id: id.clone(), seq, item: board.get(id).cloned() }).collect();
        Ok(SyncResponse { seq: self.index.seq, changes, accepted, rejected })
    }

    /// Everything that changed after `since`, limited to what `scope` may
    /// see: changed items in full, deleted ones as null. A deletion whose
    /// track isn't known is left out unless the scope covers every track.
    pub fn changes_since(&mut self, since: u64, scope: &crate::tools::Scope) -> Result<(u64, Vec<Change>), String> {
        self.reconcile()?;
        let board = self.store.load()?;
        let mut out: Vec<Change> = Vec::new();
        for (id, seq) in self.index.items.iter().filter(|(_, s)| **s > since) {
            if let Some(it) = board.get(id) {
                if scope.allows(&it.track) {
                    out.push(Change { id: id.clone(), seq: *seq, item: Some(it.clone()) });
                }
            }
        }
        for (id, seq) in self.index.deleted.iter().filter(|(_, s)| **s > since) {
            let visible = match self.index.deleted_tracks.get(id) {
                Some(t) => scope.allows(t),
                None => scope.tracks.is_none(),
            };
            if visible {
                out.push(Change { id: id.clone(), seq: *seq, item: None });
            }
        }
        out.sort_by_key(|c| c.seq);
        Ok((self.index.seq, out))
    }
}

// ---------- merging ----------

fn to_map(it: &Item) -> Map<String, Value> {
    match serde_json::to_value(it) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

fn merge_log(l: &Value, s: &Value) -> Value {
    let parse = |v: &Value| serde_json::from_value::<Vec<LogEntry>>(v.clone()).unwrap_or_default();
    let mut out = parse(l);
    for e in parse(s) {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out.sort_by(|a, b| a.d.cmp(&b.d));
    if out.len() > LOG_KEEP {
        out.drain(0..out.len() - LOG_KEEP);
    }
    serde_json::to_value(out).unwrap()
}

fn merge_list(b: Option<&Value>, l: &Value, s: &Value) -> Value {
    let list = |v: Option<&Value>| -> Vec<String> { v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default() };
    let (bl, ll, sl) = (list(b), list(Some(l)), list(Some(s)));
    let mut out: Vec<String> = ll.iter().filter(|x| !(bl.contains(x) && !sl.contains(x))).cloned().collect();
    for x in sl {
        if !bl.contains(&x) && !out.contains(&x) {
            out.push(x);
        }
    }
    serde_json::to_value(out).unwrap()
}

/// Field-by-field merge of two edits of the same item. None means a real conflict.
pub fn merge_fields(base: &Item, local: &Item, server: &Item) -> Option<Item> {
    let (b, l, s) = (to_map(base), to_map(local), to_map(server));
    let mut keys: Vec<&String> = b.keys().chain(l.keys()).chain(s.keys()).collect();
    keys.sort();
    keys.dedup();
    let null = Value::Null;
    let mut out = Map::new();
    for k in keys {
        let (bv, lv, sv) = (b.get(k), l.get(k).unwrap_or(&null), s.get(k).unwrap_or(&null));
        let v = if lv == sv || Some(sv) == bv {
            lv.clone()
        } else if Some(lv) == bv {
            sv.clone()
        } else {
            match k.as_str() {
                "log" => merge_log(lv, sv),
                "touched" => std::cmp::max(lv.as_str().unwrap_or(""), sv.as_str().unwrap_or("")).into(),
                "open" | "decisions" | "links" => merge_list(bv, lv, sv),
                _ => return None,
            }
        };
        out.insert(k.clone(), v);
    }
    serde_json::from_value(Value::Object(out)).ok()
}

/// Three-way merge of an item that may have been created, edited or deleted
/// on either side. `Some(result)` is the merged outcome (None inside means
/// deleted); `None` is a conflict for the owner.
pub fn merge3(base: Option<&Item>, local: Option<&Item>, server: Option<&Item>) -> Option<Option<Item>> {
    if local == server || local == base {
        return Some(server.cloned());
    }
    if server == base {
        return Some(local.cloned());
    }
    match (base, local, server) {
        (Some(b), Some(l), Some(s)) => merge_fields(b, l, s).map(Some),
        _ => None,
    }
}

// ---------- client side ----------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Base {
    pub seq: u64,
    pub item: Option<Item>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Conflict {
    pub seq: u64,
    /// The server's version; null if it was deleted there.
    pub server: Option<Item>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ClientState {
    pub server: String,
    pub last_seq: u64,
    pub base: BTreeMap<String, Base>,
    pub conflicts: BTreeMap<String, Conflict>,
    pub last_sync: String,
    pub last_error: String,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub sent: usize,
    pub received: usize,
    pub merged: usize,
    pub conflicts: usize,
}

impl ClientState {
    fn path(store: &Store) -> PathBuf {
        store.dir.join(".sync-state.json")
    }

    /// The saved state for this server. Pointing at a different server starts over.
    pub fn load(store: &Store, server: &str) -> ClientState {
        let st: ClientState = fs::read_to_string(Self::path(store)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if st.server == server {
            st
        } else {
            ClientState { server: server.to_string(), ..Default::default() }
        }
    }

    pub fn save(&self, store: &Store) -> Result<(), String> {
        let p = Self::path(store);
        fs::create_dir_all(&store.dir).map_err(|e| e.to_string())?;
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string(self).unwrap()).map_err(|e| format!("Couldn't save the sync state: {e}"))?;
        fs::rename(&tmp, &p).map_err(|e| format!("Couldn't save the sync state: {e}"))
    }

    /// Everything that differs from what the server last agreed to, except
    /// items waiting for the owner to settle a conflict.
    pub fn pending_push(&self, board: &Board) -> Vec<PushChange> {
        let mut out = Vec::new();
        for it in &board.projects {
            if self.conflicts.contains_key(&it.id) {
                continue;
            }
            match self.base.get(&it.id) {
                Some(b) if b.item.as_ref() == Some(it) => {}
                Some(b) => out.push(PushChange { id: it.id.clone(), base: b.seq, item: Some(it.clone()) }),
                None => out.push(PushChange { id: it.id.clone(), base: 0, item: Some(it.clone()) }),
            }
        }
        for (id, b) in &self.base {
            if b.item.is_some() && board.get(id).is_none() && !self.conflicts.contains_key(id) {
                out.push(PushChange { id: id.clone(), base: b.seq, item: None });
            }
        }
        out
    }

    fn set_base(&mut self, id: &str, seq: u64, item: Option<Item>) {
        self.base.insert(id.to_string(), Base { seq, item });
    }

    /// Take in the server's reply. Returns true if merging produced changes
    /// that still need to go to the server.
    pub fn absorb(&mut self, b: &mut Board, pushed: &[PushChange], resp: &SyncResponse, report: &mut Report) -> bool {
        let sent: HashMap<&str, &PushChange> = pushed.iter().map(|p| (p.id.as_str(), p)).collect();
        let mut done: HashMap<&str, u64> = HashMap::new();
        for a in &resp.accepted {
            if let Some(p) = sent.get(a.id.as_str()) {
                self.set_base(&a.id, a.seq, p.item.clone());
                self.conflicts.remove(&a.id);
                done.insert(a.id.as_str(), a.seq);
                report.sent += 1;
            }
        }
        let mut more = false;
        for ch in &resp.changes {
            if done.get(ch.id.as_str()) == Some(&ch.seq) {
                continue;
            }
            let local = b.get(&ch.id).cloned();
            if local == ch.item {
                self.set_base(&ch.id, ch.seq, ch.item.clone());
                self.conflicts.remove(&ch.id);
                continue;
            }
            if let Some(c) = self.conflicts.get_mut(&ch.id) {
                // Still waiting on the owner; keep their choice current.
                c.seq = ch.seq;
                c.server = ch.item.clone();
                self.set_base(&ch.id, ch.seq, ch.item.clone());
                continue;
            }
            let base = self.base.get(&ch.id).and_then(|x| x.item.clone());
            match merge3(base.as_ref(), local.as_ref(), ch.item.as_ref()) {
                Some(result) => {
                    if result == ch.item {
                        report.received += 1;
                    } else {
                        report.merged += 1;
                        more = true;
                    }
                    put_or_remove(b, &ch.id, result);
                    self.set_base(&ch.id, ch.seq, ch.item.clone());
                }
                None => {
                    self.conflicts.insert(ch.id.clone(), Conflict { seq: ch.seq, server: ch.item.clone() });
                    self.set_base(&ch.id, ch.seq, ch.item.clone());
                    report.conflicts += 1;
                }
            }
        }
        // Tombstones the client has caught up with need no base.
        self.base.retain(|id, x| x.item.is_some() || b.get(id).is_some());
        self.last_seq = self.last_seq.max(resp.seq);
        more
    }

    /// Settle a conflict: keep this computer's version ("mine") or take the
    /// other one ("theirs"). Either way the result syncs on the next round.
    pub fn resolve(&mut self, b: &mut Board, id: &str, keep_mine: bool) -> Result<(), String> {
        let c = self.conflicts.remove(id).ok_or("That conflict has already been settled.")?;
        if !keep_mine {
            put_or_remove(b, id, c.server);
        }
        Ok(())
    }
}

fn put_or_remove(b: &mut Board, id: &str, item: Option<Item>) {
    match item {
        Some(it) => b.put(it),
        None => {
            b.remove(id);
        }
    }
}

/// One full sync against any transport. Repeats while merges leave changes
/// to send (at most three round trips).
pub fn sync_with(store: &Store, server: &str, send: &mut dyn FnMut(&SyncRequest) -> Result<SyncResponse, String>) -> Result<Report, String> {
    let mut report = Report::default();
    for _ in 0..3 {
        let (since, push) = {
            let st = ClientState::load(store, server);
            let board = store.load()?;
            (st.last_seq, st.pending_push(&board))
        };
        let resp = send(&SyncRequest { since, push: push.clone() })?;
        let more = store.modify_if_changed(|b| {
            let mut st = ClientState::load(store, server);
            let more = st.absorb(b, &push, &resp, &mut report);
            st.last_sync = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
            st.last_error.clear();
            st.save(store)?;
            Ok(more)
        })?;
        if !more {
            break;
        }
    }
    Ok(report)
}

pub fn record_error(store: &Store, server: &str, err: &str) {
    let _ = store.with_lock(|| {
        let mut st = ClientState::load(store, server);
        st.last_error = err.to_string();
        st.save(store)
    });
}

/// Settle a conflict under the board lock.
pub fn resolve_conflict(store: &Store, server: &str, id: &str, keep_mine: bool) -> Result<(), String> {
    store.modify_if_changed(|b| {
        let mut st = ClientState::load(store, server);
        st.resolve(b, id, keep_mine)?;
        st.save(store)
    })
}

#[cfg(feature = "client")]
pub mod http {
    //! The sync client over HTTPS.

    use super::*;
    use std::time::Duration;

    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(40)).build()
    }

    pub fn base_url(server: &str) -> String {
        let s = server.trim().trim_end_matches('/');
        if s.starts_with("http://") || s.starts_with("https://") {
            s.to_string()
        } else {
            format!("https://{s}")
        }
    }

    fn explain(e: ureq::Error) -> String {
        match e {
            ureq::Error::Status(401, _) | ureq::Error::Status(403, _) => "The server didn't accept the access key.".into(),
            ureq::Error::Status(code, r) => {
                let body = r.into_string().unwrap_or_default();
                let msg = serde_json::from_str::<Value>(&body).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(String::from));
                msg.unwrap_or_else(|| format!("The server answered with an error ({code})."))
            }
            ureq::Error::Transport(_) => "Couldn't reach the server. This computer may be offline; changes will sync when it's back.".into(),
        }
    }

    pub fn post(server: &str, key: &str, path: &str, body: &Value) -> Result<Value, String> {
        let url = format!("{}{path}", base_url(server));
        let r = agent().post(&url).set("Authorization", &format!("Bearer {}", key.trim())).send_json(body).map_err(explain)?;
        r.into_json::<Value>().map_err(|e| format!("The server's reply couldn't be read: {e}"))
    }

    pub fn sync(store: &Store, server: &str, key: &str) -> Result<Report, String> {
        let url = format!("{}/api/sync", base_url(server));
        let a = agent();
        let auth = format!("Bearer {}", key.trim());
        let res = sync_with(store, server, &mut |req| {
            let r = a.post(&url).set("Authorization", &auth).send_json(req).map_err(explain)?;
            r.into_json::<SyncResponse>().map_err(|e| format!("The server's reply couldn't be read: {e}"))
        });
        if let Err(e) = &res {
            record_error(store, server, e);
        }
        res
    }
}
