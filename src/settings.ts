import { call, $, errText, type Info } from "./shared";
import { updateAssistantMode } from "./assistant";
import { describe, onSyncStatus, setStatus, syncNow, type SyncStatus } from "./sync";

interface Settings { mode: string; agentPath: string; model: string; effort: string; syncServer: string }
interface SettingsInfo { settings: Settings; agentFound: string | null; hasKey: boolean }

let cur: Settings = { mode: "off", agentPath: "", model: "", effort: "medium", syncServer: "" };

function showSync(st: SyncStatus) {
  $("s-sync-setup").hidden = st.configured && !!st.hasKey;
  $("s-sync-on").hidden = !st.configured || !st.hasKey;
  if (!st.configured) { $("s-sync-state").textContent = "Sync is off. The board stays on this computer only."; return; }
  const d = describe(st);
  const server = st.server ?? "";
  $("s-sync-state").textContent = "Syncing with " + server.replace(/^https:\/\//, "") + ". " + (d.detail && d.tone !== "good" ? d.detail : d.text + ".");
  const a = $<HTMLAnchorElement>("s-web-url"); a.href = server; a.textContent = server.replace(/^https:\/\//, "");
  $("s-mcp-url").textContent = server + "/mcp";
  if (!st.hasKey) input("s-sync-server").value = server.replace(/^https:\/\//, "");
}

/* ---------- limited access keys for other tools ---------- */
interface AccessKey { id: string; name: string; write: boolean; tracks: string[]; created: string; lastUsed: string }
let trackLabels: Record<string, string> = {};

function describeKey(k: AccessKey) {
  const what = k.write ? "Can change cards" : "Read only";
  const where = k.tracks.length ? k.tracks.map((t) => trackLabels[t] || t).join(", ") : "every track";
  const used = k.lastUsed ? "last used " + k.lastUsed : "not used yet";
  return what + " · " + where + " · made " + k.created + ", " + used;
}

async function loadKeys() {
  const ul = $("s-keys");
  try {
    const r = await call<{ keys: AccessKey[] }>("sync_keys", { action: "list", args: {} });
    ul.textContent = "";
    if (!r.keys.length) { const li = document.createElement("li"); li.className = "empty"; li.textContent = "No extra keys yet."; ul.appendChild(li); return; }
    for (const k of r.keys) {
      const li = document.createElement("li");
      const n = document.createElement("span"); n.className = "kn"; n.textContent = k.name;
      const d = document.createElement("span"); d.className = "kd"; d.textContent = describeKey(k);
      const b = document.createElement("button"); b.type = "button"; b.className = "btn small quiet danger"; b.textContent = "Revoke";
      b.setAttribute("aria-label", "Revoke the key " + k.name);
      b.addEventListener("click", async () => {
        if (b.dataset.armed !== "1") { b.dataset.armed = "1"; b.textContent = "Revoke: are you sure?"; return; }
        b.disabled = true;
        try { await call("sync_keys", { action: "revoke", args: { id: k.id } }); msg("s-sync-msg", "Revoked the key " + k.name + ".", true); await loadKeys(); }
        catch (e) { b.disabled = false; msg("s-sync-msg", errText(e)); }
      });
      li.appendChild(n); li.appendChild(b); li.appendChild(d);
      ul.appendChild(li);
    }
  } catch (e) { ul.textContent = ""; const li = document.createElement("li"); li.className = "empty"; li.textContent = "Couldn't list the keys: " + errText(e); ul.appendChild(li); }
}

function setupKeys(info: Info) {
  const box = $("s-k-tracks");
  info.tracks.forEach((t) => {
    trackLabels[t.key] = t.label;
    const l = document.createElement("label"); l.className = "check";
    const c = document.createElement("input"); c.type = "checkbox"; c.value = t.key; c.name = "s-k-track";
    l.appendChild(c); l.appendChild(document.createTextNode(" " + t.label));
    box.appendChild(l);
  });
  input("s-k-all").addEventListener("change", () => { box.hidden = input("s-k-all").checked; });
  $("s-k-make").addEventListener("click", async () => {
    const tracks = input("s-k-all").checked ? [] : Array.from(document.querySelectorAll<HTMLInputElement>('input[name="s-k-track"]:checked')).map((c) => c.value);
    if (!input("s-k-all").checked && !tracks.length) { msg("s-sync-msg", "Choose at least one track, or Every track."); return; }
    try {
      const r = await call<AccessKey & { key: string }>("sync_keys", { action: "create", args: { name: input("s-k-name").value, write: input("s-k-write").checked, tracks } });
      input("s-k-value").value = r.key; $("s-k-new").hidden = false; input("s-k-value").select();
      input("s-k-name").value = "";
      msg("s-sync-msg", "Made the key " + r.name + ". Copy it now; it won't be shown again.", true);
      await loadKeys();
    } catch (e) { msg("s-sync-msg", errText(e)); }
  });
  $("s-k-copy").addEventListener("click", async () => {
    try { await navigator.clipboard.writeText(input("s-k-value").value); msg("s-sync-msg", "Key copied. Paste it only into the tool it's for.", true); }
    catch (e) { msg("s-sync-msg", errText(e)); }
  });
}

function msg(id: string, text: string, ok = false) { const m = $(id); m.textContent = text; m.hidden = !text; m.classList.toggle("ok", ok); }
const input = (id: string) => $<HTMLInputElement>(id);

function showMode() {
  $("s-agent").hidden = cur.mode !== "agent";
  $("s-api").hidden = cur.mode !== "api";
  if (cur.mode === "agent") void checkAuth();
}

async function checkAuth() {
  const line = $("s-agent-auth");
  line.textContent = "Checking…";
  try {
    const s = await call<{ found: string | null; signedIn: boolean | null }>("agent_status", { path: input("s-agent-path").value.trim() });
    $("s-agent-signin").hidden = s.signedIn === true || !s.found;
    if (!s.found) line.textContent = "Find the agent first.";
    else if (s.signedIn === true) line.textContent = "The agent is signed in.";
    else if (s.signedIn === false) line.textContent = "The agent isn't signed in yet. Sign in opens the agent's own sign-in window; finish there, then choose Check again.";
    else line.textContent = "Couldn't tell whether the agent is signed in.";
  } catch (e) { line.textContent = errText(e); }
}

async function load() {
  const s = await call<SettingsInfo>("get_settings");
  cur = s.settings;
  document.querySelectorAll<HTMLInputElement>('input[name="mode"]').forEach((r) => { r.checked = r.value === cur.mode; });
  input("s-agent-path").value = cur.agentPath;
  $("s-agent-found").textContent = s.agentFound ? "Using the agent at " + s.agentFound : "The coding agent wasn't found automatically. Use Browse to point to its program.";
  $("s-key-state").textContent = s.hasKey ? "A key is stored in this computer's secure credential store." : "No key stored.";
  input("s-key").value = "";
  input("s-model").value = cur.model;
  $<HTMLSelectElement>("s-effort").value = cur.effort || "medium";
  ["s-agent-msg", "s-api-msg", "s-data-msg", "s-sync-msg"].forEach((m) => msg(m, ""));
  if (!input("s-sync-server").value) input("s-sync-server").value = (cur.syncServer || "").replace(/^https:\/\//, "");
  input("s-sync-key").value = ""; input("s-web-pw").value = "";
  input("s-k-value").value = ""; $("s-k-new").hidden = true;
  const st = await call<SyncStatus>("sync_status");
  setStatus(st);
  if (st.configured && st.hasKey) void loadKeys();
  showMode();
}

async function persist() {
  cur.agentPath = input("s-agent-path").value.trim();
  cur.model = input("s-model").value.trim();
  cur.effort = $<HTMLSelectElement>("s-effort").value;
  await call("set_settings", { new: cur });
  const s = await call<SettingsInfo>("get_settings");
  $("s-agent-found").textContent = s.agentFound ? "Using the agent at " + s.agentFound : "The coding agent wasn't found. Use Browse to point to its program.";
  void updateAssistantMode();
}

export function setupSettings(info: Info, refresh: () => Promise<void>, importFile: (path: string | null) => Promise<void>) {
  const dlg = $<HTMLDialogElement>("sdlg");
  $("s-path").textContent = info.dataFile;
  setupKeys(info);
  $("s-snippet").textContent = JSON.stringify({ "production-board": { command: info.exe, args: ["--mcp"] } }, null, 2);

  $("open-settings").addEventListener("click", async () => {
    try { await load(); } catch (e) { msg("s-data-msg", errText(e)); }
    dlg.showModal();
  });
  $("sform").addEventListener("submit", async (e) => { e.preventDefault(); try { await persist(); } catch { /* shown below */ } dlg.close(); });
  dlg.addEventListener("close", () => void persist().catch(() => {}));

  document.querySelectorAll<HTMLInputElement>('input[name="mode"]').forEach((r) => r.addEventListener("change", () => { if (r.checked) { cur.mode = r.value; showMode(); void persist(); } }));
  ["s-agent-path", "s-model"].forEach((id) => input(id).addEventListener("change", () => void persist()));
  $("s-effort").addEventListener("change", () => void persist());

  $("s-agent-browse").addEventListener("click", async () => {
    const p = await call<string | null>("pick_agent");
    if (p) { input("s-agent-path").value = p; await persist(); }
  });
  $("s-agent-test").addEventListener("click", async () => {
    msg("s-agent-msg", "Checking…", true);
    try { const v = await call<string>("test_agent", { path: input("s-agent-path").value.trim() }); msg("s-agent-msg", "The agent answered: " + v.split("\n")[0], true); }
    catch (e) { msg("s-agent-msg", errText(e)); }
  });
  $("s-agent-signin").addEventListener("click", async () => {
    try { await call("agent_sign_in", { path: input("s-agent-path").value.trim() }); $("s-agent-auth").textContent = "Finish signing in in the agent's window, then choose Check again."; }
    catch (e) { msg("s-agent-msg", errText(e)); }
  });
  $("s-agent-recheck").addEventListener("click", () => { void checkAuth(); void updateAssistantMode(); });
  $("s-connect").addEventListener("click", async () => {
    msg("s-agent-msg", "Connecting…", true);
    try { await call<string>("connect_agent", { path: input("s-agent-path").value.trim() }); msg("s-agent-msg", "Connected. New agent sessions on this PC can now read and update the board.", true); }
    catch (e) { msg("s-agent-msg", "Couldn't connect: " + errText(e)); }
  });

  $("s-key-save").addEventListener("click", async () => {
    try { await call("set_api_key", { key: input("s-key").value }); input("s-key").value = ""; $("s-key-state").textContent = "A key is stored in this computer's secure credential store."; msg("s-api-msg", "Key saved.", true); void updateAssistantMode(); }
    catch (e) { msg("s-api-msg", errText(e)); }
  });
  $("s-key-clear").addEventListener("click", async () => {
    try { await call("clear_api_key"); $("s-key-state").textContent = "No key stored."; msg("s-api-msg", "Key removed.", true); void updateAssistantMode(); }
    catch (e) { msg("s-api-msg", errText(e)); }
  });

  onSyncStatus(showSync);
  $("s-sync-connect").addEventListener("click", async () => {
    msg("s-sync-msg", "Checking the server…", true);
    try {
      const r = await call<{ items: number; passwordSet: boolean }>("sync_configure", { server: input("s-sync-server").value, key: input("s-sync-key").value });
      input("s-sync-key").value = "";
      const s = await call<SettingsInfo>("get_settings"); cur = s.settings;
      msg("s-sync-msg", "Connected. The server has " + r.items + " cards; syncing now." + (r.passwordSet ? "" : " Set a web page password below to use the board in a browser."), true);
      setStatus(await call<SyncStatus>("sync_status"));
      msg("s-sync-msg", await syncNow(), true);
    } catch (e) { msg("s-sync-msg", errText(e)); }
  });
  $("s-sync-now").addEventListener("click", async () => {
    msg("s-sync-msg", "Syncing…", true);
    try { msg("s-sync-msg", await syncNow(), true); } catch (e) { msg("s-sync-msg", errText(e)); }
  });
  $("s-sync-copy").addEventListener("click", async () => {
    try { const k = await call<string | null>("sync_key"); if (!k) throw "No access key is stored."; await navigator.clipboard.writeText(k); msg("s-sync-msg", "Access key copied. Paste it only into tools you trust.", true); }
    catch (e) { msg("s-sync-msg", errText(e)); }
  });
  $("s-sync-off").addEventListener("click", async () => {
    try { await call("sync_disable"); const s = await call<SettingsInfo>("get_settings"); cur = s.settings; setStatus({ configured: false }); msg("s-sync-msg", "Sync is off. The board on this computer is unchanged; the server keeps its copy.", true); }
    catch (e) { msg("s-sync-msg", errText(e)); }
  });
  $("s-web-pw-save").addEventListener("click", async () => {
    try { await call("sync_set_web_password", { password: input("s-web-pw").value }); input("s-web-pw").value = ""; msg("s-sync-msg", "Web page password set.", true); }
    catch (e) { msg("s-sync-msg", errText(e)); }
  });

  $("s-reveal").addEventListener("click", () => void call("reveal_data").catch((e) => msg("s-data-msg", errText(e))));
  $("s-export").addEventListener("click", async () => {
    try {
      const r = await call<{ cancelled?: boolean; path: string; count: number }>("export_board");
      if (!r.cancelled) msg("s-data-msg", "Exported " + r.count + " items to " + r.path, true);
    } catch (e) { msg("s-data-msg", errText(e)); }
  });
  $("s-import").addEventListener("click", async () => { await importFile(null); await refresh(); });
}
