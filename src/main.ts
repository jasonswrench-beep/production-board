import "@fontsource/bricolage-grotesque/600.css";
import "@fontsource/bricolage-grotesque/700.css";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles.css";
import { call, $, el, errText, lsGet, lsSet, isWeb, onBoardChanged, type Item, type Info, type Summary } from "./shared";
import { setupAssistant } from "./assistant";
import { setupSettings } from "./settings";
import { setupSync } from "./sync";

type Stage = [string, string];
interface TrackDef { label: string; short: string; stages: Stage[] }

const COLD_DAYS = 45;
const TRACKS: Record<string, TrackDef> = {};
let ORDER: string[] = [];
let info: Info;
const state = { projects: [] as Item[], summary: null as Summary | null, filter: "all", q: "", loaded: false, exists: true, view: "board" as "board" | "calendar" };
let current: Item | null = null;
let saving = false;

/* ---------- dates and stages ---------- */
function pad(n: number) { return n < 10 ? "0" + n : "" + n; }
function todayISO() { const d = new Date(); return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate()); }
function parseISO(s: string) { if (!s || !/^\d{4}-\d{2}-\d{2}$/.test(s)) return null; const p = s.split("-"); return new Date(+p[0], +p[1] - 1, +p[2]); }
function daysSince(s: string): number | null { const d = parseISO(s); if (!d) return null; const t = parseISO(todayISO())!; return Math.round((t.getTime() - d.getTime()) / 86400000); }
const MON = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
function fmt(s: string) { const d = parseISO(s); if (!d) return ""; const y = d.getFullYear() === new Date().getFullYear() ? "" : " " + d.getFullYear(); return MON[d.getMonth()] + " " + d.getDate() + y; }
const DAY = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTH = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
function iso(d: Date) { return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate()); }
function addDays(d: Date, n: number) { const x = new Date(d.getFullYear(), d.getMonth(), d.getDate()); x.setDate(x.getDate() + n); return x; }
function weekStart(d: Date) { return addDays(d, -d.getDay()); }

/* ---------- due dates ---------- */
// Days from today to the date: positive is still to come, negative is late.
function daysUntil(s: string): number | null { const d = daysSince(s); return d == null ? null : -d; }
function dueClass(n: number) { return n <= 0 ? "crit" : n <= 14 ? "warn" : ""; }
function dueShort(n: number) { return n === 0 ? "today" : n === 1 ? "tomorrow" : n > 0 ? "in " + n + "d" : (-n) + "d late"; }
function dueText(s: string) {
  const n = daysUntil(s); if (n == null) return "";
  if (n === 0) return "Due today";
  if (n === 1) return "Due tomorrow";
  return "Due " + fmt(s) + " · " + (n > 0 ? n + " days left" : (n === -1 ? "1 day" : (-n) + " days") + " late");
}
function dueHint(s: string) {
  const d = parseISO(s), n = daysUntil(s);
  if (!d || n == null) return "Optional. Cards with a due date show on the calendar.";
  const day = DAY[d.getDay()];
  if (n === 0) return day + ", today.";
  if (n === 1) return day + ", tomorrow.";
  if (n === -1) return day + ", yesterday: 1 day late.";
  return n > 0 ? day + ", " + n + " days from today." : day + ", " + (-n) + " days ago.";
}
/* ---------- which sections show ---------- */
// Each computer can hide the sections it doesn't use. A section with cards on it
// always shows, so nothing on the board can go missing.
let hiddenTracks = new Set<string>();
function loadHidden() {
  try { const v = JSON.parse(lsGet("pb-hidden") || "[]"); if (Array.isArray(v)) hiddenTracks = new Set(v.filter((k) => typeof k === "string" && TRACKS[k])); }
  catch { hiddenTracks = new Set(); }
}
function used(k: string) { return state.projects.filter((p) => p.track === k).length; }
function shownTracks() { return ORDER.filter((k) => !hiddenTracks.has(k) || used(k) > 0); }
function renderSectionChoosers() {
  ["w-sections", "s-sections"].forEach((id) => {
    const box = document.getElementById(id); if (!box) return;
    box.textContent = "";
    ORDER.forEach((k) => {
      const n = used(k);
      const l = el("label", "check"); const c = document.createElement("input"); c.type = "checkbox";
      c.checked = !hiddenTracks.has(k) || n > 0; c.disabled = n > 0;
      c.addEventListener("change", () => {
        if (c.checked) hiddenTracks.delete(k); else hiddenTracks.add(k);
        if (!shownTracks().length) { hiddenTracks.delete(k); c.checked = true; return; }
        lsSet("pb-hidden", JSON.stringify(Array.from(hiddenTracks)));
        if (state.filter !== "all" && !shownTracks().includes(state.filter)) { state.filter = "all"; lsSet("pb-filter", "all"); }
        renderAll();
      });
      l.appendChild(c); l.appendChild(document.createTextNode(" " + TRACKS[k].label + (n > 0 ? " (has " + n + (n === 1 ? " card)" : " cards)") : "")));
      box.appendChild(l);
    });
  });
}
function stagesOf(track: string) { return (TRACKS[track] || TRACKS[ORDER[0]]).stages; }
function stageIndex(p: { track: string; stage: string }) { const s = stagesOf(p.track); for (let i = 0; i < s.length; i++) if (s[i][0] === p.stage) return i; return 0; }
function stageLabel(p: { track: string; stage: string }) { return stagesOf(p.track)[stageIndex(p)][1]; }
function isFinal(p: Item) { return stageIndex(p) === stagesOf(p.track).length - 1; }
function byId(id: string) { return state.projects.find((p) => p.id === id) || null; }
function matches(p: Item) {
  if (state.filter !== "all" && p.track !== state.filter) return false;
  if (!state.q) return true;
  const hay = [p.title, p.kind, p.byline, p.next, p.status].join(" ").toLowerCase();
  return hay.indexOf(state.q) !== -1;
}
function setStatus(t: string, isErr = false) { const s = $("status"); s.textContent = t || ""; s.hidden = !t; s.classList.toggle("err", isErr); }

