import { call, $, el, errText, lsGet, lsSet } from "./shared";

interface Pending { op: string; summary: string; [k: string]: unknown }
interface SendResult { reply: string; pending: Pending[]; needsConfirm: boolean }
interface SettingsInfo { settings: { mode: string; agentPath: string }; agentFound: string | null; hasKey: boolean }

const SUGGESTIONS = [
  "What should I pick up this week?",
  "Which decisions are waiting on me, and on which projects?",
  "What has gone cold that I should either restart or set aside?",
];

let refreshBoard: () => Promise<void> = async () => {};
let busy = false;
let ready = false;

function log() { return $("a-log"); }
function scrollDown() { const l = log(); l.scrollTop = l.scrollHeight; }
function bubble(cls: string, text: string) { const b = el("div", "bubble " + cls, text); log().appendChild(b); scrollDown(); return b; }

function setBusy(on: boolean) {
  busy = on;
  $<HTMLButtonElement>("a-send").disabled = on || !ready;
  $("a-stop").hidden = !on;
  $<HTMLButtonElement>("a-reset").disabled = on;
}

function showSuggestions() {
  if (!ready || log().querySelector(".bubble")) return;
  const box = el("div", "suggest");
  box.appendChild(el("p", "hint", "Ask about the board, or tell it what changed. For example:"));
  SUGGESTIONS.forEach((s) => {
    const b = el("button", null, s); b.type = "button";
    b.addEventListener("click", () => { $<HTMLTextAreaElement>("a-text").value = s; void send(); });
    box.appendChild(b);
  });
  log().appendChild(box);
}

/** Re-read settings and show either the conversation or how to set it up. */
export async function updateAssistantMode() {
  let s: SettingsInfo;
  try { s = await call<SettingsInfo>("get_settings"); } catch { return; }
  const mode = s.settings.mode;
  const label = $("a-mode");
  let problem = "";
  if (mode === "agent") {
    label.textContent = "installed agent";
    if (!s.agentFound) problem = "The coding agent wasn't found on this PC. Set its location in Settings.";
    else {
      const a = await call<{ signedIn: boolean | null }>("agent_status", { path: s.settings.agentPath || "" }).catch(() => ({ signedIn: null }));
      if (a.signedIn === false) problem = "The coding agent isn't signed in on this PC yet. Open Settings and choose Sign in to the agent. It opens the agent's own sign-in; this app never sees your account.";
    }
  }
  else if (mode === "api") { label.textContent = "API key"; if (!s.hasKey) problem = "No API key is stored yet. Paste one in Settings."; }
  else { label.textContent = "off"; problem = "off"; }
  ready = !problem;
  const old = log().querySelector(".setup"); if (old) old.remove();
  if (!ready) {
    const box = el("div", "setup");
    if (problem === "off") {
      box.appendChild(el("p", null, "The assistant can answer questions about the board and make changes you describe in plain language."));
      box.appendChild(el("p", null, "To switch it on, open Settings and choose how it connects: the coding agent already installed and signed in on this PC, or your own API key. Everything else on the board works without it."));
    } else box.appendChild(el("p", null, problem));
    const b = el("button", "btn small", "Open settings"); b.type = "button";
    b.addEventListener("click", () => $("open-settings").click());
    box.appendChild(b);
    log().prepend(box);
    log().querySelectorAll(".suggest").forEach((n) => n.remove());
  } else showSuggestions();
  setBusy(busy);
}

function changeList(items: string[], cls = "") {
  const ul = el("ul");
  items.forEach((t) => ul.appendChild(el("li", cls || null, t)));
  return ul;
}

async function apply(card: HTMLElement, pending: Pending[]) {
  card.textContent = "";
  try {
    const r = await call<{ undo: number; done: string[]; failed: string[] }>("apply_changes", { changes: pending });
    card.className = "changes done";
    card.appendChild(el("span", "ch", r.done.length === 1 ? "Changed on the board" : r.done.length + " changes made on the board"));
    if (r.done.length) card.appendChild(changeList(r.done));
    if (r.failed.length) { card.appendChild(el("span", "fail", "Couldn't apply:")); card.appendChild(changeList(r.failed, "fail")); }
    if (r.done.length) {
      const u = el("button", "btn small", "Undo"); u.type = "button";
      u.addEventListener("click", async () => {
        u.disabled = true;
        try { await call("undo_changes", { token: r.undo }); u.replaceWith(el("span", "hint", "Undone.")); await refreshBoard(); }
        catch (e) { u.replaceWith(el("span", "fail", errText(e))); }
      });
      const a = el("div", "actions"); a.appendChild(u); card.appendChild(a);
    }
    await refreshBoard();
  } catch (e) {
    card.appendChild(el("span", "fail", "Couldn't apply the changes: " + errText(e)));
  }
  scrollDown();
}

function showChanges(pending: Pending[], needsConfirm: boolean) {
  const card = el("div", "changes");
  log().appendChild(card);
  if (!needsConfirm) { void apply(card, pending); return; }
  card.appendChild(el("span", "ch", "These changes wait for you"));
  card.appendChild(changeList(pending.map((p) => p.summary)));
  const a = el("div", "actions");
  const yes = el("button", "btn small primary", "Apply"); yes.type = "button";
  const no = el("button", "btn small", "Discard"); no.type = "button";
  yes.addEventListener("click", () => void apply(card, pending));
  no.addEventListener("click", () => { card.className = "changes done"; card.textContent = ""; card.appendChild(el("span", "hint", "Discarded. Nothing on the board changed.")); });
  a.appendChild(yes); a.appendChild(no); card.appendChild(a);
  scrollDown();
}

async function send() {
  const box = $<HTMLTextAreaElement>("a-text");
  const text = box.value.trim();
  if (!text || busy || !ready) return;
  log().querySelectorAll(".suggest").forEach((n) => n.remove());
  box.value = "";
  bubble("user", text);
  const work = bubble("work", "Working");
  const started = Date.now();
  const tick = window.setInterval(() => { work.textContent = "Working, " + Math.round((Date.now() - started) / 1000) + "s"; }, 1000);
  setBusy(true);
  try {
    const r = await call<SendResult>("assistant_send", { text });
    work.remove();
    bubble("bot", r.reply);
    if (r.pending.length) showChanges(r.pending, r.needsConfirm);
  } catch (e) {
    work.remove();
    bubble("err", errText(e));
  } finally {
    clearInterval(tick);
    setBusy(false);
    box.focus();
  }
}

function setOpen(open: boolean) {
  $("assist").hidden = !open;
  $("app").classList.toggle("with-assist", open);
  $("assist-toggle").setAttribute("aria-pressed", String(open));
  lsSet("pb-assist", open ? "1" : "");
  if (open) { void updateAssistantMode(); $<HTMLTextAreaElement>("a-text").focus(); }
}

export function setupAssistant(refresh: () => Promise<void>) {
  refreshBoard = refresh;
  $("assist-toggle").addEventListener("click", () => setOpen($("assist").hidden));
  $("a-close").addEventListener("click", () => setOpen(false));
  $("a-form").addEventListener("submit", (e) => { e.preventDefault(); void send(); });
  $("a-text").addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void send(); } });
  $("a-stop").addEventListener("click", () => void call("assistant_stop"));
  $("a-reset").addEventListener("click", async () => {
    try { await call("assistant_reset"); log().textContent = ""; await updateAssistantMode(); }
    catch (e) { bubble("err", errText(e)); }
  });
  setOpen(lsGet("pb-assist") === "1");
}
