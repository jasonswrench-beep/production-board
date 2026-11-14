//! Everything about the board that doesn't depend on a window: the record
//! format, the data file, the watch lists, the editing operations, the tools
//! offered to agent sessions, the stdio MCP server and two-way sync.

pub mod commands;
pub mod lists;
pub mod mcp;
pub mod model;
pub mod ops;
pub mod store;
pub mod sync;
pub mod tools;

pub fn today_date() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

pub fn today() -> String {
    today_date().format("%Y-%m-%d").to_string()
}