/* ---------- rendering ---------- */
function renderTally() {
  const t = $("tally"); t.textContent = "";
  if (!state.loaded || !state.summary || !state.exists) { t.appendChild(el("span", null, "Everything in flight, by track and stage.")); return; }
  const s = el("span"); s.appendChild(el("b", null, String(state.summary.inFlight))); s.appendChild(document.createTextNode(" in flight")); t.appendChild(s);
  const d = el("span"); d.appendChild(el("b", null, String(state.summary.decisionsWaiting))); d.appendChild(document.createTextNode(" decisions waiting on you")); t.appendChild(d);
}

function rowItem(p: Item, sub: string, val: string, valCls: string) {
  const li = el("li"); const b = el("button", "row"); b.type = "button";
  b.appendChild(el("span", "rt", p.title));
  const s = el("span", "rs"); s.appendChild(el("span", "tag t-" + p.track, TRACKS[p.track].short)); s.appendChild(document.createTextNode("  " + sub)); b.appendChild(s);
  b.appendChild(el("span", "rv" + (valCls ? " " + valCls : ""), val));
  b.addEventListener("click", () => openDialog(p));
  li.appendChild(b); return li;
}
function fillList(id: string, items: HTMLElement[], emptyText: string) {
  const ol = $(id); ol.textContent = "";
  if (!items.length) { const li = el("li"); li.appendChild(el("p", "none", emptyText)); ol.appendChild(li); return; }
  items.forEach((li) => ol.appendChild(li));
}
function renderWeek() {
  const sm = state.summary;
  const close = (sm ? sm.closest : []).map((r) => byId(r.id)).filter((p): p is Item => !!p)
    .map((p) => rowItem(p, p.open.length ? p.open.length + " to decide" : (p.next || p.status), stageLabel(p), stageIndex(p) === stagesOf(p.track).length - 2 ? "good" : ""));
  fillList("l-close", close, state.loaded ? "Nothing is in editing or production yet." : "Books in editing or production will list here.");

  const due = (sm ? sm.due : []).map((r) => {
    const p = byId(r.id); if (!p) return null;
    const d = r.days; let val = "", cls = "";
    if (d == null) val = "no date";
    else if (d > 0) { val = d + "d past"; cls = "crit"; }
    else if (d === 0) { val = "today"; cls = "crit"; }
    else { val = "in " + (-d) + "d"; cls = -d <= 21 ? "warn" : ""; }
    return rowItem(p, p.dueLabel || fmt(p.dueDate), val, cls);
  }).filter((x): x is HTMLLIElement => !!x);
  fillList("l-due", due, state.loaded ? "No project has an outside date." : "Projects with a release date or term will list here.");

  const cold = (sm ? sm.cold : []).map((r) => { const p = byId(r.id); return p ? rowItem(p, stageLabel(p) + ", last logged " + fmt(p.touched), r.days + "d", "warn") : null; })
    .filter((x): x is HTMLLIElement => !!x);
  fillList("l-cold", cold, state.loaded ? "Nothing has gone cold." : "Projects nobody has touched in a while will list here.");
}

