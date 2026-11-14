// Release builds use the GUI subsystem so no console window appears. The same
// executable doubles as the board's MCP server when started with --mcp; stdio
// still works because the calling process hands over its pipes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--mcp") {
        std::process::exit(production_board_lib::mcp::run_from_args(&args));
    }
    if args.iter().any(|a| a == "--configure-sync") {
        std::process::exit(production_board_lib::configure_sync_from_args(&args));
    }
    production_board_lib::run();
}
