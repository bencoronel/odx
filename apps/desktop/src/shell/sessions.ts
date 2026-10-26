import type { AgentRequest, Environment, HistoryEntry } from "../ipc";
import type { AppError } from "../appError";
import type { Table } from "../table";
import type { Command, OdooCall } from "./commands";
import type { Field } from "./complete";

// A Terminal session's data: output entries, tabs, and what is saved in the vault. Rendering lives in ShellPage.

/** Output entries kept per session. */
export const MAX_LINES = 400;
export type ConnectionErrorPresentation = { name: string; reason: "credential" | "offline"; detail?: AppError };
export type Entry =
  /** `command`: a typed command that can be re-run; `folded` hides its output. `transient`: shown but never saved (restore notes). */
  | { id: number; kind: "input"; prompt: string; text: string; command?: boolean; folded?: boolean; transient?: boolean;
    /** The MCP agent that ran it through terminal_run. */
    agent?: string }
  | { id: number; kind: "text" | "info" | "error" | "warn" | "ok"; text: string; translationKey?: "shell.initial" | "shell.help"; appError?: AppError; connectionError?: ConnectionErrorPresentation; transient?: boolean }
  | { id: number; kind: "table"; table: Table; expanded: boolean; footer?: string };
/** Distributive Omit for union variants. */
export type NewEntry = Entry extends unknown ? (Entry extends infer E ? (E extends Entry ? Omit<E, "id"> : never) : never) : never;
/** A write waiting for the user's confirmation (y/n) in the Terminal. */
export type Confirmation = {
  call: OdooCall; display: Extract<Command, { kind: "call" }>["display"]; filter?: string; page?: { limit: number; offset: number };
  /** Pin the environment at confirmation time. */
  env?: { id: string; name: string };
  /** Client id of the agent that asked; only its run can answer, so it is never saved with the window. */
  agent?: string;
};
export type Session = {
  entries: Entry[];
  env?: { id: string; name: string };
  context: Record<string, unknown>;
  expanded: boolean;
  /** Show timing and byte counts for each call (\timing). */
  timing: boolean;
  last?: Confirmation;
  /** Vault-encrypted history; `null` until loaded. */
  history: HistoryEntry[] | null;
  models: Record<string, string[]>;
  /** Autocomplete fields keyed by `environment:model`. */
  fields: Record<string, Field[]>;
  /** Record ids of the latest result table and their model, for `$_`. */
  previous?: { model?: string; ids: number[] };
  /** Ids saved with \\set, for `$name`. */
  vars?: Record<string, { model?: string; ids: number[] }>;
};

let nextId = 1;
/** Ids for new output entries, unique across tabs. */
export const nextEntryId = () => nextId++;
/** The id the next entry gets: where a pasted batch or an agent's run starts. */
export const peekEntryId = () => nextId;
/** Restored entries keep their ids; later ones must not reuse them. */
export function seedEntryIds(entries: { id: number }[]) {
  for (const entry of entries) nextId = Math.max(nextId, entry.id + 1);
}
export const fresh = (): Session => ({
  entries: [{ id: nextEntryId(), kind: "info", text: "", translationKey: "shell.initial" }],
  context: {},
  expanded: false,
  timing: true,
  history: null,
  models: {},
  fields: {},
});
/** Each tab is bound to one environment for its whole life: `$_`, `\set` ids and pages never cross databases.
 * `reconnect`: check and connect when the tab is first shown. `runs`: commands waiting their turn, from another page
 * or an MCP agent (never saved). `unseen`: an agent ran something here, or a command finished, while another tab was in front. `pinned` tabs come first
 * and are left alone by Close others, Close to the left and Close to the right. */
export type AgentRun = Extract<AgentRequest, { kind: "run" }>;
export type QueuedRun = { command: string; agent?: AgentRun };
export type ShellTab = { id: string; name: string; defaultName: boolean; envId: string; envName: string; session: Session; busy: boolean; reconnect?: boolean; runs?: QueuedRun[]; unseen?: "agent" | "done"; pinned?: boolean };

/** Saved in the vault (encrypted) so tabs and output survive restarts. Pending confirmations are never saved.
 * `aliases` are shared by every tab. */