function renderFilters() {
  const nav = $("filters"); nav.textContent = "";
  ([["all", "All tracks"]] as [string, string][]).concat(shownTracks().map((k) => [k, TRACKS[k].label] as [string, string])).forEach((f) => {
    const b = el("button", "chip", f[1]); b.type = "button"; b.setAttribute("aria-pressed", String(state.filter === f[0]));
    const n = f[0] === "all" ? state.projects.length : state.projects.filter((p) => p.track === f[0]).length;
    if (state.loaded) b.appendChild(el("span", "n", String(n)));
    b.addEventListener("click", () => { state.filter = f[0]; lsSet("pb-filter", f[0]); renderFilters(); renderView(); });
    nav.appendChild(b);
  });
}

function cardFor(p: Item, finalCol: boolean) {
  const b = el("div", "card movable"); b.setAttribute("role", "button"); b.tabIndex = 0;
  b.setAttribute("aria-label", p.title + ", " + stageLabel(p) + ". Open to edit or change stage.");
  b.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openDialog(p); } });
  b.addEventListener("pointerdown", (e) => pressCard(e, b, p));
  b.appendChild(el("span", "ct", p.title));
  const k = [p.kind, p.byline ? "as " + p.byline : ""].filter(Boolean).join(" · ");
  if (k) b.appendChild(el("span", "ck", k));
  if (!finalCol) {
    const due = p.dueDate ? daysUntil(p.dueDate) : null;
    if (due != null) { const dl = el("span", "cdue " + dueClass(due), dueText(p.dueDate)); if (p.dueLabel) dl.title = p.dueLabel; b.appendChild(dl); }
    if (p.next) { const n = el("span", "cn"); n.appendChild(el("i", null, "Next: ")); n.appendChild(document.createTextNode(p.next)); b.appendChild(n); }
    const f = el("span", "cf");
    if (p.open.length) f.appendChild(el("span", "dec", p.open.length + " to decide"));
    if (p.words) f.appendChild(el("span", null, p.words));
    const d = daysSince(p.touched);
    if (d != null) f.appendChild(el("span", d >= COLD_DAYS ? "warn" : "", (d >= COLD_DAYS ? "cold " : "") + fmt(p.touched)));
    if (f.childNodes.length) b.appendChild(f);
  }
  b.addEventListener("click", () => openDialog(p));
  return b;
}

function renderBoards() {
  const root = $("boards"); root.textContent = "";
  if (!state.exists) return;
  shownTracks().forEach((k) => {
    if (state.filter !== "all" && state.filter !== k) return;
    const stages = TRACKS[k].stages;
    const mine = state.projects.filter((p) => p.track === k && matches(p));
    const sec = el("section", "track t-" + k); sec.setAttribute("aria-label", TRACKS[k].label);
    const head = el("div", "thead"); head.appendChild(el("h2", null, TRACKS[k].label));
    const inFlight = mine.filter((p) => !isFinal(p)).length;
    head.appendChild(el("span", "count", state.loaded ? inFlight + " in flight, " + (mine.length - inFlight) + " finished" : ""));
    if (mine.length) {
      const dist = el("div", "dist"); dist.setAttribute("role", "img");
      const parts: string[] = [];
      stages.forEach((s, i) => {
        const n = mine.filter((p) => stageIndex(p) === i).length; parts.push(s[1] + " " + n);
        if (n) { const seg = el("span"); seg.style.width = (n / mine.length * 100) + "%"; seg.style.opacity = String(0.35 + 0.65 * (i / (stages.length - 1))); seg.title = s[1] + ": " + n; dist.appendChild(seg); }
      });
      dist.setAttribute("aria-label", "Share of projects by stage: " + parts.join(", "));
      head.appendChild(dist);
    }
    sec.appendChild(head);
    if (state.loaded && !mine.length && !state.q) sec.appendChild(el("p", "status", k === "social" ? "No social tasks yet. Use Add item for each post or chore you owe, or drag a card in from another section." : "Nothing here yet. Use Add item, or drag a card in from another section."));
    const sc = el("div", "scroller"); const cols = el("div", "cols"); cols.style.setProperty("--n", String(stages.length));
    stages.forEach((s, i) => {
      const items = mine.filter((p) => stageIndex(p) === i).sort((a, b) => ((b.touched || "") < (a.touched || "") ? -1 : 1));
      const last = i === stages.length - 1;
      const col = el("div", "col" + (last ? " final" : "") + (items.length ? "" : " empty"));
      wireDrop(col, k, s[0]);
      const ch = el("div", "chead"); ch.appendChild(el("span", "name", s[1])); ch.appendChild(el("span", "n", String(items.length))); col.appendChild(ch);
      if (!items.length) col.appendChild(el("div", "slot"));
      items.forEach((p) => col.appendChild(cardFor(p, last)));
      cols.appendChild(col);
    });
    sc.appendChild(cols); sec.appendChild(sc); root.appendChild(sec);
  });
}

