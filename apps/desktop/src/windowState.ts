import { useLayoutEffect, useRef, useState, useEffect, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { flushShellSessions } from "./shellFlush";

export type Theme = "light" | "dark" | "system";

let restored: Record<string, unknown> = {};
const providers = new Map<string, () => unknown>();
const blockers = new Set<{ current: boolean }>();
let pending: number | null = null;
let saving = false;
let scheduled: ReturnType<typeof setTimeout> | undefined;
let reportError: (error: unknown) => void = () => undefined;

export async function restoreWindowState() {
  const snapshot = await invoke<string | null>("desktop_restore");
  restored = snapshot ? JSON.parse(snapshot) : {};
}
export const discardRestoredWindowState = () => { restored = {}; };
export const hasWindowState = (key: string) => Object.hasOwn(restored, key);
export const captureWindowState = () => Object.fromEntries([...providers].map(([key, read]) => [key, read()]));

/** Retain selected work in backend memory while the WebView is unloaded. */
export function useWindowState<T>(key: string, initial: T | (() => T), serialize?: (value: T) => unknown): [T, Dispatch<SetStateAction<T>>] {
  const [value, setValue] = useState<T>(() => hasWindowState(key) ? restored[key] as T : typeof initial === "function" ? (initial as () => T)() : initial);
  const current = useRef(value);
  current.current = value;
  const encode = useRef(serialize);
  encode.current = serialize;
  useLayoutEffect(() => {
    delete restored[key];
    providers.set(key, () => encode.current ? encode.current(current.current) : current.current);
    return () => { providers.delete(key); };
  }, [key]);
  return [value, setValue];
}

/** Keep a hidden WebView alive until work or an unsaved form has finished. */
export function usePreventUnload(active: boolean) {
  const block = useRef(active);
  block.current = active;
  useLayoutEffect(() => {
    blockers.add(block);
    return () => { blockers.delete(block); retrySuspend(); };
  }, []);
  useEffect(retrySuspend, [active]);
}

/** Pending writes must finish even after their form unmounts. */
export function preventUnloadUntil<T>(work: Promise<T>): Promise<T> {
  const block = { current: true };
  blockers.add(block);
  return work.finally(() => { blockers.delete(block); retrySuspend(); });
}

function retrySuspend() {
  clearTimeout(scheduled);
  scheduled = setTimeout(() => { void suspend(); }, 0);
}
async function suspend() {
  if (pending === null || saving || [...blockers].some((block) => block.current)) return;
  const ticket = pending;
  saving = true;
  try {
    await invoke("desktop_suspend", { ticket, snapshot: JSON.stringify(captureWindowState()) });
    if (pending === ticket) pending = null;
  } catch (error) {
    if (pending === ticket) {
      pending = null;
      await invoke("desktop_cancel_suspend", { ticket }).catch(() => undefined);
    }
    reportError(error);
  } finally { saving = false; if (pending !== null && pending !== ticket) retrySuspend(); }
}

export function useWindowLifecycle(onError: (error: unknown) => void) {
  const report = useRef(onError);
  report.current = onError;
  useEffect(() => {
    let mounted = true;
    reportError = (error) => report.current(error);
    const stops = [
      listen<number>("desktop-suspend", ({ payload }) => { pending = payload; retrySuspend(); }),
      listen("desktop-resume", () => { pending = null; }),
      // The backend locks from the tray, on sleep or screen lock, and waits for this first.
      listen("desktop-flush", () => { void flushShellSessions().finally(() => invoke("desktop_flushed")).catch(() => undefined); }),
    ];
    void Promise.all(stops).then(async () => {
      const ticket = await invoke<number | null>("desktop_ready");
      if (mounted && ticket !== null) { pending = ticket; retrySuspend(); }
    }).catch(() => undefined);
    return () => {
      mounted = false;
      pending = null;
      reportError = () => undefined;
      clearTimeout(scheduled);
      for (const stop of stops) void stop.then((unlisten) => unlisten());
    };
  }, []);
}

/** The theme preference every window starts from. */
export function storedTheme(): Theme {
  try {
    const stored = localStorage.getItem("odx.theme");
    if (stored === "light" || stored === "dark" || stored === "system") return stored;
  } catch { /* Preferences are optional when storage is unavailable. */ }
  return "system";
}
