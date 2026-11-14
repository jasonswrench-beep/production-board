import { call, $, el, errText, onEvent, type Info, type Item } from "./shared";

export interface SyncStatus {
  configured: boolean; hasKey?: boolean; server?: string; lastSync?: string; lastError?: string;
  conflicts?: { id: string; local: Item | null; server: Item | null }[];
}

const FIELDS: [keyof Item, string][] = [
  ["title", "Title"], ["track", "Track"], ["stage", "Stage"], ["kind", "What it is"], ["byline", "Byline"], ["words", "Length"],
  ["status", "Where it stands"], ["next", "Next step"], ["open", "Waiting on you"], ["decisions", "Standing decisions"],
  ["files", "Files"], ["links", "Links"], ["dueDate", "Due date"], ["dueLabel", "What the date is for"], ["touched", "Last worked on"],
];

let info: Info;
let refreshBoard: () => Promise<void> = async () => {};
let last: SyncStatus = { configured: false };
let running = false;
const listeners: ((s: SyncStatus) => void)[] = [];

export function onSyncStatus(cb: (s: SyncStatus) => void) { listeners.push(cb); cb(last); }

function when(iso?: string) {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "";
  const today = new Date().toDateString() === d.toDateString();
  return today ? d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : d.toLocaleDateString([], { month: "short", day: "numeric" });
}

export function describe(s: SyncStatus): { text: string; tone: "" | "warn" | "crit" | "good"; detail: string } {
  if (!s.configured) return { text: "", tone: "", detail: "" };
  const n = s.conflicts?.length ?? 0;
  if (running) return { text: "Syncing…", tone: "", detail: "" };
  if (n) return { text: n === 1 ? "1 card to settle" : n + " cards to settle", tone: "warn", detail: "Changed on this computer and somewhere else. Choose which version to keep." };
  if (!s.hasKey) return { text: "Sync needs its key", tone: "crit", detail: "The access key is missing from this computer's credential store. Paste it again in Settings." };
  if (s.lastError) return { text: /reach/.test(s.lastError) ? "Offline" : "Sync problem", tone: "warn", detail: s.lastError };
  return { text: s.lastSync ? "Synced " + when(s.lastSync) : "Not synced yet", tone: "good", detail: s.server ? "Synced with " + s.server : "" };
}

function render() {
  const pill = $("sync-pill");
  const d = describe(last);
  pill.hidden = !last.configured;
  $("sync-text").textContent = d.text;
  pill.dataset.tone = d.tone;
  pill.title = d.detail;
  listeners.forEach((cb) => cb(last));
}

export function setStatus(s: SyncStatus) {
  last = s;
  render();
  if ($<HTMLDialogElement>("cdlg").open) renderConflicts();
}

export async function syncNow(): Promise<string> {
  running = true; render();
  try {
    const r = await call<{ sent: number; received: number; merged: number; conflicts: number }>("sync_now");
    const parts = [];
    if (r.sent) parts.push(r.sent + " sent");
    if (r.received) parts.push(r.received + " received");
    if (r.merged) parts.push(r.merged + " combined");
    if (r.conflicts) parts.push(r.conflicts + " to settle");
    return parts.length ? "Synced: " + parts.join(", ") + "." : "Synced. Everything was already up to date.";
  } finally {
    running = false;
    setStatus(await call<SyncStatus>("sync_status").catch(() => last));
    await refreshBoard();
  }
}

function show(v: unknown): string {
  if (Array.isArray(v)) return v.length ? v.join("\n") : "(none)";
  return v === undefined || v === null || v === "" ? "(empty)" : String(v);
}

function stageLabel(it: Item) {
  const t = info.tracks.find((x) => x.key === it.track);
  return t?.stages.find((s) => s[0] === it.stage)?.[1] ?? it.stage;
}

function value(it: Item, k: keyof Item) {
  if (k === "stage") return stageLabel(it);
  if (k === "track") return info.tracks.find((x) => x.key === it.track)?.label ?? it.track;
  return it[k];
}

function renderConflicts() {
  const body = $("c-body");
  body.textContent = "";
  const list = last.conflicts ?? [];
  if (!list.length) { body.appendChild(el("p", "hint", "Nothing left to settle.")); return; }
  for (const c of list) {
    const card = el("section", "conflict");
    const title = c.local?.title || c.server?.title || c.id;
    card.appendChild(el("h3", null, title));
    const keepMine = el("button", "btn small primary", c.local ? "Keep this computer's version" : "Keep it deleted");
    const keepOther = el("button", "btn small", c.server ? "Keep the other version" : "Delete it here too");
    keepMine.type = keepOther.type = "button";
    if (!c.local || !c.server) {
      card.appendChild(el("p", "hint", !c.server ? "It was deleted somewhere else, but edited on this computer." : "It was deleted on this computer, but edited somewhere else."));
    } else {
      const tbl = el("table", "cmp");
      const head = el("tr");
      ["", "This computer", "Elsewhere"].forEach((h) => head.appendChild(el("th", null, h)));
      tbl.appendChild(head);
      for (const [k, label] of FIELDS) {
        const a = show(value(c.local, k)), b = show(value(c.server, k));
        if (a === b) continue;
        const tr = el("tr");
        tr.appendChild(el("th", null, label));
        tr.appendChild(el("td", null, a));
        tr.appendChild(el("td", null, b));
        tbl.appendChild(tr);
      }
      card.appendChild(tbl);
    }
    const msg = el("p", "msg"); msg.hidden = true;
    const act = el("div", "actions");
    const choose = (mine: boolean) => async () => {
      keepMine.disabled = keepOther.disabled = true;
      try { await call("sync_resolve", { id: c.id, keepMine: mine }); setStatus(await call<SyncStatus>("sync_status")); await refreshBoard(); }
      catch (e) { msg.hidden = false; msg.textContent = errText(e); keepMine.disabled = keepOther.disabled = false; }
    };
    keepMine.addEventListener("click", choose(true));
    keepOther.addEventListener("click", choose(false));
    act.appendChild(keepMine); act.appendChild(keepOther);
    card.appendChild(act); card.appendChild(msg);
    body.appendChild(card);
  }
}

export function setupSync(i: Info, refresh: () => Promise<void>) {
  info = i;
  refreshBoard = refresh;
  void onEvent<SyncStatus>("sync-status", (s) => setStatus(s));
  void call<SyncStatus>("sync_status").then(setStatus).catch(() => {});
  $("sync-pill").addEventListener("click", async () => {
    if (last.conflicts?.length) { renderConflicts(); $<HTMLDialogElement>("cdlg").showModal(); return; }
    if (!last.hasKey) { $("open-settings").click(); return; }
    try { await syncNow(); } catch { /* shown on the pill */ }
  });
}