/* ---------- calendar ---------- */
let calMonth = (() => { const d = new Date(); return new Date(d.getFullYear(), d.getMonth(), 1); })();

function dated() { return state.projects.filter((p) => !!parseISO(p.dueDate) && matches(p)).sort((a, b) => (a.dueDate < b.dueDate ? -1 : a.dueDate > b.dueDate ? 1 : a.title.localeCompare(b.title))); }

function dueRow(p: Item) {
  const n = daysUntil(p.dueDate)!; const d = parseISO(p.dueDate)!;
  const when = DAY[d.getDay()].slice(0, 3) + " " + fmt(p.dueDate);
  return rowItem(p, [when, p.dueLabel || stageLabel(p)].join(", "), dueShort(n), dueClass(n));
}

function calItem(p: Item) {
  const b = el("button", "calitem" + (isFinal(p) ? " done" : "")); b.type = "button";
  b.style.borderLeftColor = "var(--" + p.track + ")";
  b.appendChild(el("span", "cit", p.title));
  if (p.dueLabel) b.appendChild(el("span", "cil", p.dueLabel));
  const what = TRACKS[p.track].label + ", " + stageLabel(p) + (isFinal(p) ? " (finished)" : "") + (p.dueLabel ? ". " + p.dueLabel : "");
  b.title = p.title + ". " + what;
  b.setAttribute("aria-label", p.title + ". " + what + ". Open to edit.");
  b.addEventListener("click", () => openDialog(p));
  return b;
}

function renderCalendar() {
  const items = dated();
  const today = parseISO(todayISO())!, t = iso(today), ws = weekStart(today);
  const thisEnd = iso(addDays(ws, 6)), nextStart = iso(addDays(ws, 7)), nextEnd = iso(addDays(ws, 13));
  const open = items.filter((p) => !isFinal(p));
  fillList("c-late", open.filter((p) => p.dueDate < t).map(dueRow), "Nothing is late.");
  fillList("c-this", open.filter((p) => p.dueDate >= t && p.dueDate <= thisEnd).map(dueRow), "Nothing else is due this week.");
  fillList("c-next", open.filter((p) => p.dueDate >= nextStart && p.dueDate <= nextEnd).map(dueRow), "Nothing is due next week.");
  $("c-this-why").textContent = "Today through Saturday, " + fmt(thisEnd) + ".";
  $("c-next-why").textContent = fmt(nextStart) + " to " + fmt(nextEnd) + ".";

  const byDay = new Map<string, Item[]>();
  items.forEach((p) => { const a = byDay.get(p.dueDate); if (a) a.push(p); else byDay.set(p.dueDate, [p]); });
  const y = calMonth.getFullYear(), m = calMonth.getMonth();
  $("cal-title").textContent = MONTH[m] + " " + y;
  const last = new Date(y, m + 1, 0);
  const grid = $("cal-grid"); grid.textContent = "";
  DAY.forEach((d) => { const h = el("div", "dow", d.slice(0, 3)); h.setAttribute("aria-hidden", "true"); grid.appendChild(h); });
  const agenda = $("cal-agenda"); agenda.textContent = "";
  for (let d = weekStart(calMonth); d <= last || d.getDay() !== 0; d = addDays(d, 1)) {
    const k = iso(d), here = byDay.get(k) || [];
    const cell = el("div", "day" + (d.getMonth() !== m ? " out" : "") + (k === t ? " today" : "") + (k >= iso(ws) && k <= thisEnd ? " thisweek" : ""));
    cell.setAttribute("role", "group");
    cell.setAttribute("aria-label", DAY[d.getDay()] + ", " + MONTH[d.getMonth()] + " " + d.getDate() + (k === t ? ", today" : "") + (here.length ? ", " + here.length + " due" : ""));
    cell.appendChild(el("span", "dn", String(d.getDate())));
    here.forEach((p) => cell.appendChild(calItem(p)));
    grid.appendChild(cell);
    if (d.getMonth() === m && here.length) {
      const li = el("li", k === t ? "today" : "");
      li.appendChild(el("h3", null, DAY[d.getDay()] + ", " + MONTH[m] + " " + d.getDate() + (k === t ? " (today)" : "")));
      here.forEach((p) => li.appendChild(calItem(p)));
      agenda.appendChild(li);
    }
  }
  if (!agenda.childNodes.length) { const li = el("li"); li.appendChild(el("p", "none", "Nothing is due in " + MONTH[m] + ".")); agenda.appendChild(li); }

  // Cards that name a date in words but have none set can't show up here.
  const undated = state.projects.filter((p) => !isFinal(p) && p.dueLabel && !parseISO(p.dueDate) && matches(p));
  const u = $("cal-undated"); u.textContent = ""; u.hidden = !undated.length;
  if (undated.length) {
    u.appendChild(el("p", "hint", undated.length === 1 ? "This card mentions a date but has no due date set, so it isn't on the calendar:" : "These cards mention a date but have no due date set, so they aren't on the calendar:"));
    const wrap = el("div", "uitems"); undated.forEach((p) => wrap.appendChild(calItem(p))); u.appendChild(wrap);
  }
}

