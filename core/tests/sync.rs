//! Two desktop boards syncing through one server, all in one process.

use board_core::model::Item;
use board_core::ops::Op;
use board_core::store::{read_board_file, Store};
use board_core::sync::{resolve_conflict, sync_with, ClientState, Hub, Report};
use std::path::PathBuf;
use std::sync::Mutex;

const SERVER: &str = "https://board.test";

fn temp(name: &str) -> Store {
    let d = std::env::temp_dir().join(format!("pb-sync-{}-{}-{name}", std::process::id(), rand_suffix()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Store { dir: d }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

fn seed() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src-tauri/tests/fixtures/seed.json")
}

struct World {
    hub: Mutex<Hub>,
}

impl World {
    fn new() -> World {
        World { hub: Mutex::new(Hub::open(temp("server")).unwrap()) }
    }
    fn sync(&self, client: &Store) -> Report {
        sync_with(client, SERVER, &mut |req| self.hub.lock().unwrap().handle(req)).unwrap()
    }
    fn server(&self) -> Store {
        self.hub.lock().unwrap().store.clone()
    }
}

fn item(s: &Store, id: &str) -> Option<Item> {
    s.load().unwrap().get(id).cloned()
}

fn edit(s: &Store, id: &str, f: impl FnOnce(&mut Item)) {
    s.modify(|b| {
        f(b.get_mut(id).unwrap());
        Ok(())
    })
    .unwrap();
}

/// Pc A imports the seed and syncs; pc B starts empty and pulls it all.
fn setup() -> (World, Store, Store) {
    let w = World::new();
    let a = temp("a");
    let b = temp("b");
    let incoming = read_board_file(&seed()).unwrap();
    a.modify(|bd| {
        for it in incoming.projects {
            bd.put(it);
        }
        Ok(())
    })
    .unwrap();
    let r = w.sync(&a);
    assert_eq!(r.sent, 67);
    let r = w.sync(&b);
    assert_eq!(r.received, 67);
    assert_eq!(b.load().unwrap().projects.len(), 67);
    (w, a, b)
}

#[test]
fn first_sync_copies_the_whole_board_both_ways() {
    let (w, a, b) = setup();
    assert_eq!(w.server().load().unwrap().projects.len(), 67);
    assert_eq!(a.load().unwrap().projects, {
        let mut p = b.load().unwrap().projects;
        let order: Vec<String> = a.load().unwrap().projects.iter().map(|x| x.id.clone()).collect();
        p.sort_by_key(|x| order.iter().position(|o| *o == x.id));
        p
    });
    // Nothing left to do on a second round.
    assert_eq!(w.sync(&a), Report::default());
    assert_eq!(w.sync(&b), Report::default());
}

#[test]
fn a_move_on_one_computer_reaches_the_other() {
    let (w, a, b) = setup();
    a.modify(|bd| Op::Move { id: "ichabod".into(), track: None, stage: "edit".into() }.apply(bd, "2026-11-10").map(|_| ())).unwrap();
    assert_eq!(w.sync(&a).sent, 1);
    assert_eq!(w.sync(&b).received, 1);
    let it = item(&b, "ichabod").unwrap();
    assert_eq!(it.stage, "edit");
    assert_eq!(it.touched, "2026-11-10");
    assert_eq!(it.log.last().unwrap().t, "Stage: Revising to Editing");
}

#[test]
fn different_fields_edited_on_each_side_merge_without_asking() {
    let (w, a, b) = setup();
    edit(&a, "ichabod", |it| {
        it.status = "Edited on A".into();
        it.push_log("2026-11-10", "Worked on A");
    });
    edit(&b, "ichabod", |it| {
        it.next = "Next step from B".into();
        it.push_log("2026-11-11", "Worked on B");
        it.open.push("New question from B".into());
    });
    w.sync(&a);
    let r = w.sync(&b);
    assert_eq!(r.merged, 1);
    assert_eq!(r.conflicts, 0);
    w.sync(&a);
    for s in [&a, &b, &w.server()] {
        let it = item(s, "ichabod").unwrap();
        assert_eq!(it.status, "Edited on A");
        assert_eq!(it.next, "Next step from B");
        assert!(it.open.contains(&"New question from B".to_string()));
        let texts: Vec<&str> = it.log.iter().map(|l| l.t.as_str()).collect();
        assert!(texts.contains(&"Worked on A") && texts.contains(&"Worked on B"), "{texts:?}");
    }
}

#[test]
fn the_same_field_edited_both_ways_waits_for_the_owner() {
    let (w, a, b) = setup();
    edit(&a, "ichabod", |it| it.status = "Status from A".into());
    edit(&b, "ichabod", |it| it.status = "Status from B".into());
    w.sync(&a);
    let r = w.sync(&b);
    assert_eq!(r.conflicts, 1);
    // B keeps its own version until the owner decides, and doesn't push it.
    assert_eq!(item(&b, "ichabod").unwrap().status, "Status from B");
    assert_eq!(item(&w.server(), "ichabod").unwrap().status, "Status from A");
    assert_eq!(w.sync(&b), Report::default());
    let st = ClientState::load(&b, SERVER);
    assert_eq!(st.conflicts["ichabod"].server.as_ref().unwrap().status, "Status from A");

    resolve_conflict(&b, SERVER, "ichabod", true).unwrap();
    assert_eq!(w.sync(&b).sent, 1);
    w.sync(&a);
    assert_eq!(item(&a, "ichabod").unwrap().status, "Status from B");
    assert_eq!(item(&w.server(), "ichabod").unwrap().status, "Status from B");
}

#[test]
fn taking_the_other_version_settles_a_conflict() {
    let (w, a, b) = setup();
    edit(&a, "ichabod", |it| it.status = "Status from A".into());
    edit(&b, "ichabod", |it| it.status = "Status from B".into());
    w.sync(&a);
    w.sync(&b);
    resolve_conflict(&b, SERVER, "ichabod", false).unwrap();
    assert_eq!(item(&b, "ichabod").unwrap().status, "Status from A");
    assert_eq!(w.sync(&b), Report::default());
    assert!(ClientState::load(&b, SERVER).conflicts.is_empty());
}

#[test]
fn deletes_sync_and_stay_deleted() {
    let (w, a, b) = setup();
    a.modify(|bd| bd.remove("ichabod").map(|_| ()).ok_or("missing".into())).unwrap();
    w.sync(&a);
    w.sync(&b);
    assert!(item(&b, "ichabod").is_none());
    assert!(item(&w.server(), "ichabod").is_none());
    // Later rounds don't bring it back.
    w.sync(&a);
    w.sync(&b);
    assert!(item(&a, "ichabod").is_none() && item(&b, "ichabod").is_none());
}

#[test]
fn edit_against_delete_is_a_conflict_not_a_loss() {
    let (w, a, b) = setup();
    a.modify(|bd| bd.remove("ichabod").map(|_| ()).ok_or("missing".into())).unwrap();
    edit(&b, "ichabod", |it| it.next = "Still working on it".into());
    w.sync(&a);
    assert_eq!(w.sync(&b).conflicts, 1);
    assert_eq!(item(&b, "ichabod").unwrap().next, "Still working on it");
    resolve_conflict(&b, SERVER, "ichabod", true).unwrap();
    w.sync(&b);
    w.sync(&a);
    assert_eq!(item(&a, "ichabod").unwrap().next, "Still working on it");
}

#[test]
fn new_items_from_both_sides_all_arrive() {
    let (w, a, b) = setup();
    let today = "2026-11-10";
    let add = |s: &Store, title: &str, track: &str| {
        s.modify(|bd| {
            let fields = serde_json::json!({"title": title, "track": track});
            let it = board_core::ops::new_item(bd, fields.as_object().unwrap(), today)?;
            Op::Add { item: it }.apply(bd, today).map(|_| ())
        })
        .unwrap()
    };
    add(&a, "Added on A", "fiction");
    add(&b, "Added on B", "social");
    w.sync(&a);
    w.sync(&b);
    w.sync(&a);
    for s in [&a, &b, &w.server()] {
        let bd = s.load().unwrap();
        assert_eq!(bd.projects.len(), 69);
        assert!(bd.get("added-on-a").is_some() && bd.get("added-on-b").is_some());
    }
}

#[test]
fn changes_made_on_the_server_reach_clients() {
    let (w, a, _b) = setup();
    // The web page and remote agent sessions edit the server's board directly.
    w.server().modify(|bd| Op::Move { id: "ichabod".into(), track: None, stage: "edit".into() }.apply(bd, "2026-11-10").map(|_| ())).unwrap();
    assert_eq!(w.sync(&a).received, 1);
    assert_eq!(item(&a, "ichabod").unwrap().stage, "edit");
}

#[test]
fn the_server_numbering_survives_a_restart() {
    let (w, a, _b) = setup();
    let seq = w.hub.lock().unwrap().index.seq;
    let store = w.server();
    drop(w);
    let w2 = World { hub: Mutex::new(Hub::open(store).unwrap()) };
    assert_eq!(w2.hub.lock().unwrap().index.seq, seq);
    assert_eq!(w2.sync(&a), Report::default());
}
