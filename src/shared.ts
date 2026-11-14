import { invoke } from "@tauri-apps/api/core";

export interface LogEntry { d: string; t: string }
export interface Item {
  id: string; title: string; track: string; stage: string; kind: string; byline: string; words: string;
  status: string; next: string; open: string[]; decisions: string[]; files: string; links: string[];
  dueLabel: string; dueDate: string; touched: string; log: LogEntry[];
}
export interface Row { id: string; days: number | null }
export interface Summary { closest: Row[]; due: Row[]; cold: Row[]; inFlight: number; decisionsWaiting: number }
export interface Info {
  dataDir: string; dataFile: string; backupsDir: string; exists: boolean; exe: string; version: string; today: string;
  tracks: { key: string; label: string; short: string; stages: [string, string][] }[];
}

/** True when the board runs as a web page on the server rather than in the desktop window. */
export const isWeb = !("__TAURI_INTERNALS__" in window);

export async function call<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isWeb) return invoke<T>(cmd, args);
  const r = await fetch("/api/call/" + cmd, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(args ?? {}), credentials: "same-origin" });
  if (r.status === 401) { location.href = "/login"; throw "You've been signed out."; }
  const j = await r.json().catch(() => null);
  if (!r.ok) throw (j && j.error) || "The server answered with an error.";
  return j as T;
}

/** Desktop events from the app's background work. The web page has none. */
export async function onEvent<T>(name: string, cb: (payload: T) => void) {
  if (isWeb) return;
  const { listen } = await import("@tauri-apps/api/event");
  await listen<T>(name, (e) => cb(e.payload));
}

/** Call `cb` when the board changes somewhere else: the file on disk on the
 *  desktop, or the server's copy (checked every few seconds) on the web. */
export async function onBoardChanged(cb: () => void) {
  if (!isWeb) return onEvent("board-changed", cb);
  let seq = -1;
  const check = async () => {
    if (document.hidden) return;
    try {
      const r = await fetch("/api/version", { credentials: "same-origin" });
      if (r.status === 401) { location.href = "/login"; return; }
      const j = await r.json();
      if (seq >= 0 && j.seq !== seq) cb();
      seq = j.seq;
    } catch { /* offline for a moment */ }
  };
  await check();
  window.setInterval(() => void check(), 5000);
  document.addEventListener("visibilitychange", () => void check());
}

export function $<T extends HTMLElement = HTMLElement>(id: string): T {
  return document.getElementById(id) as T;
}

export function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string | null, text?: string | null): HTMLElementTagNameMap[K] {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = text;
  return n;
}

export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return "Something went wrong.";
}

export function lsGet(k: string): string | null { try { return localStorage.getItem(k); } catch { return null; } }
export function lsSet(k: string, v: string) { try { localStorage.setItem(k, v); } catch { /* storage unavailable */ } }