function setView(v: "board" | "calendar") {
  state.view = v; lsSet("pb-view", v);
  $("v-board").setAttribute("aria-pressed", String(v === "board"));
  $("v-cal").setAttribute("aria-pressed", String(v === "calendar"));
  renderAll();
}
function renderView() { if (state.view === "calendar") renderCalendar(); else renderBoards(); }

function renderAll() {
  const cal = state.view === "calendar";
  $("welcome").hidden = state.exists;
  $("week").hidden = !state.exists || cal;
  $("filters").hidden = !state.exists;
  $("boards").hidden = cal;
  $("cal").hidden = !state.exists || !cal;
  renderTally(); renderWeek(); renderFilters(); renderView(); renderSectionChoosers();
}

/* ---------- loading ---------- */
let refreshing = false, again = false;
async function refresh() {
  if (refreshing) { again = true; return; }
  refreshing = true;
  try {
    const r = await call<{ projects: Item[]; summary: Summary; exists: boolean }>("load_board");
    state.projects = r.projects; state.summary = r.summary; state.exists = r.exists; state.loaded = true;
    if (!r.exists) { setStatus(""); await showWelcome(); }
    else setStatus(state.projects.length ? "" : "No projects on the board yet. Use Add item to put the first one on.");
    $("note").hidden = !state.exists || !state.projects.length || lsGet("pb-note") !== "show";
    renderAll();
    // Keep an open dialog's activity list current, unless the user is mid-edit.
    if (current && !byId(current.id) && $<HTMLDialogElement>("dlg").open) showMsg("This item was removed from the board.");
  } catch (e) {
    setStatus("Couldn't read the board: " + errText(e), true);
  } finally {
    refreshing = false;
    if (again) { again = false; void refresh(); }
  }
}

async function showWelcome() {
  const seed = await call<string | null>("find_seed").catch(() => null);
  $("w-seed").hidden = !seed; $("w-import-seed").hidden = !seed;
  if (seed) { $("w-seed-path").textContent = seed; $("w-import-seed").dataset.path = seed; }
}
async function doImport(path: string | null, msgId: string) {
  const m = $(msgId);
  try {
    const r = await call<{ cancelled?: boolean; count: number; added: number; replaced: number }>("import_file", { path });
    if (r.cancelled) return;
    m.hidden = false; m.classList.add("ok");
    m.textContent = "Imported " + r.count + " items (" + r.added + " new, " + r.replaced + " replaced).";
    lsSet("pb-note", "show");
    await refresh();
  } catch (e) { m.hidden = false; m.classList.remove("ok"); m.textContent = errText(e); }
}

/* ---------- drag a card to another stage ---------- */
// Built on pointer events rather than the browser's own drag and drop, which
// the desktop window doesn't deliver reliably. A mouse drag starts after a few
// pixels of movement; a finger has to rest on the card briefly first, so that
// swiping still scrolls the page.
function wireDrop(col: HTMLElement, track: string, stage: string) {
  col.dataset.track = track; col.dataset.stage = stage;
}

interface Drag {
  p: Item; card: HTMLElement; id: number; touch: boolean;
  x0: number; y0: number; x: number; y: number; dx: number; dy: number;
  ghost: HTMLElement | null; over: HTMLElement | null; timer: number; frame: number;
}
let drag: Drag | null = null;
let swallowClick = 0;

function pressCard(e: PointerEvent, card: HTMLElement, p: Item) {
  if (drag || !e.isPrimary || (e.pointerType === "mouse" && e.button !== 0)) return;
  const touch = e.pointerType !== "mouse";
  drag = { p, card, id: e.pointerId, touch, x0: e.clientX, y0: e.clientY, x: e.clientX, y: e.clientY, dx: 0, dy: 0, ghost: null, over: null, timer: 0, frame: 0 };
  if (touch) drag.timer = window.setTimeout(() => { if (drag && !drag.ghost) beginDrag(); }, 350);
  window.addEventListener("pointermove", onDragMove, { passive: false });
  window.addEventListener("pointerup", onDragEnd);
  window.addEventListener("pointercancel", onDragCancel);
}