export type SavedSessions = { version: 1; activeId: string; aliases?: Record<string, string>; tabs: { id: string; name: string; defaultName: boolean; pinned?: boolean; envId?: string; envName?: string; session: Pick<Session, "entries" | "env" | "context" | "expanded" | "timing" | "last" | "previous" | "vars"> }[] };
const SAVE_LIMIT = 7 * 1024 * 1024;
const encoder = new TextEncoder();
const bytes = (value: unknown) => encoder.encode(JSON.stringify(value)).length;
export function snapshot(tabs: ShellTab[], activeId: string, aliases: Record<string, string>): string {
  const saved: SavedSessions = { version: 1, activeId, aliases, tabs: tabs.map((tab) => ({
    id: tab.id, name: tab.name, defaultName: tab.defaultName, pinned: tab.pinned, envId: tab.envId, envName: tab.envName,
    session: { entries: tab.session.entries.filter((entry) => !("transient" in entry && entry.transient)), env: tab.session.env, context: tab.session.context, expanded: tab.session.expanded, timing: tab.session.timing, last: tab.session.last, previous: tab.session.previous, vars: tab.session.vars },
  })) };
  // Over the limit, drop the oldest output of the largest tab until it fits. The vault limits UTF-8 bytes, not characters.
  // Each tab is encoded once; trimming subtracts only the dropped entries, never more than they take, so the estimate stays an upper bound.
  const json = JSON.stringify(saved);
  if (encoder.encode(json).length <= SAVE_LIMIT) return json;
  const overhead = bytes({ ...saved, tabs: [] }) + saved.tabs.length;
  const sizes = saved.tabs.map(bytes);
  do {
    const i = sizes.indexOf(Math.max(...sizes));
    const entries = saved.tabs[i].session.entries;
    if (!entries.length) break;
    const cut = Math.max(1, Math.ceil(entries.length / 10));
    sizes[i] -= bytes(entries.slice(0, cut)) - 2;
    saved.tabs[i].session.entries = entries.slice(cut);
  } while (overhead + sizes.reduce((a, b) => a + b, 0) > SAVE_LIMIT);
  return JSON.stringify(saved);
}
/** An untouched tab: default name, not pinned and only informational lines. */
export const pristine = (tab: Pick<ShellTab, "defaultName" | "pinned"> & { session?: Partial<Pick<Session, "entries">> }) =>
  tab.defaultName && !tab.pinned && (tab.session?.entries ?? []).every((e) => e.kind === "info" || e.kind === "ok");
export function restore(data: string, note: string): { tabs: ShellTab[]; activeId: string } | null {
  try {
    const saved = JSON.parse(data) as SavedSessions;
    if (saved?.version !== 1 || !Array.isArray(saved.tabs)) return null;
    // Untouched tabs are not a session worth announcing; tabs saved before sessions had a fixed environment and never
    // connected have nothing to resume.
    const used = saved.tabs.filter((tab) => !pristine(tab) && (tab.envId ?? tab.session?.env?.id));
    if (!used.length) return null;
    const tabs = used.map((tab): ShellTab => {
      const entries = Array.isArray(tab.session?.entries) ? tab.session.entries : [];
      seedEntryIds(entries);
      return {
        id: tab.id, name: tab.name, defaultName: tab.defaultName, pinned: tab.pinned, busy: false, reconnect: true,
        envId: (tab.envId ?? tab.session.env?.id)!, envName: tab.envName ?? tab.session.env?.name ?? "",
        session: { ...fresh(), ...tab.session, entries: [...entries, { id: nextEntryId(), kind: "info" as const, text: note, transient: true }].slice(-MAX_LINES) },
      };
    });
    return { tabs, activeId: tabs.some((tab) => tab.id === saved.activeId) ? saved.activeId : tabs[0].id };
  } catch { return null; }
}
/** Aliases survive even when every saved tab is untouched and the tabs start fresh. */
export function savedAliases(data: string): Record<string, string> {
  try {
    const aliases = (JSON.parse(data) as SavedSessions)?.aliases;
    return aliases && typeof aliases === "object" ? Object.fromEntries(Object.entries(aliases).filter(([, command]) => typeof command === "string")) : {};
  } catch { return {}; }
}
export const newTab = (env: Pick<Environment, "id" | "name">): ShellTab =>
  ({ id: crypto.randomUUID(), name: "", defaultName: true, envId: env.id, envName: env.name, session: fresh(), busy: false, reconnect: true });
export const tabLabel = (tab: ShellTab) => tab.defaultName ? tab.envName : tab.name;
