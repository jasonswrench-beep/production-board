//! The three "this week" lists, with the same rules as the web prototype.

use crate::model::Item;
use crate::store::Board;
use chrono::NaiveDate;
use serde::Serialize;
use std::cmp::Ordering;

pub const COLD_DAYS: i64 = 45;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    /// Days since the date (negative means in the future). None when there is no date.
    pub days: Option<i64>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub closest: Vec<Row>,
    pub due: Vec<Row>,
    pub cold: Vec<Row>,
    pub in_flight: usize,
    pub decisions_waiting: usize,
}

pub fn days_since(s: &str, today: NaiveDate) -> Option<i64> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(|d| (today - d).num_days())
}

fn is_book(p: &Item) -> bool {
    p.track == "fiction" || p.track == "nonfiction"
}

pub fn summary(board: &Board, today: NaiveDate) -> Summary {
    let ps = &board.projects;

    // Closest to finished: books one or two stages from Published, furthest
    // along first, then fewest open decisions, then most recently logged.
    let mut close: Vec<&Item> = ps
        .iter()
        .filter(|p| {
            let n = p.track_def().stages.len();
            is_book(p) && !p.is_final() && p.stage_index() + 3 >= n
        })
        .collect();
    close.sort_by(|a, b| {
        b.stage_index()
            .cmp(&a.stage_index())
            .then(a.open.len().cmp(&b.open.len()))
            .then(b.touched.cmp(&a.touched))
    });
    let closest = close.into_iter().take(7).map(|p| Row { id: p.id.clone(), days: days_since(&p.touched, today) }).collect();

    // Due dates: unfinished with a date or a date in words, soonest first.
    let mut due: Vec<&Item> = ps.iter().filter(|p| !p.is_final() && (!p.due_label.is_empty() || !p.due_date.is_empty())).collect();
    let key = |p: &Item| if p.due_date.is_empty() { "9999".to_string() } else { p.due_date.clone() };
    due.sort_by(|a, b| key(a).cmp(&key(b)));
    let due = due.into_iter().take(8).map(|p| Row { id: p.id.clone(), days: if p.due_date.is_empty() { None } else { days_since(&p.due_date, today) } }).collect();

    // Gone cold: unfinished and not logged in 45 days or more, oldest first.
    let mut cold: Vec<(&Item, i64)> = ps
        .iter()
        .filter(|p| !p.is_final())
        .filter_map(|p| days_since(&p.touched, today).filter(|d| *d >= COLD_DAYS).map(|d| (p, d)))
        .collect();
    cold.sort_by(|a, b| a.0.touched.partial_cmp(&b.0.touched).unwrap_or(Ordering::Equal));
    let cold = cold.into_iter().take(8).map(|(p, d)| Row { id: p.id.clone(), days: Some(d) }).collect();

    Summary {
        closest,
        due,
        cold,
        in_flight: ps.iter().filter(|p| !p.is_final()).count(),
        decisions_waiting: ps.iter().map(|p| p.open.len()).sum(),
    }
}