function beginDrag() {
  const d = drag!; const r = d.card.getBoundingClientRect();
  d.dx = d.x0 - r.left; d.dy = d.y0 - r.top;
  const g = d.card.cloneNode(true) as HTMLElement;
  g.classList.add("ghost"); g.removeAttribute("tabindex"); g.setAttribute("aria-hidden", "true");
  g.style.width = r.width + "px";
  document.body.appendChild(g);
  d.ghost = g;
  d.card.classList.add("dragging");
  document.body.classList.add("dragging-card");
  try { d.card.setPointerCapture(d.id); } catch { /* the pointer may already be gone */ }
  place();
  d.frame = requestAnimationFrame(autoScroll);
}

function place() {
  const d = drag!; if (!d.ghost) return;
  d.ghost.style.transform = "translate(" + (d.x - d.dx) + "px," + (d.y - d.dy) + "px) rotate(1.5deg)";
  const under = document.elementFromPoint(d.x, d.y);
  const col = (under?.closest(".col[data-track]") as HTMLElement | null) ?? null;
  if (col !== d.over) { d.over?.classList.remove("over"); col?.classList.add("over"); d.over = col; }
}

function onDragMove(e: PointerEvent) {
  const d = drag; if (!d || e.pointerId !== d.id) return;
  d.x = e.clientX; d.y = e.clientY;
  if (!d.ghost) {
    const far = Math.hypot(d.x - d.x0, d.y - d.y0) > (d.touch ? 10 : 5);
    if (!far) return;
    if (d.touch) { finishDrag(); return; } // moved before the hold: it's a scroll
    beginDrag();
  }
  e.preventDefault();
  place();
}

// While a finger drag is under way, stop the page from scrolling under it.
document.addEventListener("touchmove", (e) => { if (drag?.ghost) e.preventDefault(); }, { passive: false });

function onDragEnd(e: PointerEvent) {
  const d = drag; if (!d || e.pointerId !== d.id) return;
  const moved = !!d.ghost; const col = d.over;
  finishDrag();
  if (!moved) return;
  swallowClick = Date.now();
  if (col?.dataset.track && col.dataset.stage) void moveTo(d.p, col.dataset.track, col.dataset.stage);
}

function onDragCancel(e: PointerEvent) { if (drag && e.pointerId === drag.id) finishDrag(); }

function finishDrag() {
  const d = drag; if (!d) return;
  clearTimeout(d.timer); cancelAnimationFrame(d.frame);
  d.ghost?.remove(); d.over?.classList.remove("over");
  d.card.classList.remove("dragging");
  document.body.classList.remove("dragging-card");
  try { d.card.releasePointerCapture(d.id); } catch { /* already released */ }
  window.removeEventListener("pointermove", onDragMove);
  window.removeEventListener("pointerup", onDragEnd);
  window.removeEventListener("pointercancel", onDragCancel);
  drag = null;
}

// Scroll the board, or a track's row of stages, when the card nears an edge.
function autoScroll() {
  const d = drag; if (!d?.ghost) return;
  const edge = 48, step = (dist: number) => Math.ceil((edge - dist) / 4);
  const main = document.querySelector(".main") as HTMLElement | null;
  if (main) {
    const r = main.getBoundingClientRect();
    if (d.y < r.top + edge) main.scrollTop -= step(d.y - r.top);
    else if (d.y > r.bottom - edge) main.scrollTop += step(r.bottom - d.y);
  }
  const sc = d.over?.closest(".scroller") as HTMLElement | null;
  if (sc && sc.scrollWidth > sc.clientWidth) {
    const r = sc.getBoundingClientRect();
    if (d.x < r.left + edge) sc.scrollLeft -= step(d.x - r.left);
    else if (d.x > r.right - edge) sc.scrollLeft += step(r.right - d.x);
  }
  place();
  d.frame = requestAnimationFrame(autoScroll);
}

document.addEventListener("keydown", (e) => { if (e.key === "Escape" && drag?.ghost) { e.preventDefault(); e.stopPropagation(); finishDrag(); } }, true);
// The click that ends a drag shouldn't also open the card.
window.addEventListener("click", (e) => { if (Date.now() - swallowClick < 400) { e.preventDefault(); e.stopPropagation(); swallowClick = 0; } }, true);
async function moveTo(p: Item, track: string, stage: string) {
  if (p.track === track && stageIndex(p) === stageIndex({ track, stage })) return;
  try { await call("move_item", { id: p.id, track, stage }); await refresh(); }
  catch (e) { setStatus(errText(e), true); }
}

