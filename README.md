# Production Board

A desktop app for Windows and Mac that shows everything in flight on one board: fiction, craft books and textbooks, products, software, teaching, research, department chair work, service, and social media chores. One pipeline per track, one column per stage, one card per item. Above the board, three lists answer the weekly questions: what is closest to finished, what is coming due, and what has gone cold. A calendar view shows every card with a due date by month, with what is late, due this week and due next week.

An optional assistant panel answers questions about the board and makes changes described in plain language. A built-in MCP server lets any agent session on the PC update the board at the end of a piece of work, so the board is kept current by the work itself.

The app runs fully offline with the assistant switched off.

## Installing

Run `Production Board_1.0.0_x64-setup.exe`. It installs for the current user only (no administrator prompt) and adds a Start menu entry.

On first run the app offers to import `production-board-seed.json` from the Downloads folder, to import another board file, or to start with an empty board.

## Where the data lives

The whole board is one file:

```
Documents\Production Board\board.json
```

If Documents is synced (for example to a cloud drive), the board is synced with it. Settings → Board data shows the exact path and has a **Show in folder** button.

Next to it:

- `backups\board-YYYY-MM-DD.json`: the first change each day copies the previous file here first. The last 30 days are kept.
- `.board.lock`: an empty file used so the app and agent sessions never write at the same moment. Safe to ignore.

### Backing up

Any of these works:

- Copy the `Production Board` folder from Documents to a backup drive.
- Settings → Board data → **Export the board** writes the whole board to a JSON file wherever you choose.
- Rely on the dated copies in `backups`.

To restore, use **Import a board file** with an export or a backup copy. Importing replaces items that have the same id and adds the rest; nothing else on the board is removed. To roll the whole board back, close the app and copy a backup over `board.json`.

The file format is the same as the seed and export files: `{ "projects": [ ... ] }`, one record per item with the fields `id, title, track, stage, kind, byline, words, status, next, open[], decisions[], files, dueLabel, dueDate, touched, log[]`. Dates are `YYYY-MM-DD`.

App settings (not the board) are in `%APPDATA%\Production Board\settings.json`. An API key, if used, is stored in Windows Credential Manager under "Production Board", never in a file.

## Using the board

- Drag a card to another column to change its stage, or into another track to move it there. A move stamps the card as worked on today and adds a line to its activity log.
- Click a card to edit any field, advance it one stage, mark it worked on today, or remove it.
- A card's **Links** field holds pointers to related records kept elsewhere, such as notes, decisions, sources or repositories. Put one per line: a web address, or a label, a bar, then the address (`Spec | https://...`).
- On a touch screen, hold a card for a moment before dragging it; a quick swipe scrolls instead. Escape cancels a drag.
- Track chips filter the board; the search box matches title, kind, byline, status and next step.
- Light and dark themes follow the Windows setting.

### The three lists

| List | Rule |
| --- | --- |
| Closest to finished | Fiction and nonfiction in Editing or Production. Furthest along first, then fewest open decisions, then most recently worked on. Up to 7. |
| Due dates | Unfinished items with a date or a date in words. Soonest first; overdue in red, within three weeks in amber. Up to 8. |
| Gone cold | Unfinished and not worked on for 45 days or more. Oldest first. Up to 8. |

## The assistant panel

Open it with **Assistant** in the header. Settings → Assistant picks how it connects:

- **Off**: the panel explains how to switch it on. Nothing else changes.
- **Installed coding agent**: requests go to the command-line coding agent already installed and signed in on the PC, run in non-interactive mode with only the board's tools available. It uses the agent's own sign-in and plan; the app never asks for, stores or passes on account credentials. The app finds the agent automatically; if it can't, use **Browse** to point at its program.
- **Your own API key**: the app calls the model API directly with a key pasted into Settings. This is the mode to use if the app is ever given to other people, each with their own key.

Every change the assistant makes is staged first. Small changes apply as soon as it finishes and show an **Undo** button. Removals, and changes touching more than three items, are listed for confirmation with **Apply** and **Discard**.

## Letting agent sessions update the board

The app's own executable is an MCP server when started with `--mcp` (stdio transport). Settings → Assistant → **Connect the board to the agent** registers it with the installed coding agent for all of its sessions on this PC. Any other MCP-capable tool can use this entry (the exact path is shown in Settings → Other agents):

```json
{
  "production-board": {
    "command": "C:\\Users\\<you>\\AppData\\Local\\Production Board\\production-board.exe",
    "args": ["--mcp"]
  }
}
```

Tools:

| Tool | What it does |
| --- | --- |
| `board_summary` | Counts and the three watch lists |
| `list_items` | Compact list, filtered by track, stage or search text |
| `get_item` | One full record |
| `update_item` | Change fields; also `add_open`, `resolve_open`, `add_decisions` |
| `move_item` | Change stage, optionally track; stamps today and logs the move |
| `add_item` | New item |
| `log_activity` | Log work done, stamp the date, optionally set next step and status |
| `list_tracks` | Tracks and stages |

Removing items is only offered inside the app's assistant panel, where the user confirms it.

Optional flags: `--data-dir <folder>` uses another board folder; `--stage-to <file>` records changes to a file instead of writing them (the assistant panel uses this).

## Sync and the web board

The board can also run on a server, where it does three things:

- **Keeps a second copy and syncs it with this computer.** The desktop app syncs a moment after every change, once a minute, and whenever you choose **Sync now**. Without a connection the app works as usual and catches up when it's back online.
- **Shows the board in any browser**, on a phone too, behind a password: the same board, lists and edit dialog. The assistant and settings stay in the desktop app.
- **Offers the board's tools to agent sessions anywhere** at `/mcp` on the server, so a session on another computer or in the cloud can log work and move cards. Those sessions send the access key as a bearer token.

The server for this board is at **board.jasonswrench.com**.

### Turning it on

In the desktop app: **Settings → Sync**. Enter the server address and the access key, then choose **Turn on sync**. The key is kept in Windows Credential Manager. In the same section, set the password for the web page. Setting a new password signs out every browser. **Copy access key** puts the key on the clipboard for connecting an agent session elsewhere.

### Access keys for other tools

Another tool, such as a notes or knowledge system, should get its own key rather than the main one. In **Settings → Sync → Access keys for other tools**:

1. Give the key a name.
2. Choose whether it can change cards, or only read them.
3. Choose every track, or just some.
4. Choose **Make key**.

The key is shown once; copy it into the tool. A limited key reaches the board's tools at `/mcp` and the change feed, and only for its tracks. It can never sync, set the web password or make keys. The list shows when each key was last used, and **Revoke** shuts a key out at once. `docs/integration-contract.md` describes everything a connected tool can rely on.

### How changes are combined

Each card syncs separately, so changes to different cards never interfere. When the same card changed in two places, the changes are combined field by field:

- If a field changed in only one place, that change is kept.
- Activity logs from both places are merged.
- The later "last worked on" date wins.
- Additions to and removals from the decision lists and links are kept from both sides.

The board asks you to choose only when the same field was changed to different values in two places, or when a card was edited in one place and deleted in another. The sync indicator in the header then reads "1 card to settle". Click it to see both versions side by side and keep one. Until you decide, this computer keeps its own version and doesn't send it. Nothing is overwritten silently.

### The server's data

On the server, everything lives in `/opt/production-board/data`:

| File | What it is |
| --- | --- |
| `board.json` | The board, in the same format as the desktop file |
| `backups/` | A copy of the board from each of the last 30 days |
| `sync-index.json` | The change numbering sync uses |
| `web-password.txt` | The web password, stored only as a one-way hash |
| `web-sessions.json` | Signed-in browsers, also stored as hashes |
| `access-keys.json` | Limited access keys for other tools, stored only as hashes |

No access key is ever stored on the server, only its fingerprint.

The desktop app keeps its own sync record in `.sync-state.json`, beside `board.json` in Documents\Production Board. Each desktop is itself a full copy of the board, so a server loss costs nothing that a sync won't restore.

## Tracks and stages

Each computer chooses which sections show: on the first-run screen, or later under **Settings → Sections**. A section that has cards on it always shows. The choice is kept on that computer only.

| Track | Stages |
| --- | --- |
| Fiction | Plot and outline, Drafting, Revising, Editing, Production, Published |
| Craft books and textbooks | Planning, Drafting, Revising, Editing, Production, Published |
| Products | Idea, Making, Packaging, On sale |
| Software and plugins | Spec and design, Building, Working build, In use |
| Teaching | Planning, Teaching now, Wrapping up, Done |
| Research | Design and approval, Data collection, Analysis, Writing up, Done |
| Department chair | Upcoming, In progress, Waiting on others, Done |
| Service | Upcoming, Active, Wrapping up, Done |
| Social media to-do | To do, Drafted, Scheduled, Posted |

They are defined once in `src-tauri/src/model.rs`.

## Building from source

Requires Rust (stable, 1.89 or later), Node.js 20 or later, and the WebView2 runtime (part of Windows 11).

```
npm install
npm run app:build     # installer in src-tauri/target/release/bundle/nsis/
npm run app:dev       # run with live reload
npm test              # every test: sync, server, watch lists, MCP server
```

The shared board code (`core/`) and the server (`server/`) have their own tests:

```
cd core && cargo test      # sync and merge rules: two computers and a server in one process
cd server && cargo test    # the real server over HTTP: sync, agent tools, web sign-in
```

The server is a single static Linux program:

```
cd server
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld cargo build --release --target x86_64-unknown-linux-musl
```

`deploy/docker-compose.yml` runs it on the VPS behind the existing HTTPS proxy. Put the program and the built web page (`dist/`) in `/opt/production-board/app`, with `web` as the page's folder name. Put the board data in `/opt/production-board/data`, owned by user 10001. Put `PB_KEY_SHA256=<SHA-256 of the access key>` in a `.env` file beside the compose file.

Set `PB_DATA_DIR` to point the app or the tests at another board folder, and `PB_CONFIG_DIR` for another settings folder.

### Layout

```
index.html, src/          web layer: board (main.ts), assistant panel, settings, sync;
                          the same page runs in the desktop window and on the server
core/src/model.rs         records, tracks, stages
core/src/store.rs         the board file: locking, atomic saves, daily backups
core/src/lists.rs         the three watch lists
core/src/ops.rs           every change to the board, and undo
core/src/commands.rs      save, move and delete, shared by the window and the web page
core/src/tools.rs         board tools shared by the MCP server and the assistant
core/src/mcp.rs           MCP server over stdio
core/src/sync.rs          sync: change numbering, per-field merging, conflicts
core/tests/               sync between two computers and a server
src-tauri/src/            the desktop app: window commands, assistant modes, background sync
src-tauri/tests/          watch lists against the seed file; MCP server end to end
server/src/main.rs        the server: sync hub, web page with sign-in, agent tools over HTTP
server/tests/             the server end to end over HTTP
deploy/                   the container definition for the VPS
```
