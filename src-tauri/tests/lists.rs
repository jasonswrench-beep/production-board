//! The three watch lists against the seed file, as of the day it was exported.

use chrono::NaiveDate;
use production_board_lib::lists::summary;
use production_board_lib::store::read_board_file;
use std::path::Path;

fn seed() -> production_board_lib::store::Board {
    read_board_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/seed.json")).unwrap()
}

fn ids(rows: &[production_board_lib::lists::Row]) -> Vec<&str> {
    rows.iter().map(|r| r.id.as_str()).collect()
}

#[test]
fn seed_imports_every_record() {
    let b = seed();
    assert_eq!(b.projects.len(), 67);
    // Stage keys from the seed survive unchanged.
    let ich = b.get("ichabod").unwrap();
    assert_eq!((ich.track.as_str(), ich.stage.as_str()), ("fiction", "revise"));
    assert_eq!(b.get("six-packed").unwrap().open.len(), 4);
}

#[test]
fn watch_lists_match_the_rules() {
    let b = seed();
    let s = summary(&b, NaiveDate::from_ymd_opt(2026, 11, 9).unwrap());

    // Books in Editing or Production; furthest along, then fewest open decisions, then most recent.
    assert_eq!(
        ids(&s.closest),
        ["happily-ever-his", "case-handbook-audiobook", "christmas-plus-one-problem", "north-pole-ny-mysteries", "business-comm-oer", "gravy-on-the-seating-plan", "terms-and-conditions-apply"]
    );

    // Unfinished items with a due date, soonest first; overdue shows as positive days.
    assert_eq!(
        ids(&s.due),
        ["terms-and-conditions-apply", "gravy-on-the-seating-plan", "christmas-plus-one-problem", "krampus-needs-a-vacation", "cmm-360", "cmm-500-independent-study", "training-and-development-course"]
    );
    assert_eq!(s.due[0].days, Some(39));
    assert_eq!(s.due[1].days, Some(-17));
    assert_eq!(s.due[4].days, None);

    // Not logged in 45 days or more, oldest first, at most eight.
    assert_eq!(
        ids(&s.cold),
        ["ai-literacy-textbook", "business-comm-oer", "krampus-needs-a-vacation", "romance-reader-survey-2025", "rwa-contest-2026", "straw-crossed-lovers", "consulting-website", "cmm-500-independent-study"]
    );
    assert_eq!(s.cold[0].days, Some(122));
    // Finished items never appear, however old.
    assert!(!ids(&s.cold).contains(&"pucking-power-plays"));

    let finished = b.projects.iter().filter(|p| p.is_final()).count();
    assert_eq!(s.in_flight, 67 - finished);
    assert_eq!(s.decisions_waiting, b.projects.iter().map(|p| p.open.len()).sum::<usize>());
}

#[test]
fn cold_threshold_is_inclusive_at_45_days() {
    let mut b = seed();
    b.projects.retain(|p| p.id == "cross-ex-and-chill"); // logged 2026-09-19
    let at = |y, m, d| summary(&b, NaiveDate::from_ymd_opt(y, m, d).unwrap()).cold.len();
    assert_eq!(at(2026, 11, 2), 0); // 44 days
    assert_eq!(at(2026, 11, 3), 1); // 45 days
}