/* ---------- dialog ---------- */
function fillStages(track: string, stage: string) {
  const sel = $<HTMLSelectElement>("f-stage"); sel.textContent = "";
  stagesOf(track).forEach((s) => { const o = el("option", null, s[1]); o.value = s[0]; sel.appendChild(o); });
  const ok = stagesOf(track).some((s) => s[0] === stage);
  sel.value = ok ? stage : stagesOf(track)[0][0];
  updateAdvance();
}
function updateAdvance() {
  const st = stagesOf($<HTMLSelectElement>("f-track").value); let i = 0;
  for (let j = 0; j < st.length; j++) if (st[j][0] === $<HTMLSelectElement>("f-stage").value) i = j;
  const b = $("d-advance");
  if (i >= st.length - 1) { b.hidden = true; } else { b.hidden = false; b.textContent = "Advance to " + st[i + 1][1]; b.dataset.to = st[i + 1][0]; }
}
function showMsg(t: string) { const m = $("d-msg"); m.textContent = t || ""; m.hidden = !t; }
const val = (id: string) => $<HTMLInputElement>(id);
function openDialog(p: Item | null) {
  current = p || null;
  $("d-h").textContent = p ? p.title : "Add an item";
  const tr = $<HTMLSelectElement>("f-track"); tr.textContent = "";
  const pick = shownTracks(); if (p && !pick.includes(p.track)) pick.push(p.track);
  ORDER.filter((k) => pick.includes(k)).forEach((k) => { const o = el("option", null, TRACKS[k].label); o.value = k; tr.appendChild(o); });
  const track = p ? p.track : (state.filter !== "all" ? state.filter : pick[0]);
  tr.value = track; fillStages(track, p ? p.stage : "");
  val("f-title").value = p ? p.title : ""; val("f-kind").value = p ? p.kind : ""; val("f-byline").value = p ? p.byline : "";
  val("f-status").value = p ? p.status : ""; val("f-next").value = p ? p.next : "";
  val("f-open").value = p ? p.open.join("\n") : ""; val("f-decisions").value = p ? p.decisions.join("\n") : "";
  val("f-words").value = p ? p.words : ""; val("f-touched").value = p ? p.touched : todayISO();
  val("f-dueLabel").value = p ? p.dueLabel : ""; val("f-dueDate").value = p ? p.dueDate : ""; $("f-due-hint").textContent = dueHint(val("f-dueDate").value); val("f-files").value = p ? p.files : ""; val("f-links").value = p ? (p.links || []).join("\n") : "";
  const lg = $("d-log"); lg.textContent = "";
  (p ? p.log : []).slice(-6).reverse().forEach((e) => { if (e && e.t) lg.appendChild(el("li", null, (fmt(e.d) || "") + "  " + e.t)); });
  $("d-remove").hidden = !p; $("d-confirm").hidden = true; showMsg(""); $<HTMLButtonElement>("d-save").disabled = false;
  $<HTMLDialogElement>("dlg").showModal();
  val("f-title").focus();
}
function closeDialog() { const dlg = $<HTMLDialogElement>("dlg"); if (dlg.open) dlg.close(); current = null; }
function lines(v: string) { return v.split("\n").map((s) => s.trim()).filter(Boolean).slice(0, 40); }
function collect() {
  return {
    title: val("f-title").value.trim(), track: val("f-track").value, stage: val("f-stage").value, kind: val("f-kind").value.trim(), byline: val("f-byline").value.trim(),
    words: val("f-words").value.trim(), status: val("f-status").value.trim(), next: val("f-next").value.trim(), open: lines(val("f-open").value), decisions: lines(val("f-decisions").value),
    files: val("f-files").value.trim(), links: lines(val("f-links").value), dueLabel: val("f-dueLabel").value.trim(), dueDate: val("f-dueDate").value, touched: val("f-touched").value,
  };
}
async function save() {
  if (saving) return;
  const data: Record<string, unknown> & ReturnType<typeof collect> = collect();
  if (!data.title) { showMsg("Give the project a title."); return; }
  const prev = current, log = (prev ? prev.log : []).slice(-11), t = todayISO();
  if (!prev) log.push({ d: t, t: "Added to the board" });
  else if (prev.stage !== data.stage || prev.track !== data.track) {
    const to = stageLabel({ track: data.track, stage: data.stage });
    log.push({ d: t, t: prev.track === data.track ? "Stage: " + stageLabel(prev) + " to " + to : "Moved to " + TRACKS[data.track].label + ": " + to });
    if (data.touched === prev.touched) data.touched = t;
  }
  data.log = log;
  if (prev) data.id = prev.id;
  saving = true; $<HTMLButtonElement>("d-save").disabled = true; showMsg("");
  try { await call("save_item", { item: data, isNew: !prev }); saving = false; closeDialog(); await refresh(); }
  catch (e) { saving = false; $<HTMLButtonElement>("d-save").disabled = false; showMsg(errText(e)); }
}

