//! Background sync with the board server: shortly after every change to the
//! board file (from this window or an agent session), once a minute, and on
//! request. The window gets a "sync-status" event after each attempt.

use crate::settings;
use board_core::store::Store;
use board_core::sync::{self, ClientState, Report};
use serde_json::{json, Value};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub struct Syncer {
    tx: Mutex<Sender<()>>,
    lock: Arc<Mutex<()>>,
    store: Store,
    app: AppHandle,
}

impl Syncer {
    pub fn start(app: AppHandle, store: Store) -> Syncer {
        let (tx, rx) = channel::<()>();
        let lock = Arc::new(Mutex::new(()));
        let (lock2, store2, app2) = (lock.clone(), store.clone(), app.clone());
        std::thread::spawn(move || loop {
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(()) => {
                    // Let a burst of edits settle into one round trip.
                    std::thread::sleep(Duration::from_millis(1200));
                    while rx.try_recv().is_ok() {}
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if let Some((server, key)) = settings::sync_config() {
                let _g = lock2.lock().unwrap();
                let _ = sync::http::sync(&store2, &server, &key);
                drop(_g);
                let _ = app2.emit("sync-status", status(&store2));
            }
        });
        let _ = tx.send(());
        Syncer { tx: Mutex::new(tx), lock, store, app }
    }

    pub fn trigger(&self) {
        let _ = self.tx.lock().unwrap().send(());
    }

    pub fn run_now(&self) -> Result<Report, String> {
        let (server, key) = settings::sync_config().ok_or("Sync isn't set up on this computer.")?;
        let r = {
            let _g = self.lock.lock().unwrap();
            sync::http::sync(&self.store, &server, &key)
        };
        let _ = self.app.emit("sync-status", status(&self.store));
        r
    }

    pub fn resolve(&self, id: &str, keep_mine: bool) -> Result<(), String> {
        let server = settings::load().sync_server;
        {
            let _g = self.lock.lock().unwrap();
            sync::resolve_conflict(&self.store, &server, id, keep_mine)?;
        }
        self.trigger();
        let _ = self.app.emit("sync-status", status(&self.store));
        Ok(())
    }
}

pub fn status(store: &Store) -> Value {
    let server = settings::load().sync_server;
    if server.trim().is_empty() {
        return json!({"configured": false});
    }
    let st = ClientState::load(store, &server);
    let board = store.load().unwrap_or_default();
    let conflicts: Vec<Value> = st.conflicts.iter().map(|(id, c)| json!({"id": id, "local": board.get(id), "server": c.server})).collect();
    json!({
        "configured": true,
        "hasKey": settings::get_sync_key().is_some(),
        "server": server,
        "lastSync": st.last_sync,
        "lastError": st.last_error,
        "conflicts": conflicts,
    })
}