/* ---------- wiring ---------- */
function wire() {
  $("dform").addEventListener("submit", (e) => { e.preventDefault(); void save(); });
  $("d-cancel").addEventListener("click", closeDialog);
  $("dlg").addEventListener("close", () => { current = null; });
  $("d-today").addEventListener("click", () => { val("f-touched").value = todayISO(); });
  $("d-advance").addEventListener("click", function (this: HTMLElement) { val("f-stage").value = this.dataset.to || ""; updateAdvance(); });
  $("f-track").addEventListener("change", function (this: HTMLSelectElement) { fillStages(this.value, ""); });
  $("f-stage").addEventListener("change", updateAdvance);
  $("d-remove").addEventListener("click", () => { $("d-confirm").hidden = false; });
  $("d-remove-no").addEventListener("click", () => { $("d-confirm").hidden = true; });
  $("d-remove-yes").addEventListener("click", async () => {
    if (!current) return;
    try { await call("delete_item", { id: current.id }); closeDialog(); await refresh(); } catch (e) { showMsg(errText(e)); }
  });
  $("add").addEventListener("click", () => openDialog(null));
  $("f-dueDate").addEventListener("input", () => { $("f-due-hint").textContent = dueHint(val("f-dueDate").value); });
  $("v-board").addEventListener("click", () => setView("board"));
  $("v-cal").addEventListener("click", () => setView("calendar"));
  const shift = (n: number) => { calMonth = new Date(calMonth.getFullYear(), calMonth.getMonth() + n, 1); renderCalendar(); };
  $("cal-prev").addEventListener("click", () => shift(-1));
  $("cal-next").addEventListener("click", () => shift(1));
  $("cal-today").addEventListener("click", () => { const d = new Date(); calMonth = new Date(d.getFullYear(), d.getMonth(), 1); renderCalendar(); });
  let qt: number | undefined;
  $("q").addEventListener("input", function (this: HTMLInputElement) { const v = this.value; clearTimeout(qt); qt = window.setTimeout(() => { state.q = v.trim().toLowerCase(); renderView(); }, 120); });
  $("note-x").addEventListener("click", () => { $("note").hidden = true; lsSet("pb-note", "1"); });
  $("w-import-seed").addEventListener("click", () => void doImport($("w-import-seed").dataset.path || null, "w-msg"));
  $("w-import").addEventListener("click", () => void doImport(null, "w-msg"));
  $("w-empty").addEventListener("click", async () => { try { await call("start_empty"); await refresh(); } catch (e) { const m = $("w-msg"); m.hidden = false; m.textContent = errText(e); } });
}

async function boot() {
  wire();
  try { info = await call<Info>("app_info"); }
  catch (e) { setStatus("The app couldn't start its data layer: " + errText(e), true); return; }
  info.tracks.forEach((t) => { TRACKS[t.key] = { label: t.label, short: t.short, stages: t.stages }; });
  ORDER = info.tracks.map((t) => t.key);
  loadHidden();
  const saved = lsGet("pb-filter"); if (saved && (saved === "all" || TRACKS[saved])) state.filter = saved;
  if (lsGet("pb-view") === "calendar") { state.view = "calendar"; $("v-board").setAttribute("aria-pressed", "false"); $("v-cal").setAttribute("aria-pressed", "true"); }
  renderAll();
  await refresh();
  let pending: number | undefined;
  await onBoardChanged(() => { clearTimeout(pending); pending = window.setTimeout(() => void refresh(), 150); });
  if (isWeb) {
    // The web page shows the board itself; the assistant and settings live in the desktop app.
    $("assist-toggle").hidden = true;
    $("open-settings").hidden = true;
    const out = $("web-signout"); out.hidden = false;
    out.addEventListener("click", async () => { await fetch("/api/logout", { method: "POST", credentials: "same-origin" }).catch(() => {}); location.href = "/login"; });
    return;
  }
  setupAssistant(refresh);
  setupSettings(info, refresh, (path) => doImport(path, "s-data-msg"));
  setupSync(info, refresh);
}

void boot();
