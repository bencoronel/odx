import { createContext, memo, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { currentShellFlush, registerShellFlush } from "./shellFlush";
import { ipc, type AgentRequest, type Environment, type HistoryEntry, type ShellCall } from "./ipc";
import { highlight } from "./shell/highlight";
import { CommandError, META, VERBS, expandAlias, needsOdoo, parse, searchCall, substitute, unfinished, type Command, type OdooCall } from "./shell/commands";
import { completions, type Completion, type Field } from "./shell/complete";
import { modeIcons, modeLabels, modeShortLabels, modeTone } from "./ModeSelect";
import { CallDetails, ValuesTable, methodTone, recordIds, writtenValues } from "./ApprovalDetails";
import { EnvIcon } from "./EnvIcon";
import { hostOf } from "./hosts";
import { Icon } from "./Icon";
import CopyButton from "./CopyButton";
import { useTranslation } from "react-i18next";
import { formatDateTime, formatNumber } from "./i18n/format";
import { i18n } from "./i18n";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { hasWindowState, useWindowState, usePreventUnload } from "./windowState";
import { placeInWindow, useDismiss } from "./ContextMenu";

const MAX_LINES = 400;
/** Commands kept out of the history, with their aliases (\s for \history). */
const UNRECALLED = new Set(["\\?", "help", "\\history", "\\s", "\\clear"]);
/** Longer logins (often e-mail addresses) are cut with an ellipsis in the prompt. */
const PROMPT_LOGIN = 20;
const FONT_KEY = "odx.shell.fontSize";
const FONT_RANGE = [10, 24] as const;
const DEFAULT_FONT = 12;
function storedFont(): number {
  try { const n = Number(localStorage.getItem(FONT_KEY)); return n >= FONT_RANGE[0] && n <= FONT_RANGE[1] ? n : DEFAULT_FONT; } catch { return DEFAULT_FONT; }
}
const GROUP_FIELDS = ["all_group_ids", "groups_id", "group_ids"];

/** `code`: column indexes containing commands to highlight. `model`: source of the rows, for clickable ids and relations. */
/** `call`: the single Odoo call behind the rows, when there is one, so it can be copied as code. */
export type Table = { columns: string[]; rows: unknown[][]; code?: number[]; model?: string; call?: OdooCall };
type ConnectionErrorPresentation = { name: string; reason: "credential" | "offline"; detail?: AppError };
type Entry =
  /** `command`: a typed command that can be re-run; `folded` hides its output. `transient`: shown but never saved (restore notes). */
  | { id: number; kind: "input"; prompt: string; text: string; command?: boolean; folded?: boolean; transient?: boolean;
    /** The MCP agent that ran it through terminal_run. */
    agent?: string }
  | { id: number; kind: "text" | "info" | "error" | "warn" | "ok"; text: string; translationKey?: "shell.initial" | "shell.help"; appError?: AppError; connectionError?: ConnectionErrorPresentation; transient?: boolean }
  | { id: number; kind: "table"; table: Table; expanded: boolean; footer?: string };
/** Distributive Omit for union variants. */
type NewEntry = Entry extends unknown ? (Entry extends infer E ? (E extends Entry ? Omit<E, "id"> : never) : never) : never;
type Pending = {
  call: OdooCall; display: Extract<Command, { kind: "call" }>["display"]; filter?: string; page?: { limit: number; offset: number };
  /** Pin the environment at confirmation time. */
  env?: { id: string; name: string };
};
type Session = {
  entries: Entry[];
  env?: { id: string; name: string };
  context: Record<string, unknown>;
  expanded: boolean;
  /** Show timing and byte counts for each call (\timing). */
  timing: boolean;
  last?: Pending;
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
type Menu = Completion & { index: number };

let nextId = 1;
const fresh = (): Session => ({
  entries: [{ id: nextId++, kind: "info", text: "", translationKey: "shell.initial" }],
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
type AgentRun = Extract<AgentRequest, { kind: "run" }>;
type QueuedRun = { command: string; agent?: AgentRun };
type ShellTab = { id: string; name: string; defaultName: boolean; envId: string; envName: string; session: Session; busy: boolean; reconnect?: boolean; runs?: QueuedRun[]; unseen?: "agent" | "done"; pinned?: boolean };

/** Saved in the vault (encrypted) so tabs and output survive restarts. Pending confirmations are never saved.
 * `aliases` are shared by every tab. */
type SavedSessions = { version: 1; activeId: string; aliases?: Record<string, string>; tabs: { id: string; name: string; defaultName: boolean; pinned?: boolean; envId?: string; envName?: string; session: Pick<Session, "entries" | "env" | "context" | "expanded" | "timing" | "last" | "previous" | "vars"> }[] };
const SAVE_LIMIT = 7 * 1024 * 1024;
const encoder = new TextEncoder();
const bytes = (value: unknown) => encoder.encode(JSON.stringify(value)).length;
function snapshot(tabs: ShellTab[], activeId: string, aliases: Record<string, string>): string {
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
const pristine = (tab: Pick<ShellTab, "defaultName" | "pinned"> & { session?: Partial<Pick<Session, "entries">> }) =>
  tab.defaultName && !tab.pinned && (tab.session?.entries ?? []).every((e) => e.kind === "info" || e.kind === "ok");
function restore(data: string, note: string): { tabs: ShellTab[]; activeId: string } | null {
  try {
    const saved = JSON.parse(data) as SavedSessions;
    if (saved?.version !== 1 || !Array.isArray(saved.tabs)) return null;
    // Untouched tabs are not a session worth announcing; tabs saved before sessions had a fixed environment and never
    // connected have nothing to resume.
    const used = saved.tabs.filter((tab) => !pristine(tab) && (tab.envId ?? tab.session?.env?.id));
    if (!used.length) return null;
    const tabs = used.map((tab): ShellTab => {
      const entries = Array.isArray(tab.session?.entries) ? tab.session.entries : [];
      for (const entry of entries) nextId = Math.max(nextId, entry.id + 1);
      return {
        id: tab.id, name: tab.name, defaultName: tab.defaultName, pinned: tab.pinned, busy: false, reconnect: true,
        envId: (tab.envId ?? tab.session.env?.id)!, envName: tab.envName ?? tab.session.env?.name ?? "",
        session: { ...fresh(), ...tab.session, entries: [...entries, { id: nextId++, kind: "info" as const, text: note, transient: true }].slice(-MAX_LINES) },
      };
    });
    return { tabs, activeId: tabs.some((tab) => tab.id === saved.activeId) ? saved.activeId : tabs[0].id };
  } catch { return null; }
}
/** Aliases survive even when every saved tab is untouched and the tabs start fresh. */
function savedAliases(data: string): Record<string, string> {
  try {
    const aliases = (JSON.parse(data) as SavedSessions)?.aliases;
    return aliases && typeof aliases === "object" ? Object.fromEntries(Object.entries(aliases).filter(([, command]) => typeof command === "string")) : {};
  } catch { return {}; }
}
const newTab = (env: Pick<Environment, "id" | "name">): ShellTab =>
  ({ id: crypto.randomUUID(), name: "", defaultName: true, envId: env.id, envName: env.name, session: fresh(), busy: false, reconnect: true });
const tabLabel = (tab: ShellTab) => tab.defaultName ? tab.envName : tab.name;

/** JSON as a Python literal: the same structure with None, True and False. */
function pythonLiteral(value: unknown): string {
  if (value === null || value === undefined) return "None";
  if (typeof value === "boolean") return value ? "True" : "False";
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(pythonLiteral).join(", ")}]`;
  return `{${Object.entries(value as Record<string, unknown>).map(([k, v]) => `${JSON.stringify(k)}: ${pythonLiteral(v)}`).join(", ")}}`;
}

/** An Odoo call as a standalone XML-RPC script; the API key stays a placeholder, Odx never copies secrets. */
function pythonCall(env: Pick<Environment, "url" | "database" | "login">, call: OdooCall, context: Record<string, unknown>): string {
  const kwargs = Object.keys(context).length ? { ...call.kwargs, context: { ...context, ...(call.kwargs.context as object | undefined) } } : call.kwargs;
  return [
    "import xmlrpc.client",
    "",
    `url, db, login = ${JSON.stringify(env.url.replace(/\/+$/, ""))}, ${JSON.stringify(env.database)}, ${JSON.stringify(env.login)}`,
    'key = "<API key>"',
    'uid = xmlrpc.client.ServerProxy(f"{url}/xmlrpc/2/common").authenticate(db, login, key, {})',
    'models = xmlrpc.client.ServerProxy(f"{url}/xmlrpc/2/object")',
    `result = models.execute_kw(db, uid, key, ${JSON.stringify(call.model)}, ${JSON.stringify(call.method)}, ${pythonLiteral(call.args)}, ${pythonLiteral(kwargs)})`,
    "print(result)",
    "",
  ].join("\n");
}

export function cellText(v: unknown): string {
  if (v === null || v === undefined) return "null";
  if (typeof v === "string") return v;
  if (Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string") return `${v[1]} (${v[0]})`;
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}
const isM2o = (v: unknown): v is [number, string] => Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string";
const isIds = (v: unknown): v is number[] => Array.isArray(v) && v.length > 0 && v.every((n) => Number.isInteger(n) && n > 0);
type Link = { model: string; column: string; value: unknown; row: Record<string, unknown> };
/** Cell clicks: follow a record or relation, or open a long value. */
/** What a table can do. In a session all of it; in a table window only reading: the optional ones need the session. */
export const CellActions = createContext<{ follow?: (link: Link, inOdoo?: boolean) => void; inspect?: (title: string, value: unknown) => void; exportTable?: (table: Table, format: "csv" | "json") => void;
  /** A model's fields for header tooltips, when loaded. */
  fields: (model: string) => Field[] | undefined;
  /** The call as a Python script against this tab's environment, without its API key. */
  callCode?: (call: OdooCall) => string | null;
  /** Put a command in the input, whole, to edit or run it. */
  insert?: (command: string) => void;
  /** Open the table in a window of its own. */
  popOut?: (table: Table, footer?: string) => void } | null>(null);

/** `pos` ("row:column") and `active` support keyboard navigation; `pin` keeps the column visible when scrolling sideways. */
/** How a value reads once its field is known: selection labels, datetimes in local time, booleans as marks.
 * `tip` keeps the raw value; copying and exporting always use the raw value. */
function typedValue(value: unknown, field?: Field): { text: string; tip?: string; className?: string } | null {
  if (!field) return null;
  if (field.type === "boolean" && typeof value === "boolean") return { text: value ? "✓" : "✗", tip: String(value), className: value ? "bool yes" : "bool no" };
  if (field.type === "selection" && typeof value === "string") {
    const label = field.selection?.find(([key]) => key === value)?.[1];
    return label ? { text: label, tip: value } : null;
  }
  // Odoo sends datetimes in UTC without a zone.
  if (field.type === "datetime" && typeof value === "string" && /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(value)) {
    const at = new Date(`${value.replace(" ", "T")}Z`);
    return Number.isNaN(at.getTime()) ? null : { text: formatDateTime(at, { dateStyle: "short", timeStyle: "medium" }), tip: `${value} UTC` };
  }
  return null;
}

/** `edges`: the selection's outline on this cell (inset shadows on the sides that border unselected cells). */
function Cell({ value, column, model, row, pos, active, selected, edges, pin }: { value: unknown; column: string; model?: string; row: Record<string, unknown>; pos?: string; active?: boolean; selected?: boolean; edges?: string; pin?: boolean }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const typed = typedValue(value, model ? actions?.fields(model)?.find((f) => f.name === column) : undefined);
  const text = typed?.text ?? cellText(value);
  const dim = !typed && (value === false || value === null || value === undefined || (Array.isArray(value) && !value.length));
  const short = text.length > 80 ? `${text.slice(0, 80)}…` : text;
  const className = `${dim ? "dim" : ""} ${typeof value === "number" ? "num" : ""} ${typed?.className ?? ""} ${pin ? "pin" : ""}`;
  const cell = { "data-cell": pos, "data-active": active || undefined, "data-selected": selected || undefined, style: edges ? { boxShadow: edges } : undefined };
  // ir.model.data rows point at another model through res_id.
  const follow = actions?.follow;
  const linked = model && follow && (
    (column === "id" && typeof value === "number")
    || (model === "ir.model.data" && column === "res_id" && typeof value === "number" && typeof row.model === "string")
    || (column !== "id" && (isM2o(value) || isIds(value))));
  // A click selects the cell (see TableBlock); a double click acts on it.
  if (linked) return <td className={className} {...cell}><button type="button" className="term-link" tabIndex={-1} data-tip={t("shell.openRecord")}
    onDoubleClick={(event) => follow({ model, column, value, row }, event.ctrlKey || event.metaKey)}>{short}</button></td>;
  const inspect = actions?.inspect;
  if (inspect && (short !== text || (value !== null && typeof value === "object"))) return <td className={className} {...cell}>
    <button type="button" className="term-more" tabIndex={-1} data-tip={t("shell.inspectValue")}
      onDoubleClick={() => inspect(`${model ? `${model} ` : ""}${row.id !== undefined ? `${cellText(row.id)} · ` : ""}${column}`, value)}>{short}</button>
  </td>;
  return <td className={className} {...cell} data-tip={typed?.tip ?? (short !== text ? text : undefined)}>{short}</td>;
}
export function rowsTable(rows: Record<string, unknown>[]): Table {
  const columns: string[] = [];
  for (const row of rows) for (const key of Object.keys(row)) if (!columns.includes(key)) columns.push(key);
  return { columns, rows: rows.map((row) => columns.map((c) => row[c])) };
}
const size = (bytes: number) => bytes < 1024 ? `${formatNumber(bytes)} B` : `${formatNumber(bytes / 1024, { maximumFractionDigits: 1 })} KB`;

export default function ShellPage({ envs, connectTo, command, onConnectHandled, visible = true, agentRequests = [], onAgentHandled }: {
  /** `command` runs once connected to `connectTo`. */
  envs: Environment[]; connectTo?: string | null; command?: string | null; onConnectHandled?: () => void; visible?: boolean;
  /** Requests from MCP agents (terminal_run, show); each is answered through ipc.agentReply and then reported handled. */
  agentRequests?: AgentRequest[]; onAgentHandled?: (id: number) => void;
}) {
  const { t } = useTranslation();
  const [tabs, setTabs] = useWindowState<ShellTab[]>("shell.tabs", () => [], (tabs) => tabs.map((tab) => ({
    ...tab, busy: false, runs: undefined, session: { ...tab.session, history: null, models: {}, fields: {} },
  })));
  // Restored entries keep their ids; new entries must not reuse them.
  const seeded = useRef(false);
  if (!seeded.current) {
    seeded.current = true;
    for (const tab of tabs) for (const entry of tab.session.entries) nextId = Math.max(nextId, entry.id + 1);
  }
  const [activeId, setActiveId] = useWindowState("shell.activeId", tabs[0]?.id ?? "");
  const [aliases, setAliases] = useWindowState<Record<string, string>>("shell.aliases", {});

  // After a restart (no in-memory window state), load the tabs saved in the vault. Saving waits
  // for this load so an empty start never overwrites the saved sessions.
  const [loaded, setLoaded] = useState(() => hasWindowState("shell.tabs"));
  useEffect(() => {
    if (loaded) return;
    let cancelled = false;
    void Promise.resolve(ipc.shellSessions()).then((data) => {
      if (cancelled || !data) return;
      setAliases((current) => ({ ...savedAliases(data), ...current }));
      const restored = restore(data, t("shell.sessionRestored", { date: formatDateTime(new Date(), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }) }));
      if (!restored) return;
      // Keep anything typed while the vault was being read.
      setTabs((current) => [...restored.tabs, ...current.filter((tab) => !pristine(tab))]);
      setActiveId(restored.activeId);
    }).catch(() => undefined).finally(() => { if (!cancelled) setLoaded(true); });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const state = useRef({ tabs, activeId, aliases });
  state.current = { tabs, activeId, aliases };
  const saveTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const saving = useRef<Promise<void>>(Promise.resolve());
  const [saveFailed, setSaveFailed] = useState(false);
  const save = () => {
    clearTimeout(saveTimer.current);
    saveTimer.current = undefined;
    return saving.current = Promise.resolve().then(() => ipc.saveShellSessions(snapshot(state.current.tabs, state.current.activeId, state.current.aliases)))
      .then(() => setSaveFailed(false), () => setSaveFailed(true));
  };
  const persist = useCallback(() => {
    if (!loaded) return;
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(save, 800);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded]);
  // Switching tabs alone is not worth a save of every tab's output; the next save records it.
  useEffect(persist, [tabs, aliases, persist]);
  // Flush on quit, lock or when the window goes away.
  useEffect(() => {
    const flush = () => saveTimer.current !== undefined ? save() : saving.current;
    const onHide = () => void flush();
    registerShellFlush(flush);
    window.addEventListener("pagehide", onHide);
    return () => { window.removeEventListener("pagehide", onHide); if (currentShellFlush() === flush) registerShellFlush(undefined); void flush(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [renaming, setRenaming] = useState<{ id: string; value: string; width: number } | null>(null);
  const [fontSize, setFontSizeState] = useState(storedFont);
  const setFontSize = useCallback((size: number) => {
    const next = Math.min(FONT_RANGE[1], Math.max(FONT_RANGE[0], size));
    setFontSizeState(next);
    try { localStorage.setItem(FONT_KEY, String(next)); } catch { /* per-window only */ }
  }, []);
  usePreventUnload(tabs.some((tab) => tab.busy) || renaming !== null);
  const refreshTabs = useCallback(() => setTabs((current) => [...current]), [setTabs]);
  // Stable callbacks let hidden sessions skip re-rendering when the app updates.
  const connectHandled = useRef(onConnectHandled);
  connectHandled.current = onConnectHandled;
  const add = (env: Pick<Environment, "id" | "name">) => {
    const tab = newTab(env);
    setTabs((current) => [...current, tab]);
    setActiveId(tab.id);
    return tab;
  };
  // "+" and Ctrl+T ask for the environment first, so a new session opens already connected.
  const [picking, setPicking] = useState<{ left: number; top: number } | null>(null);
  const plus = useRef<HTMLButtonElement>(null);
  const closePicker = useCallback(() => { setPicking(null); plus.current?.focus(); }, []);
  /** `from`: the button the menu hangs from; fixed position, since the tab strip scrolls and would clip it. */
  const pick = (from: HTMLElement | null = plus.current) => {
    if (!envs.length) return;
    const rect = from?.getBoundingClientRect();
    setPicking({ left: rect?.left ?? 8, top: (rect?.bottom ?? 40) + 4 });
  };
  const openWith = (env: Environment) => { setPicking(null); add(env); };
  // A requested environment reuses a tab of that environment (the active one first), else opens one. The command
  // queues in that tab until it is connected. Waits for saved tabs, since a restore changes the active tab.
  useEffect(() => {
    if (!connectTo || !loaded) return;
    const env = envs.find((e) => e.name.toLowerCase() === connectTo.toLowerCase());
    connectHandled.current?.();
    if (!env) return;
    const own = tabs.filter((tab) => tab.envId === env.id);
    const tab = own.find((item) => item.id === activeId) ?? own[0] ?? add(env);
    setActiveId(tab.id);
    if (command) { tab.runs = [...tab.runs ?? [], { command }]; refreshTabs(); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectTo, loaded]);
  /** Busy tabs stay open. When the active tab goes, `keep` (the tab whose menu closed the others) takes its place, else its neighbour. */
  const close = (targets: ShellTab[], keep?: ShellTab) => {
    const gone = targets.filter((tab) => !tab.busy);
    if (!gone.length) return;
    const remaining = tabs.filter((item) => !gone.includes(item));
    setTabs(remaining);
    if (gone.some((tab) => tab.id === activeId)) {
      const index = tabs.findIndex((tab) => tab.id === activeId);
      setActiveId(keep && remaining.includes(keep) ? keep.id : remaining[Math.min(index, remaining.length - 1)]?.id ?? "");
    }
    if (renaming && gone.some((tab) => tab.id === renaming.id)) setRenaming(null);
  };
  // Pinning moves the tab to the front; unpinning, to just after the pinned ones.
  const pin = (tab: ShellTab) => {
    tab.pinned = !tab.pinned;
    const rest = tabs.filter((item) => item !== tab);
    const at = tab.pinned ? 0 : rest.filter((item) => item.pinned).length;
    setTabs([...rest.slice(0, at), tab, ...rest.slice(at)]);
  };
  const [tabMenu, setTabMenu] = useState<{ id: string; left: number; top: number } | null>(null);
  // Dragging a tab carries it under the pointer and marks with a line where it will land; the order changes on release.
  // It lands within its group (pinned or not). Mouse events, since native drag is off app-wide.
  const [dragging, setDragging] = useState<{ id: string; dx: number; drop?: { id: string; side: "before" | "after" } } | null>(null);
  const drag = (event: ReactMouseEvent, tab: ShellTab) => {
    if (event.button !== 0) return;
    const startX = event.clientX;
    let started = false;
    let next: ShellTab[] | undefined;
    const move = (e: MouseEvent) => {
      if (!started && Math.abs(e.clientX - startX) < 5) return;
      started = true;
      const current = state.current.tabs;
      const rest = current.filter((item) => item !== tab);
      const pinned = rest.filter((item) => item.pinned).length;
      // The slot among the other tabs: past every middle the pointer has crossed, kept inside the tab's group.
      const middles = rest.map((item) => {
        const rect = document.getElementById(`shell-tab-${item.id}`)?.parentElement?.getBoundingClientRect();
        return rect ? rect.left + rect.width / 2 : 0;
      });
      const crossed = middles.filter((middle) => e.clientX > middle).length;
      const slot = tab.pinned ? Math.min(crossed, pinned) : Math.max(crossed, pinned);
      const drop = slot === current.indexOf(tab) ? undefined
        : slot < rest.length ? { id: rest[slot].id, side: "before" as const } : { id: rest[rest.length - 1].id, side: "after" as const };
      next = drop && [...rest.slice(0, slot), tab, ...rest.slice(slot)];
      setDragging({ id: tab.id, dx: e.clientX - startX, drop });
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      setDragging(null);
      if (next) setTabs(next);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
  // Agent requests: a run goes to its tab's queue (the session runs it); show brings that tab to the front.
  useEffect(() => {
    if (!loaded) return;
    for (const request of agentRequests) {
      onAgentHandled?.(request.id);
      if (request.kind === "show") {
        if (tabs.some((tab) => tab.id === request.terminal)) { setActiveId(request.terminal); void ipc.agentReply(request.id); }
        else void ipc.agentReply(request.id, undefined, `no terminal ${request.terminal}`);
        continue;
      }
      const env = envs.find((e) => e.id === request.envId);
      const own = tabs.filter((tab) => tab.envId === request.envId);
      let tab = request.terminal ? tabs.find((item) => item.id === request.terminal) : own.find((item) => item.id === activeId) ?? own[0];
      if (!tab && !request.terminal && env) {
        // A new session for the agent opens behind the current tab; only show brings it forward.
        tab = newTab(env);
        setTabs((current) => [...current, tab!]);
        if (!tabs.length) setActiveId(tab.id);
      }
      if (!tab) { void ipc.agentReply(request.id, undefined, request.terminal ? `no terminal ${request.terminal}` : "environment unavailable"); continue; }
      tab.runs = [...tab.runs ?? [], { command: request.command, agent: request }];
      if (tab.id !== activeId) tab.unseen = "agent";
    }
    if (agentRequests.length) refreshTabs();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentRequests, loaded]);
  // Terminal tab keys: Ctrl+T new, Ctrl+W close, Ctrl+Tab / Ctrl+PageDown next, with Shift / PageUp previous.
  useEffect(() => {
    if (!visible) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey || document.querySelector('[role="alertdialog"]')) return;
      const key = event.key.toLowerCase();
      const index = tabs.findIndex((tab) => tab.id === activeId);
      if (key === "t" && !event.shiftKey) { event.preventDefault(); pick(plus.current ?? document.querySelector<HTMLElement>(".shell-empty .btn")); }
      else if (key === "w" && !event.shiftKey) { event.preventDefault(); if (tabs[index]) close([tabs[index]]); }
      else if (key === "tab" || key === "pagedown" || key === "pageup") {
        event.preventDefault();
        const step = key === "pageup" || (key === "tab" && event.shiftKey) ? -1 : 1;
        if (tabs.length) setActiveId(tabs[(index + step + tabs.length) % tabs.length].id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, tabs, activeId, renaming]);
  const rename = (tab: ShellTab) => {
    const width = document.getElementById(`shell-tab-${tab.id}`)?.getBoundingClientRect().width;
    if (width === undefined) return;
    setActiveId(tab.id);
    setRenaming({ id: tab.id, value: tabLabel(tab), width });
  };
  const finishRename = (tab: ShellTab) => {
    if (renaming?.id !== tab.id) return;
    const name = renaming.value.trim();
    // Clearing the name, or typing the environment's, goes back to following the environment.
    tab.defaultName = !name || name === tab.envName;
    tab.name = tab.defaultName ? "" : name;
    setRenaming(null);
    refreshTabs();
  };
  return <div className="shell-sessions">
    <div className="shell-tabs" role="tablist" aria-label={t("shell.sessions")} aria-orientation="horizontal">
      {tabs.map((tab, index) => <div className="shell-tab" key={tab.id} role="presentation" data-active={activeId === tab.id} data-dragging={dragging?.id === tab.id || undefined}
        data-drop={dragging?.drop?.id === tab.id ? dragging.drop.side : undefined} style={dragging?.id === tab.id ? { transform: `translateX(${dragging.dx}px)` } : undefined}
        onMouseDown={(event) => { if (event.button === 1) event.preventDefault(); }}
        onAuxClick={(event) => { if (event.button === 1) { event.preventDefault(); close([tab]); } }}>
        {renaming?.id === tab.id ? <div className="shell-tab-editor" style={{ width: renaming.width }}>
          <Icon name={tab.busy ? "spinner" : "terminal"} className={tab.busy ? "spin" : undefined} />
          <input className="shell-tab-name" aria-label={t("shell.sessionName")} autoFocus maxLength={80} spellCheck={false} autoCorrect="off" autoComplete="off"
          value={renaming.value} onFocus={(event) => event.currentTarget.select()}
          onChange={(event) => setRenaming({ ...renaming, value: event.target.value })}
          onBlur={() => finishRename(tab)} onKeyDown={(event) => {
            if (event.key === "Enter") { event.preventDefault(); finishRename(tab); }
            if (event.key === "Escape") { event.preventDefault(); setRenaming(null); }
          }} />
        </div> : <button type="button" role="tab" id={`shell-tab-${tab.id}`} aria-controls={`shell-panel-${tab.id}`}
          aria-selected={activeId === tab.id} tabIndex={activeId === tab.id ? 0 : -1}
          onMouseDown={(event) => drag(event, tab)} onClick={() => setActiveId(tab.id)} onDoubleClick={(event) => { event.preventDefault(); rename(tab); }}
          onContextMenu={(event) => {
            event.preventDefault();
            event.stopPropagation();
            // The menu key reports no pointer position: hang the menu from the tab.
            const rect = event.currentTarget.getBoundingClientRect();
            setTabMenu({ id: tab.id, ...event.clientX || event.clientY ? { left: event.clientX, top: event.clientY } : { left: rect.left, top: rect.bottom + 4 } });
          }} onKeyDown={(event) => {
            if (event.key === "F2") { event.preventDefault(); rename(tab); return; }
            let target: ShellTab | undefined;
            if (event.key === "ArrowRight") target = tabs[(index + 1) % tabs.length];
            if (event.key === "ArrowLeft") target = tabs[(index + tabs.length - 1) % tabs.length];
            if (event.key === "Home") target = tabs[0];
            if (event.key === "End") target = tabs[tabs.length - 1];
            if (target) { event.preventDefault(); setActiveId(target.id); document.getElementById(`shell-tab-${target.id}`)?.focus(); }
          }}>
          <Icon name={tab.busy ? "spinner" : "terminal"} className={tab.busy ? "spin" : undefined} />
          <span>{tabLabel(tab)}</span>{!tab.defaultName && <>{" "}<span className="shell-tab-env">{tab.envName}</span></>}
          {tab.unseen && <span className="shell-tab-unseen" role="status" aria-label={t(tab.unseen === "agent" ? "shell.agentActivity" : "shell.finishedAway")} data-tip={tab.unseen === "agent" ? t("shell.agentActivity") : undefined} />}
        </button>}
        {tab.pinned ? <button type="button" className="icon-btn sm shell-tab-pin" aria-label={t("shell.unpinSession")} data-tip={t("shell.unpinSession")}
          onClick={() => pin(tab)}><Icon name="pin" /></button>
        : <button type="button" className="icon-btn sm shell-tab-close" aria-label={t("shell.closeSession", { name: tabLabel(tab) })}
          disabled={tab.busy} data-tip={tab.busy ? t("shell.callInProgress") : `${t("shell.closeSessionLabel")} · Ctrl+W`} onClick={() => close([tab])}><Icon name="x" /></button>}
      </div>)}
      <button type="button" ref={plus} className="icon-btn sm" aria-label={t("shell.newSession")} data-tip={picking ? undefined : envs.length ? `${t("shell.newSessionLabel")} · Ctrl+T` : t("shell.noEnvironments")} aria-keyshortcuts="Control+T"
        disabled={!envs.length} aria-haspopup="menu" aria-expanded={!!picking} onClick={() => picking ? setPicking(null) : pick()}><Icon name="plus" /></button>
    </div>
    {loaded && !tabs.length && <div className="empty shell-empty">
      <span className="empty-icon"><Icon name="terminal" /></span>
      <h2>{t("shell.noSessions")}</h2>
      {envs.length ? <button type="button" className="btn primary" aria-haspopup="menu" aria-expanded={!!picking} onClick={(event) => picking ? setPicking(null) : pick(event.currentTarget)}><Icon name="plus" />{t("shell.newSession")}</button>
        : <p>{t("shell.noEnvironments")}</p>}
    </div>}
    {picking && <EnvironmentMenu envs={envs} at={picking} choose={openWith} close={closePicker} />}
    {tabMenu && (() => {
      const tab = tabs.find((item) => item.id === tabMenu.id);
      if (!tab) return null;
      const index = tabs.indexOf(tab);
      const closable = (list: ShellTab[]) => list.filter((item) => !item.pinned && !item.busy);
      const others = closable(tabs.filter((item) => item !== tab));
      const left = closable(tabs.slice(0, index));
      const right = closable(tabs.slice(index + 1));
      const done = () => { setTabMenu(null); document.getElementById(`shell-tab-${tab.id}`)?.focus(); };
      return <TabMenu at={tabMenu} label={tabLabel(tab)} close={done} items={[
        { label: t("shell.renameSession"), keys: "F2", run: () => rename(tab) },
        { label: t(tab.pinned ? "shell.unpinSession" : "shell.pinSession"), run: () => pin(tab) },
        null,
        { label: t("shell.closeTab"), keys: "Ctrl+W", disabled: tab.busy, run: () => close([tab]) },
        { label: t("shell.closeOthers"), disabled: !others.length, run: () => close(others, tab) },
        { label: t("shell.closeLeft"), disabled: !left.length, run: () => close(left, tab) },
        { label: t("shell.closeRight"), disabled: !right.length, run: () => close(right, tab) },
      ]} />;
    })()}
    {tabs.map((tab) => <div className="shell-panel" key={tab.id} role="tabpanel" id={`shell-panel-${tab.id}`}
      aria-labelledby={`shell-tab-${tab.id}`} hidden={activeId !== tab.id}>
      <ShellSession tab={tab} envs={envs} active={visible && activeId === tab.id} onChange={refreshTabs} onPersist={persist} saveFailed={saveFailed} fontSize={fontSize} setFontSize={setFontSize}
        aliases={aliases} setAliases={setAliases}
        queued={tab.runs?.length ?? 0} />
    </div>)}
  </div>;
}

// `tab` keeps its identity while the session mutates it; the session re-renders from its own state.
const ShellSession = memo(function ShellSession({ tab, envs, active, onChange, onPersist, saveFailed, fontSize, setFontSize, aliases, setAliases, queued }: {
  tab: ShellTab; envs: Environment[]; active: boolean; onChange: () => void; onPersist: () => void; saveFailed: boolean;
  /** How many commands wait in `tab.runs`; a change wakes the session to run them. */
  queued: number;
  fontSize: number; setFontSize: (size: number) => void;
  aliases: Record<string, string>; setAliases: (update: (aliases: Record<string, string>) => Record<string, string>) => void;
}) {
  const { t } = useTranslation();
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [session, setSession] = useState<Session>(tab.session);
  const [input, setInput] = useWindowState(`shell.${tab.id}.input`, "");
  const [cursor, setCursor] = useState<number | null>(null);
  const [busy, setBusyState] = useState(false);
  // A command that ends while the tab is not in front (another tab or page) leaves a mark on the tab until it is shown.
  // Connecting does not: it is not something the user waits to read.
  const shown = useRef(active);
  shown.current = active;
  const setBusy = (value: boolean) => {
    if (!mounted.current) return;
    if (!value && tab.busy && !shown.current && !quiet.current) tab.unseen ??= "done";
    tab.busy = value; setBusyState(value); onChange();
  };
  useEffect(() => { if (active && tab.unseen) { tab.unseen = undefined; onChange(); } }, [active, tab, onChange]);
  const [started, setStarted] = useState(0);
  // Ctrl+C bumps the ticket: the call keeps running in Odoo, but its stale result is ignored.
  const ticket = useRef(0);
  const writing = useRef(false);
  const quiet = useRef(false);
  const begin = (write = false, connect = false) => {
    const id = ++ticket.current;
    writing.current = write;
    quiet.current = connect;
    setStarted(Date.now());
    setBusy(true);
    return () => ticket.current === id;
  };
  const finish = (current: () => boolean) => { if (current()) setBusy(false); };
  const discard = () => {
    ticket.current++;
    setBusy(false);
    print({ kind: "input", prompt: prompt(), text: "^C" });
    text("warn", t(writing.current ? "shell.discardedWrite" : "shell.discarded"));
  };
  const [confirming, setConfirming] = useWindowState<Pending | null>(`shell.${tab.id}.confirming`, null);
  // Mirrors for the agent flow, which waits across renders.
  const confirmingNow = useRef(confirming);
  confirmingNow.current = confirming;
  /** The agent a running command belongs to: its calls carry its id, for the core's checks and the audit. */
  const acting = useRef<AgentRun | null>(null);
  const shell = (call: ShellCall) => ipc.shell(acting.current ? { ...call, client_id: acting.current.clientId } : call);
  usePreventUnload(busy);
  const [menu, setMenu] = useState<Menu | null>(null);
  const menuList = useRef<HTMLUListElement>(null);
  const out = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLTextAreaElement>(null);

  useLayoutEffect(() => { if (active) out.current?.scrollTo({ top: out.current.scrollHeight }); }, [session.entries, busy, confirming, active]);
  useEffect(() => {
    if (active && !busy && !document.activeElement?.closest(".shell-tabs")) field.current?.focus();
  }, [busy, confirming, active]);

  const update = (f: (s: Session) => Session) => {
    if (!mounted.current) return;
    const previous = tab.session.env?.name;
    tab.session = f(tab.session);
    setSession(tab.session);
    if (previous !== tab.session.env?.name) onChange();
    onPersist();
  };

  // Connect on mount when opened from Environments.
  useEffect(() => {
    if (tab.session.history !== null) return;
    void ipc.shellHistory().then((history) => update((s) => ({ ...s, history })))
      .catch(() => update((s) => ({ ...s, history: [] })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  /** Ctrl+R as in bash: `query` typed in place of the line, `skip` older matches passed with Ctrl+R, `original` the line before. */
  const [search, setSearch] = useState<{ query: string; skip: number; original: string } | null>(null);
  const [help, setHelp] = useState(false);
  /** Why the last connection attempt failed; formatted when shown, so it follows the language. */
  const [failure, setFailure] = useState<ConnectionErrorPresentation | AppError | null>(null);
  const failureNow = useRef(failure);
  failureNow.current = failure;
  /** What the last successful check reported, for the mode chip's tooltip. */
  const [server, setServer] = useState<{ version?: string; latency: number } | null>(null);
  /** Lines pasted together, each a command, waiting for "run in order" or "paste as text". */
  const [pasted, setPasted] = useState<string[] | null>(null);
  /** Pasted commands running one by one; `mark` is the first entry id of the current step, to spot its errors. */
  const [batch, setBatch] = useState<{ lines: string[]; total: number; mark: number } | null>(null);
  const stepping = useRef(false);
  /** A line starts with a command or an alias; indented or option lines continue a multi-line command instead. */
  const isCommand = (line: string) => { const word = line.split(/\s+/)[0]; return VERBS.includes(word) || META.includes(word) || word in aliases; };
  const onPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    if (confirming || batch || input.trim()) return;
    const lines = event.clipboardData.getData("text").split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    if (lines.length < 2 || !lines.every(isCommand)) return;
    event.preventDefault();
    setPasted(lines);
  };
  const runPasted = () => {
    if (!pasted) return;
    if (!tab.session.env && pasted.some(odooCommand)) { setHint(t(connecting ? "shell.waitConnection" : "shell.needsConnection")); setNudge((n) => n + 1); return; }
    setBatch({ lines: pasted, total: pasted.length, mark: nextId });
    setPasted(null);
  };
  // One step at a time: wait for the previous call and any write confirmation, stop at the first error.
  useEffect(() => {
    if (!batch || busy || confirming || stepping.current) return;
    const failed = tab.session.entries.findIndex((entry) => entry.id >= batch.mark && entry.kind === "error") >= 0;
    const index = batch.total - batch.lines.length;
    if (failed) { setBatch(null); return text("warn", t("shell.batchStopped", { index, total: batch.total })); }
    if (!batch.lines.length) return setBatch(null);
    const [line, ...rest] = batch.lines;
    stepping.current = true;
    setBatch({ ...batch, lines: rest, mark: nextId });
    void submit(line).finally(() => { stepping.current = false; setBatch((current) => current && { ...current }); });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [batch, busy, confirming]);
  // Queued commands, one at a time: after the user's own call, connected, and through any write confirmation.
  const serving = useRef(false);
  useEffect(() => {
    const run = tab.runs?.[0];
    if (!run || serving.current) return;
    serving.current = true;
    void serveRun(run).finally(() => {
      tab.runs = tab.runs?.filter((item) => item !== run);
      serving.current = false;
      onChange();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queued]);
  /** A command from another page replaces a pending confirmation, as the user asked for it; an agent waits its turn. */
  async function serveRun({ command, agent }: QueuedRun) {
    const until = async (done: () => boolean) => { while (mounted.current && !done()) await new Promise((resolve) => setTimeout(resolve, 50)); };
    const idle = () => !tab.busy && !confirmingNow.current;
    const reply = (result?: unknown, error?: string) => { if (agent) void ipc.agentReply(agent.id, result, error).catch(() => undefined); };
    if (!agent && confirmingNow.current) { setConfirming(null); confirmingNow.current = null; text("info", t("shell.cancelled")); }
    await until(idle);
    if (!tab.session.env) { reconnect(); await until(() => !tab.busy); }
    if (!tab.session.env) return reply(undefined, failureNow.current ? connectionProblem(failureNow.current) : t("shell.noConnection"));
    acting.current = agent ?? null;
    const mark = nextId;
    try {
      await submit(command, agent?.clientLabel);
      await until(() => !tab.busy);
      // A write the client may not request is withdrawn before the user is asked.
      if (agent && confirmingNow.current && !agent.allowWrites) {
        setConfirming(null);
        confirmingNow.current = null;
        text("error", t("shell.agentWriteDenied", { name: agent.clientLabel }));
      }
      await until(idle);
    } finally { acting.current = null; }
    const output = tab.session.entries.filter((entry) => entry.id >= mark).map(plainEntry);
    reply({ terminal: tab.id, env: tab.envName, command, output });
  }
  /** Bumped to shake the status chip when a command needs the connection it lacks. */
  const [nudge, setNudge] = useState(0);
  const odooCommand = (line: string) => needsOdoo(expandAlias(line, aliases));
  /** A Retry in flight keeps the chip in place, spinning, instead of hiding it while busy. */
  const [retrying, setRetrying] = useState(false);
  // A check that answers at once would only flash "Retrying…"; keep it long enough to be seen.
  const retryStarted = useRef(0);
  const RETRY_MIN_MS = 600;
  const editorHighlight = useRef<HTMLDivElement>(null);
  const syncScroll = () => {
    if (!editorHighlight.current || !field.current) return;
    editorHighlight.current.scrollLeft = field.current.scrollLeft;
    editorHighlight.current.scrollTop = field.current.scrollTop;
  };
  // Show parse errors once typing pauses; while typing, every prefix is incomplete.
  const [hint, setHint] = useState<string | null>(null);
  useEffect(() => {
    setHint(null);
    if (!input.trim() || confirming || search || /^!(!|\d+)$/.test(input.trim())) return;
    const timer = setTimeout(() => {
      // While the first word is being typed, the highlighter already marks unknown commands.
      // An open string or bracket means the user is mid-value; Enter still reports it.
      if (!/\s/.test(input.trim()) || unfinished(input)) return;
      try { parse(expand(input)); } catch (e) { if (e instanceof CommandError) setHint(e.message); }
    }, 700);
    return () => clearTimeout(timer);
  }, [input, confirming, search]);

  /** Check the tab's environment and connect; the status chip shows how it goes. */
  const reconnect = () => {
    const env = envs.find((e) => e.id === tab.envId);
    // Disconnected until the health check passes; \next must not reuse the old connection.
    update((s) => ({ ...s, env: undefined, last: undefined }));
    if (!env) return print({ kind: "error", text: t("shell.restoredEnvironmentGone", { name: tab.envName }), transient: true });
    // The environment may have been renamed since the tab was saved.
    if (tab.envName !== env.name) { tab.envName = env.name; onChange(); }
    void open(env);
  };
  // New and restored tabs connect the first time they are shown.
  useEffect(() => {
    if (!active || !tab.reconnect || busy || !envs.length) return;
    tab.reconnect = undefined;
    reconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, envs, busy]);
  const print = (...entries: NewEntry[]) => update((s) => ({ ...s, entries: [...s.entries, ...entries.map((e) => ({ ...e, id: nextId++ }) as Entry)].slice(-MAX_LINES) }));
  const text = (kind: "text" | "info" | "error" | "warn" | "ok", t: string) => print({ kind, text: t });
  /** `login>`: who the calls run as; the database shows at the end of the input line. */
  const prompt = () => {
    const env = envs.find((e) => e.id === tab.envId);
    if (!env) return "odoo>";
    return `${env.login.length > PROMPT_LOGIN ? `${env.login.slice(0, PROMPT_LOGIN - 1)}…` : env.login}>`;
  };
  /** Alias, then `$_` and `$name`; \\alias keeps its command as typed. */
  const expand = (line: string) => /^\s*\\alias(\s|$)/.test(line) ? line : substitute(expandAlias(line, aliases), tab.session.previous?.ids, tab.session.vars);
  /** The latest table's record ids become `$_`. */
  const remember = (table: Table) => {
    const idColumn = table.columns.indexOf("id");
    if (idColumn >= 0) update((s) => ({ ...s, previous: { model: table.model, ids: table.rows.map((row) => row[idColumn]).filter((id): id is number => typeof id === "number") } }));
  };

  /** Why the latest environment state refuses writes, mirroring policy::authorize; empty when allowed. */
  const writeBlocked = (id: string) => {
    const env = envs.find((e) => e.id === id);
    if (env?.write_mode === "read_only") return t("shell.readOnlyWrite", { name: env.name });
    if (env?.write_mode === "temporary" && !(env.write_until && Date.parse(env.write_until) > Date.now())) return t("shell.writeWindowExpired", { name: env.name });
    return "";
  };

  async function run(pending: Pending, confirmed: boolean) {
    const env = pending.env ?? tab.session.env;
    if (!env) return text("error", t("shell.noConnection"));
    const current = begin(confirmed);
    try {
      const result = await shell({ environment_id: env.id, ...pending.call, context: tab.session.context, confirmed });
      if (!current()) {
        // A discarded read is dropped; a confirmed write still reports what Odoo did.
        if (!confirmed) return;
        text("warn", t("shell.lateResult", { call: `${pending.call.method} ${pending.call.model}` }));
      }
      render(pending, result.data, tab.session.timing ? `${result.upstream_ms} ms · ${size(result.upstream_bytes)}` : "");
      // \next repeats search_read pages only; name_search has no offset.
      if (pending.page && pending.call.method === "search_read") update((s) => ({ ...s, last: pending }));
    } catch (reason) {
      if (!current() && !confirmed) return;
      const e = reason as AppError;
      // The core classifies writes and requires confirmation before sending them.
      if (e?.code === "confirmation_required" && !confirmed) {
        const c = pending.call;
        print({ kind: "warn", text: `${c.method} ${c.model} ${JSON.stringify(c.args)}${Object.keys(c.kwargs).length ? ` ${JSON.stringify(c.kwargs)}` : ""}` });
        // Don't ask for a confirmation the core will refuse anyway; the core still rejects with `forbidden`.
        const blocked = writeBlocked(env.id);
        if (blocked) text("error", blocked);
        // The mirror updates now: an agent run checks it right after this call, before the next render.
        else { confirmingNow.current = { ...pending, env }; setConfirming(confirmingNow.current); }
      } else if (e?.code === "uncertain_result") print({ kind: "warn", text: "", appError: normalizeAppError(reason) });
      else print({ kind: "error", text: "", appError: normalizeAppError(reason) });
    } finally { finish(current); }
  }

  function render(p: Pending, data: unknown, meta: string) {
    describe(p.call.model);
    const expanded = tab.session.expanded;
    if (Array.isArray(data) && p.display !== "groups") {
      // name_search returns [id, name] pairs; reads return records.
      const ids = data.map((r) => Array.isArray(r) ? r[0] : (r as { id?: unknown } | null)?.id).filter((id): id is number => typeof id === "number");
      update((s) => ({ ...s, previous: { model: p.call.model, ids: p.page ? ids.slice(0, p.page.limit) : ids } }));
    }
    const m = meta ? ` · ${meta}` : "";
    if (p.display === "fields" && data && typeof data === "object") {
      const q = p.filter?.toLowerCase();
      const rows = Object.entries(data as Record<string, Record<string, unknown>>)
        .filter(([name, f]) => !q || name.toLowerCase().includes(q) || String(f.string ?? "").toLowerCase().includes(q))
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([name, f]) => [name, f.type, f.relation ?? "", f.string, f.required ? t("shell.yes") : "", f.store === false ? t("shell.no") : ""]);
      return print({ kind: "table", expanded, table: { columns: [t("shell.field"), t("shell.type"), t("shell.relationship"), t("shell.label"), t("shell.required"), t("shell.stored")], rows, call: p.call }, footer: t("shell.count", { count: rows.length, item: t("shell.fields") }) + m });
    }
    if (p.display === "names" && Array.isArray(data)) {
      const more = !!p.page && data.length > p.page.limit;
      const rows = (p.page ? data.slice(0, p.page.limit) : data) as unknown[][];
      return print({ kind: "table", expanded, table: { columns: ["id", t("shell.name")], rows, model: p.call.model, call: p.call }, footer: t("shell.count", { count: rows.length, item: t("shell.results") }) + m + (more ? t("shell.moreRefine") : "") });
    }
    if (p.display === "groups" && Array.isArray(data) && data.length) {
      const groupby = p.call.kwargs.groupby as string[];
      const more = !!p.page && data.length > p.page.limit;
      const groups = (p.page ? data.slice(0, p.page.limit) : data) as Record<string, unknown>[];
      // Groupby columns first, then the count, then aggregates; drop Odoo's __domain/__range/__context.
      const rows = groups.map((g) => Object.fromEntries([
        ...groupby.map((key) => [key, g[key]]),
        ["count", g.__count],
        ...Object.entries(g).filter(([key]) => !key.startsWith("__") && !groupby.includes(key)),
      ]));
      return print({ kind: "table", expanded, table: { ...rowsTable(rows), model: p.call.model, call: p.call }, footer: t("shell.count", { count: rows.length, item: t("shell.groups") }) + m + (more ? t("shell.moreGroups") : "") });
    }
    if (Array.isArray(data) && data.length && data.every((r) => r && typeof r === "object" && !Array.isArray(r))) {
      let rows = data as Record<string, unknown>[];
      let more = false;
      if (p.page) { more = rows.length > p.page.limit; rows = rows.slice(0, p.page.limit); }
      const where = p.page ? t("shell.fromOffset", { offset: p.page.offset }) : "";
      // A single wide record reads better as a vertical list.
      const wide = rows.length === 1 && Object.keys(rows[0]).length > 12;
      return print({ kind: "table", expanded: expanded || wide, table: { ...rowsTable(rows), model: p.call.model, call: p.call }, footer: t("shell.row", { count: rows.length }) + where + m + (more ? t("shell.moreNext") : "") });
    }
    if (Array.isArray(data) && !data.length) return text("info", `${t("shell.noResults")}${m}`);
    print({ kind: "text", text: typeof data === "object" ? JSON.stringify(data, null, 2) : String(data) });
    if (meta) text("info", meta);
  }

  /** Run several read calls as one command; timing covers all of them. */
  async function sequence(work: (rpc: (call: OdooCall) => Promise<unknown>, env: Environment) => Promise<{ table: Table; footer?: string }>) {
    const env = envs.find((e) => e.id === tab.session.env?.id);
    if (!env) return text("error", t("shell.connectHint"));
    const current = begin();
    let ms = 0;
    let bytes = 0;
    const rpc = async (call: OdooCall) => {
      const r = await shell({ environment_id: env.id, ...call, context: tab.session.context, confirmed: false });
      ms += r.upstream_ms;
      bytes += r.upstream_bytes;
      return r.data;
    };
    try {
      const { table, footer } = await work(rpc, env);
      if (!current()) return;
      remember(table);
      describe(table.model);
      const meta = tab.session.timing ? `${ms} ms · ${size(bytes)}` : "";
      print({ kind: "table", expanded: tab.session.expanded, table, footer: [footer, meta].filter(Boolean).join(" · ") || undefined });
    } catch (reason) {
      if (!current()) return;
      if (reason instanceof CommandError) text("error", reason.message);
      else print({ kind: "error", text: "", appError: normalizeAppError(reason) });
    } finally { finish(current); }
  }

  /** The vault stores the login, not the uid; logins are unique per database. */
  async function currentUser(rpc: (call: OdooCall) => Promise<unknown>, env: Environment, fields: string[]) {
    const users = await rpc({ model: "res.users", method: "search_read", args: [], kwargs: { domain: [["login", "=", env.login]], fields, limit: 1 } }) as Record<string, unknown>[];
    if (!users.length) throw new CommandError(t("shell.userNotFound", { login: env.login }));
    return users[0];
  }

  /** Connection state lives on the status chip, not in the output: failures in its tooltip, the server on the mode's. */
  async function open(env: Environment) {
    const current = begin(false, true);
    setFailure(null);
    try {
      const health = await ipc.check(env.id);
      if (!current()) return;
      if (health.status !== "ok") return setFailure({
        name: env.name,
        reason: health.status === "auth_failed" ? "credential" : "offline",
        detail: health.error_code ? { code: health.error_code, params: health.error_params ?? undefined, detail: health.error ?? undefined } : undefined,
      });
      update((s) => ({ ...s, env: { id: env.id, name: env.name }, last: undefined }));
      setServer({ version: health.server_version ?? undefined, latency: health.latency_ms });
      if (!tab.session.models[env.id]) {
        void shell({ environment_id: env.id, model: "ir.model", method: "search_read", args: [], kwargs: { fields: ["model"], limit: 5000 } })
          .then((r) => update((s) => ({ ...s, models: { ...s.models, [env.id]: (r.data as { model: string }[]).map((m) => m.model).sort() } })))
          .catch(() => undefined);
      }
    } catch (reason) { if (current()) setFailure(normalizeAppError(reason)); } finally {
      finish(current);
      const left = RETRY_MIN_MS - (Date.now() - retryStarted.current);
      if (left > 0) setTimeout(() => { if (mounted.current) setRetrying(false); }, left); else setRetrying(false);
    }
  }

  /** Relation of a field, cached per environment. */
  const relations = useRef(new Map<string, string | undefined>());
  async function relationOf(envId: string, model: string, field: string) {
    const key = `${envId}:${model}.${field}`;
    if (!relations.current.has(key)) {
      const r = await shell({ environment_id: envId, model, method: "fields_get", args: [], kwargs: { allfields: [field], attributes: ["relation"] } });
      const relation = (r.data as Record<string, { relation?: string | false }>)[field]?.relation;
      relations.current.set(key, relation || undefined);
    }
    return relations.current.get(key);
  }

  /** A click runs a visible command, so it lands in history like a typed one; Ctrl+click opens the record in Odoo. */
  async function follow({ model, column, value, row }: Link, inOdoo = false) {
    const env = tab.session.env;
    if (!env || (!inOdoo && (busy || confirming))) return;
    let target: string | undefined;
    let ids: number[] = [];
    if (column === "id") { target = model; ids = [value as number]; }
    else if (column === "res_id" && model === "ir.model.data") { target = String(row.model); ids = [value as number]; }
    else {
      try { target = await relationOf(env.id, model, column); } catch (reason) { return print({ kind: "error", text: "", appError: normalizeAppError(reason) }); }
      if (!target) return text("error", t("shell.noRelation", { field: `${model}.${column}` }));
      ids = isM2o(value) ? [value[0]] : isIds(value) ? value : [];
    }
    if (inOdoo) {
      if (ids.length !== 1) return text("error", t("shell.openOneRecord"));
      return ipc.openUrl(env.id, { model: target, id: ids[0] }).catch((reason) => print({ kind: "error", text: "", appError: normalizeAppError(reason) }));
    }
    if (isIds(value) && column !== "id" && column !== "res_id") return submit(`search ${target} -d [('id','in',[${ids.join(",")}])]`);
    return submit(`read ${target} ${ids[0]} -f *`);
  }

  async function exportTable(table: Table, format: "csv" | "json") {
    const stamp = new Date().toISOString().slice(0, 19).replace(/[-:]/g, "").replace("T", "-");
    const name = `odx-${table.model ?? "table"}-${stamp}.${format}`;
    try {
      const path = await ipc.shellExport(name, format === "csv" ? toCsv(table) : toJson(table));
      text("ok", t("shell.exported", { path }));
    } catch (reason) { print({ kind: "error", text: "", appError: normalizeAppError(reason) }); }
  }

  const fold = (id: number) => update((s) => ({ ...s, entries: s.entries.map((e) => e.id === id && e.kind === "input" ? { ...e, folded: !e.folded } : e) }));
  const rerun = (command: string) => { if (!busy && !confirming) void submit(command); };

  const [find, setFind] = useState<{ query: string; index: number } | null>(null);
  const [matches, setMatches] = useState<Range[]>([]);
  const findField = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const root = out.current;
    if (!root || !find?.query) { setMatches([]); return; }
    const q = find.query.toLowerCase();
    const found: Range[] = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node && found.length < 1000; node = walker.nextNode()) {
      const value = node.nodeValue?.toLowerCase() ?? "";
      for (let at = value.indexOf(q); at >= 0 && found.length < 1000; at = value.indexOf(q, at + q.length)) {
        const range = document.createRange();
        range.setStart(node, at);
        range.setEnd(node, at + q.length);
        found.push(range);
      }
    }
    setMatches(found);
  }, [find?.query, session.entries]);
  const findIndex = matches.length && find ? ((find.index % matches.length) + matches.length) % matches.length : 0;
  useEffect(() => {
    // The CSS Custom Highlight API paints matches without touching React's DOM.
    const registry = (globalThis.CSS as unknown as { highlights?: Map<string, unknown> } | undefined)?.highlights;
    const Highlight = (globalThis as unknown as { Highlight?: new (...ranges: Range[]) => unknown }).Highlight;
    if (registry && Highlight) {
      registry.delete("term-find");
      registry.delete("term-find-current");
      if (matches.length) {
        registry.set("term-find", new Highlight(...matches));
        registry.set("term-find-current", new Highlight(matches[findIndex]));
      }
    }
    matches[findIndex]?.startContainer.parentElement?.scrollIntoView?.({ block: "nearest" });
  }, [matches, findIndex]);
  const openFind = () => {
    setFind((current) => current ?? { query: "", index: 0 });
    requestAnimationFrame(() => { findField.current?.focus(); findField.current?.select(); });
  };
  const closeFind = () => { setFind(null); field.current?.focus(); };

  /** Ctrl+F and font zoom work from the editor and the find bar alike. */
  function onTermKey(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "F1") { event.preventDefault(); setHelp((open) => !open); return; }
    if (event.key === "Escape" && help) { event.preventDefault(); setHelp(false); field.current?.focus(); return; }
    const inTable = (event.target as HTMLElement).classList.contains("term-scroll");
    if (inTable && event.key === "Escape") { event.preventDefault(); field.current?.focus(); return; }
    // Typing in a table goes back to the editor; the key lands there.
    if (inTable && event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey && event.key.toLowerCase() !== "c") { field.current?.focus(); return; }
    if ((event.ctrlKey || event.metaKey) && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      // Ctrl+↑ from the editor enters the newest table; inside, Ctrl+↑/↓ moves between tables.
      const tables = [...(out.current?.querySelectorAll<HTMLElement>(".term-scroll[tabindex]") ?? [])];
      const at = tables.indexOf(event.target as HTMLElement);
      const next = at < 0 ? (event.key === "ArrowUp" ? tables.length - 1 : -1) : at + (event.key === "ArrowUp" ? -1 : 1);
      event.preventDefault();
      if (next >= 0 && next < tables.length) tables[next].focus();
      else if (at >= 0 && next >= tables.length) field.current?.focus();
      return;
    }
    if (!event.ctrlKey && !event.metaKey) return;
    const key = event.key.toLowerCase();
    if (key === "f") { event.preventDefault(); openFind(); }
    else if (key === "+" || key === "=") { event.preventDefault(); setFontSize(fontSize + 1); }
    else if (key === "-") { event.preventDefault(); setFontSize(fontSize - 1); }
    else if (key === "0") { event.preventDefault(); setFontSize(DEFAULT_FONT); }
  }

  const [inspecting, setInspecting] = useState<{ title: string; text: string } | null>(null);
  function inspect(title: string, value: unknown) {
    let text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
    // Pretty-print text fields that hold JSON.
    if (typeof value === "string" && /^\s*[[{]/.test(value)) { try { text = JSON.stringify(JSON.parse(value), null, 2); } catch { /* plain text */ } }
    setInspecting({ title, text });
  }

  /** Enter on a line typed or found with Ctrl+R. */
  function send(line: string) {
    // An empty line does nothing; only a confirmation reads it, as its default "no".
    if (!line.trim() && !confirming) return setInput(line);
    // Without a connection, a command for Odoo is not sent nor kept in history: it stays typed for when it connects.
    if (!confirming && !tab.session.env && odooCommand(line)) {
      setInput(line);
      setHint(t(connecting ? "shell.waitConnection" : "shell.needsConnection"));
      setNudge((n) => n + 1);
      return;
    }
    setInput(""); setCursor(null); void submit(line);
  }
  async function submit(line: string, agent?: string) {
    if (confirming) {
      const yes = /^(y|s|si|sí|yes)$/i.test(line.trim());
      print({ kind: "input", prompt: t("shell.confirmPrompt", { name: confirming.env?.name }), text: line });
      const p = confirming;
      setConfirming(null);
      // The temporary window may have closed while the prompt was open.
      const blocked = yes && p.env ? writeBlocked(p.env.id) : "";
      if (blocked) text("error", blocked);
      else if (yes) await run(p, true);
      else { text("info", t("shell.cancelled")); if (batch) { setBatch(null); text("warn", t("shell.batchStopped", { index: batch.total - batch.lines.length, total: batch.total })); } }
      return;
    }
    // Load !n / !! into the input for review before execution.
    const bang = /^!(!|\d+)$/.exec(line.trim());
    if (bang) {
      const all = tab.session.history ?? [];
      const entry = bang[1] === "!" ? all[all.length - 1] : all[Number(bang[1]) - 1];
      if (!entry) return text("error", t("shell.missingHistoryCommand", { entry: bang[1] === "!" ? t("shell.previous") : t("shell.numbered", { number: bang[1] }) }));
      setInput(entry.command);
      return;
    }
    // The greeting and restore notes only matter until the first command actually runs.
    update((s) => ({ ...s, entries: s.entries.filter((entry) => !(entry.kind === "info" && (entry.translationKey === "shell.initial" || entry.transient))) }));
    print({ kind: "input", prompt: prompt(), text: line, command: !!line.trim(), ...agent ? { agent } : {} });
    // Looking at help or history, or clearing the screen, is not work worth recalling.
    if (line.trim() && !UNRECALLED.has(line.trim().split(/\s+/)[0])) {
      const entry = { at: new Date().toISOString(), environment: tab.session.env?.name ?? null, command: line.trim() };
      // Like shell_history_add, skip only consecutive duplicates in the same environment.
      update((s) => {
        const history = s.history ?? [];
        const last = history[history.length - 1];
        if (last?.command === entry.command && last.environment === entry.environment) return s;
        return { ...s, history: [...history, entry].slice(-500) };
      });
      void ipc.shellHistoryAdd(entry.command, entry.environment ?? undefined).catch(() => undefined);
    }
    let command: Command | null;
    const previous = tab.session.previous;
    let expanded: string;
    try { expanded = expand(line); command = parse(expanded); } catch (e) { return text("error", e instanceof CommandError ? e.message : String(e)); }
    if (!command) return;
    // Ids from another model are usually a mistake (ref leaves ir.model.data ids), so say where they come from.
    const target = command.kind === "call" ? command.call.model : command.kind === "metadata" ? command.model : undefined;
    if (target && command.kind !== "alias") for (const name of new Set(Array.from(expandAlias(line, aliases).matchAll(/\$([A-Za-z_]\w*)/g), (m) => m[1]))) {
      const source = name === "_" ? previous : tab.session.vars?.[name];
      if (source?.model && source.model !== target) text("warn", t("shell.previousFromModel", { name: `$${name}`, model: source.model, target }));
    }
    switch (command.kind) {
      case "help": return setHelp(true);
      case "clear": return update((s) => ({ ...s, entries: [] }));
      case "history": {
        if (command.clear) {
          await ipc.shellHistoryClear().catch((e) => print({ kind: "error", text: "", appError: normalizeAppError(e) }));
          update((s) => ({ ...s, history: [] }));
          return text("info", t("shell.historyCleared"));
        }
        const q = command.q?.toLowerCase();
        const all = tab.session.history ?? [];
        // When and what; a click on a command puts it in the input. The search still matches the environment.
        const rows = all.filter((h) => !q || h.command.toLowerCase().includes(q) || (h.environment ?? "").toLowerCase().includes(q))
          .map((h) => [formatDateTime(new Date(h.at), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }), h.command] as unknown[])
          .slice(-50);
        if (!rows.length) return text("info", q ? t("shell.noCommandMatches") : t("shell.emptyHistory"));
        return print({ kind: "table", expanded: false, table: { columns: [t("shell.columns.date"), t("shell.columns.command")], rows, code: [1] }, footer: t("shell.historyFooter", { shown: rows.length, total: all.length }) });
      }
      case "timing": {
        const timing = !tab.session.timing;
        update((s) => ({ ...s, timing }));
        return text("info", t("shell.timing", { state: timing ? t("common.visible") : t("common.hidden") }));
      }
      case "expanded": {
        const expanded = !tab.session.expanded;
        update((s) => ({ ...s, expanded }));
        return text("info", t("shell.expanded", { state: expanded ? t("common.enabled") : t("common.disabled") }));
      }
      case "list":
        if (!envs.length) return text("info", t("shell.noEnvironments"));
        return print({ kind: "table", expanded: false, table: { columns: [t("common.environment"), t("common.database"), t("common.url"), t("shell.writeMode")], rows: envs.map((e) => [e.name, e.database, e.url, t(modeLabels[e.write_mode] as "mode.readOnly")]) } });
      // Each session keeps its environment, so `$_` and saved ids can never reach another database.
      case "connect": case "disconnect": return text("info", t("shell.fixedEnvironment", { name: tab.envName }));
      case "context":
        if (command.value === undefined) return text("text", JSON.stringify(tab.session.context));
        update((s) => ({ ...s, context: command.value ?? {} }));
        return text("info", command.value ? t("shell.contextSet") : t("shell.contextCleared"));
      case "next": {
        const last = tab.session.last;
        if (!last?.page) return text("error", t("shell.noPreviousSearch"));
        const offset = last.page.offset + last.page.limit;
        const kw = last.call.kwargs as { domain: unknown[]; fields?: string[]; order?: string };
        return run({ ...last, page: { ...last.page, offset }, call: searchCall(last.call.model, kw.domain, kw.fields, last.page.limit, offset, kw.order) }, false);
      }
      case "prev": {
        const last = tab.session.last;
        if (!last?.page) return text("error", t("shell.noPreviousSearch"));
        if (!last.page.offset) return text("info", t("shell.firstPage"));
        const offset = Math.max(0, last.page.offset - last.page.limit);
        const kw = last.call.kwargs as { domain: unknown[]; fields?: string[]; order?: string };
        return run({ ...last, page: { ...last.page, offset }, call: searchCall(last.call.model, kw.domain, kw.fields, last.page.limit, offset, kw.order) }, false);
      }
      case "set": {
        const vars = tab.session.vars ?? {};
        const { name } = command;
        if (!name) {
          const rows = Object.entries(vars).map(([key, v]) => [`$${key}`, v.model ?? "", v.ids.length, v.ids.join(",")]);
          if (!rows.length) return text("info", t("shell.noVariables"));
          return print({ kind: "table", expanded: false, table: { columns: [t("shell.variable"), t("shell.model"), t("shell.records"), "ids"], rows } });
        }
        if (command.remove) {
          update((s) => ({ ...s, vars: Object.fromEntries(Object.entries(vars).filter(([key]) => key !== name)) }));
          return text("info", t("shell.variableRemoved", { name: `$${name}` }));
        }
        if (!previous?.ids.length) return text("error", t("shell.errors.noPreviousIds"));
        update((s) => ({ ...s, vars: { ...vars, [name]: previous } }));
        return text("ok", t("shell.variableSet", { name: `$${name}`, count: previous.ids.length, model: previous.model ?? "" }));
      }
      case "alias": {
        const { name } = command;
        if (!name) {
          const rows = Object.entries(aliases).sort(([a], [b]) => a.localeCompare(b));
          if (!rows.length) return text("info", t("shell.noAliases"));
          return print({ kind: "table", expanded: false, table: { columns: ["alias", t("shell.columns.command")], rows, code: [1] } });
        }
        if (command.command === null) {
          setAliases((current) => Object.fromEntries(Object.entries(current).filter(([key]) => key !== name)));
          return text("info", t("shell.aliasRemoved", { name }));
        }
        const body = command.command ?? "";
        setAliases((current) => ({ ...current, [name]: body }));
        return text("ok", t("shell.aliasSet", { name }));
      }
      case "grep": case "cols": {
        const source = [...tab.session.entries].reverse().find((e): e is Extract<Entry, { kind: "table" }> => e.kind === "table");
        if (!source) return text("error", t("shell.noTable"));
        const { table } = source;
        let next: Table;
        if (command.kind === "grep") {
          const q = command.q.toLowerCase();
          next = { ...table, rows: table.rows.filter((row) => row.some((v) => cellText(v).toLowerCase().includes(q))) };
        } else {
          const missing = command.columns.filter((c) => !table.columns.includes(c));
          if (missing.length) return text("error", t("shell.unknownColumns", { columns: missing.join(", "), available: table.columns.join(", ") }));
          const keep = command.hide ? table.columns.filter((c) => !command.columns.includes(c)) : command.columns;
          const at = keep.map((c) => table.columns.indexOf(c));
          next = { ...table, columns: keep, rows: table.rows.map((row) => at.map((j) => row[j])), code: table.code?.map((j) => at.indexOf(j)).filter((j) => j >= 0) };
        }
        remember(next);
        return print({ kind: "table", expanded: source.expanded, table: next, footer: t("shell.filtered", { shown: next.rows.length, count: table.rows.length }) });
      }
      case "whoami": return sequence(async (rpc, env) => {
        const user = await currentUser(rpc, env, ["login", "name", "company_id", "company_ids"]);
        return { table: { columns: ["uid", "login", t("shell.name"), "company_id", "company_ids"], rows: [[user.id, user.login, user.name, user.company_id, user.company_ids]], model: "res.users" } };
      });
      case "metadata": {
        const { model, ids } = command;
        return sequence(async (rpc) => {
          const records = await rpc({ model, method: "read", args: [ids], kwargs: { fields: ["create_uid", "create_date", "write_uid", "write_date"] } }) as Record<string, unknown>[];
          const xmlids = await rpc({ model, method: "get_external_id", args: [ids], kwargs: {} }) as Record<string, string>;
          const rows = records.map((r) => [r.id, xmlids[String(r.id)] || false, r.create_uid, r.create_date, r.write_uid, r.write_date]);
          return { table: { columns: ["id", "xmlid", "create_uid", "create_date", "write_uid", "write_date"], rows, model }, footer: t("shell.row", { count: rows.length }) };
        });
      }
      case "access": {
        const { model } = command;
        return sequence(async (rpc) => {
          const rows: unknown[][] = [];
          for (const operation of ["read", "write", "create", "unlink"]) {
            const allowed = await rpc({ model, method: "check_access_rights", args: [], kwargs: { operation, raise_exception: false } });
            rows.push([operation, allowed === true ? t("shell.yes") : t("shell.no")]);
          }
          return { table: { columns: [t("shell.operation"), t("shell.allowed")], rows } };
        });
      }
      case "has_group": {
        const { group } = command;
        return sequence(async (rpc, env) => {
          const [module, name] = [group.slice(0, group.indexOf(".")), group.slice(group.indexOf(".") + 1)];
          const refs = await rpc({ model: "ir.model.data", method: "search_read", args: [], kwargs: { domain: [["module", "=", module], ["name", "=", name]], fields: ["model", "res_id"], limit: 2 } }) as { model: string; res_id: number }[];
          if (refs.length !== 1 || refs[0].model !== "res.groups") throw new CommandError(t("shell.groupNotFound", { group }));
          // Odoo 19 renamed groups_id; all_group_ids includes implied groups, group_ids does not.
          const described = await rpc({ model: "res.users", method: "fields_get", args: [], kwargs: { allfields: GROUP_FIELDS, attributes: ["type"] } }) as Record<string, unknown>;
          const field = GROUP_FIELDS.find((f) => f in described);
          if (!field) throw new CommandError(t("shell.noGroupField"));
          const member = await rpc({ model: "res.users", method: "search_count", args: [[["login", "=", env.login], [field, "in", [refs[0].res_id]]]], kwargs: {} });
          return { table: { columns: [t("shell.group"), "id", "login", t("shell.member")], rows: [[group, refs[0].res_id, env.login, member ? t("shell.yes") : t("shell.no")]] }, footer: field === "group_ids" ? t("shell.directGroupsOnly") : undefined };
        });
      }
      case "call": {
        if (!tab.session.env) return text("error", t("shell.connectHint"));
        const pending: Pending = { call: command.call, display: command.display, filter: command.filter, page: command.page, env: tab.session.env };
        return run(pending, false);
      }
    }
  }

  const context = (sess: Session = tab.session) => ({
    models: sess.env ? sess.models[sess.env.id] : undefined,
    fields: (model: string) => (sess.env ? sess.fields[`${sess.env.id}:${model}`] : undefined),
    connected: !!sess.env,
    aliases,
  });

  // Shared with autocompletion: once loaded for a model, both use the same fields.
  const fieldLoads = useRef(new Set<string>());
  const tableFields = (model: string) => tab.session.env ? session.fields[`${tab.session.env.id}:${model}`] : undefined;
  /** After a fresh result only: restoring old output never calls Odoo. */
  const describe = (model?: string) => {
    const env = tab.session.env;
    if (!env || !model) return;
    const key = `${env.id}:${model}`;
    if (tab.session.fields[key] || fieldLoads.current.has(key)) return;
    fieldLoads.current.add(key);
    void load({ fields: model }).catch(() => fieldLoads.current.delete(key));
  };
  /** Load missing models or fields; report whether loading was needed. */
  async function load(needs: NonNullable<Completion["needs"]>) {
    const env = tab.session.env;
    if (!env) return;
    if ("models" in needs) {
      const r = await shell({ environment_id: env.id, model: "ir.model", method: "search_read", args: [], kwargs: { fields: ["model"], limit: 5000 } });
      const models = (r.data as { model: string }[]).map((m) => m.model).sort();
      update((s) => ({ ...s, models: { ...s.models, [env.id]: models } }));
    } else {
      const r = await shell({ environment_id: env.id, model: needs.fields, method: "fields_get", args: [], kwargs: { attributes: ["string", "type", "selection", "relation"] } });
      const fields = Object.entries(r.data as Record<string, { string: string; type: string; relation?: string | false; selection?: [string, string][] }>)
        .map(([name, f]) => ({ name, label: f.string, type: f.type, ...(f.relation ? { relation: f.relation } : {}), ...(Array.isArray(f.selection) ? { selection: f.selection } : {}) }))
        .sort((a, b) => a.name.localeCompare(b.name));
      update((s) => ({ ...s, fields: { ...s.fields, [`${env.id}:${needs.fields}`]: fields } }));
    }
  }

  function accept(m: Menu, value = input) {
    const item = m.items[m.index];
    if (!item) return;
    setInput(value.slice(0, m.from) + item.value + m.suffix);
    setMenu(null);
  }

  /** Tab completes a single match or opens the menu for multiple matches. */
  async function openMenu(value: string) {
    let found = completions(value, context());
    if (found?.needs) {
      try { await load(found.needs); } catch { return text("error", t("shell.suggestionsError")); }
      found = completions(value, context(tab.session));
    }
    if (!found || !found.items.length) { setMenu(null); return; }
    const m = { ...found, index: 0 };
    if (found.items.length === 1) accept(m, value); else setMenu(m);
  }

  /** Typing filters an open completion menu. */
  function refilter(value: string) {
    if (!menu) return;
    const found = completions(value, context());
    setMenu(found && found.items.length ? { ...found, index: 0 } : null);
  }

  useLayoutEffect(() => {
    menuList.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [menu]);

  function onKey(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (pasted) {
      if (event.key === "Enter") { event.preventDefault(); runPasted(); return; }
      if (event.key === "Escape") { event.preventDefault(); setInput(pasted.join("\n")); setPasted(null); return; }
    }
    if (inspecting && event.key === "Escape") { event.preventDefault(); setInspecting(null); return; }
    if (menu) {
      const move = (step: number) => setMenu({ ...menu, index: (menu.index + step + menu.items.length) % menu.items.length });
      if (event.key === "Tab") { event.preventDefault(); move(event.shiftKey ? -1 : 1); return; }
      if (event.key === "ArrowDown") { event.preventDefault(); move(1); return; }
      if (event.key === "ArrowUp") { event.preventDefault(); move(-1); return; }
      if (event.key === "Enter" || event.key === "ArrowRight") { event.preventDefault(); accept(menu); return; }
      if (event.key === "Escape") { event.preventDefault(); setMenu(null); return; }
    }
    if (search) {
      const match = findMatch(search.query, search.skip);
      if (event.ctrlKey && event.key.toLowerCase() === "r") { event.preventDefault(); if (findMatch(search.query, search.skip + 1)) setSearch({ ...search, skip: search.skip + 1 }); return; }
      // Enter runs what was found; Ctrl+G or Ctrl+C gives the line back as it was; any other move leaves the match to edit.
      if (event.ctrlKey && ["g", "c"].includes(event.key.toLowerCase())) { event.preventDefault(); setSearch(null); setInput(search.original); return; }
      const found = match?.command ?? search.original;
      if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); setSearch(null); if (busy) setInput(found); else send(found); return; }
      if (["Escape", "Tab", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
        event.preventDefault(); setSearch(null); setInput(found); setCursor(event.key === "Home" ? 0 : found.length); return;
      }
      return;
    }
    if (event.ctrlKey && event.key.toLowerCase() === "r" && !confirming) { event.preventDefault(); setMenu(null); setSearch({ query: "", skip: 0, original: input }); setInput(""); return; }
    if (busy) {
      if ((event.ctrlKey && event.key.toLowerCase() === "c" && !window.getSelection()?.toString()) || event.key === "Escape") { event.preventDefault(); discard(); return; }
      if (event.key === "Enter") { event.preventDefault(); return; }
    }
    if (ghost && (event.key === "ArrowRight" || event.key === "End") && event.currentTarget.selectionStart === input.length) {
      event.preventDefault();
      setInput(input + ghost);
      setCursor(null);
      return;
    }
    // Shift+Enter inserts a newline for long values and domains.
    if (event.key === "Enter" && event.shiftKey && !confirming) return;
    if (event.key === "Enter") {
      event.preventDefault();
      send(input);
      return;
    }
    if (event.key === "Tab") { event.preventDefault(); if (!confirming) void openMenu(input); return; }
    if (event.ctrlKey && event.key.toLowerCase() === "l") { event.preventDefault(); update((s) => ({ ...s, entries: [] })); return; }
    if ((event.ctrlKey && event.key.toLowerCase() === "c" && !window.getSelection()?.toString()) || event.key === "Escape") {
      event.preventDefault();
      if (confirming) { setConfirming(null); text("info", t("shell.cancelled")); }
      else if (input) { print({ kind: "input", prompt: prompt(), text: `${input}^C` }); setInput(""); }
      return;
    }
    if (confirming) return;
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && !event.ctrlKey && !event.metaKey) {
      // Inside a multi-line command the arrows move between lines; history starts at the edges.
      const el = event.currentTarget;
      if (event.key === "ArrowUp" && el.value.slice(0, el.selectionStart).includes("\n")) return;
      if (event.key === "ArrowDown" && el.value.slice(el.selectionEnd).includes("\n")) return;
      event.preventDefault();
      const h = (tab.session.history ?? []).map((e) => e.command);
      if (!h.length) return;
      const next = cursor === null ? (event.key === "ArrowUp" ? h.length - 1 : null) : cursor + (event.key === "ArrowUp" ? -1 : 1);
      if (next === null || next >= h.length) { setCursor(null); setInput(""); return; }
      const i = Math.max(0, next);
      setCursor(i);
      setInput(h[i]);
    }
  }

  /** Match containing `query`, with `skip=0` selecting the newest. */
  function findMatch(query: string, skip: number): HistoryEntry | undefined {
    const q = query.toLowerCase();
    return [...(tab.session.history ?? [])].reverse().filter((h) => !q || h.command.toLowerCase().includes(q))[skip];
  }
  const match = search ? findMatch(search.query, search.skip) : undefined;
  // As in bash, a query that stops matching keeps showing the last match its beginning found.
  const shownMatch = search && (match ?? Array.from({ length: search.query.length }, (_, n) => findMatch(search.query.slice(0, search.query.length - n - 1), 0)).find(Boolean));
  // fish-style suggestion: the newest single-line history command that extends the input.
  const ghost = !search && !confirming && !menu && input.trim() && !input.includes("\n")
    ? [...(tab.session.history ?? [])].reverse().find((h) => h.command.length > input.length && h.command.startsWith(input) && !h.command.includes("\n"))?.command.slice(input.length)
    : undefined;
  const current = search ? `${t(match ? "shell.reverseSearch" : "shell.reverseSearchFailed")} '` : confirming ? t("shell.confirmPrompt", { name: confirming.env?.name }) : prompt();
  const tabEnv = envs.find((env) => env.id === tab.envId);
  const connected = session.env ? tabEnv : undefined;
  // Disconnected and busy can only mean the health check is running: the first connection or a Retry.
  const connecting = retrying || (busy && !connected);
  const connectingLabel = retrying ? t("shell.retrying") : t("shell.connecting");
  const insert = (command: string) => { setInput(command); setHelp(false); requestAnimationFrame(() => { field.current?.focus(); field.current?.setSelectionRange(command.length, command.length); }); };

  return <div className="term" style={{ fontSize }} onKeyDown={onTermKey} onMouseUp={(event) => {
    if (!window.getSelection()?.toString() && !(event.target as HTMLElement).closest(".term-find, .term-scroll, .term-status, .term-help, .term-confirm")) field.current?.focus();
  }}>
    {find && <div className="term-find" role="search">
      <Icon name="search" />
      <input ref={findField} value={find.query} aria-label={t("shell.findLabel")} placeholder={t("shell.findLabel")} spellCheck={false}
        onChange={(e) => setFind({ query: e.target.value, index: 0 })}
        onKeyDown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); setFind({ ...find, index: findIndex + (e.shiftKey ? -1 : 1) }); }
          if (e.key === "Escape") { e.preventDefault(); closeFind(); }
        }} />
      <span className="term-find-count" role="status">{find.query ? (matches.length ? t("shell.findCount", { index: findIndex + 1, count: matches.length }) : t("shell.noMatches")) : ""}</span>
      <button type="button" className="icon-btn sm" aria-label={t("shell.previousMatch")} disabled={!matches.length} onClick={() => setFind({ ...find, index: findIndex - 1 })}><Icon name="caretUp" /></button>
      <button type="button" className="icon-btn sm" aria-label={t("shell.nextMatch")} disabled={!matches.length} onClick={() => setFind({ ...find, index: findIndex + 1 })}><Icon name="caretDown" /></button>
      <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={closeFind}><Icon name="x" /></button>
    </div>}
    <div className="term-out" ref={out} role="log" aria-live="polite" aria-label={t("shell.output")}>
      <SessionActions.Provider value={{ rerun, fold, busy: busy || !!confirming, aliases: Object.keys(aliases) }}>
        <CellActions.Provider value={{ follow: (link, inOdoo) => void follow(link, inOdoo), inspect, exportTable: (table, format) => void exportTable(table, format), fields: tableFields,
          callCode: (call) => tabEnv ? pythonCall(tabEnv, call, tab.session.context) : null, insert,
          // The window gets the table with what describes it (field labels and types); it shares nothing else with the session.
          popOut: (table, footer) => {
            // Named after the command that printed it (the input line above the table), then where it ran.
            const at = tab.session.entries.findIndex((entry) => entry.kind === "table" && entry.table === table);
            const source = tab.session.entries.slice(0, at).reverse().find((entry) => entry.kind === "input");
            // "environment (database) - command"
            const where = `${tabEnv?.name ?? tab.envName}${tabEnv ? ` (${tabEnv.database})` : ""}`;
            const title = [where, source?.text ?? table.model].filter(Boolean).join(" - ");
            // With the environment and session context, the window can repeat the read to add a column, and nothing more.
            void ipc.openTableWindow({ table, footer, fields: table.model ? tableFields(table.model) : undefined, title, envId: tab.envId, context: tab.session.context }, title).catch(() => undefined);
          } }}>
          {groups(visible(session.entries)).map((group) => <div key={group[0].entry.id} className="term-group" data-outcome={outcomeOf(group.map(({ entry }) => entry))}>
            {group.map(({ entry, hidden }) => <Line key={entry.id} entry={entry} hidden={hidden} onToggle={() => update((s) => ({
              ...s, entries: s.entries.map((e) => e.id === entry.id && e.kind === "table" ? { ...e, expanded: !e.expanded } : e),
            }))} />)}
          </div>)}
        </CellActions.Provider>
      </SessionActions.Provider>
      {/* Connecting is shown on the status chip; the output only tracks calls. */}
      {busy && !connecting && <Running since={started} />}
    </div>
    {help && <HelpPanel insert={insert} close={() => { setHelp(false); field.current?.focus(); }} />}
    {confirming && <WritePreview pending={confirming} answer={(yes) => void submit(yes ? "y" : "n")} />}
    {pasted && <div className="term-batch" role="group" aria-label={t("shell.pastedCommands", { count: pasted.length })} onMouseUp={(e) => e.stopPropagation()}>
      <span className="grow">{t("shell.pastedCommands", { count: pasted.length })}</span>
      <button type="button" className="btn sm" onClick={() => { setInput(pasted.join("\n")); setPasted(null); }}>{t("shell.pasteAsText")} <kbd>Esc</kbd></button>
      <button type="button" className="btn sm primary" onClick={runPasted}>{t("shell.runInOrder")} <kbd>Enter</kbd></button>
    </div>}
    {batch && <div className="term-batch" role="status" onMouseUp={(e) => e.stopPropagation()}>
      <Icon name="refresh" className="spin" /><span className="grow">{t("shell.batchProgress", { done: batch.total - batch.lines.length, total: batch.total })}</span>
      <button type="button" className="btn sm" onClick={() => setBatch(null)}>{t("shell.stop")}</button>
    </div>}
    {inspecting && <div className="term-inspect" role="dialog" aria-label={inspecting.title} onMouseUp={(e) => e.stopPropagation()}>
      <div className="term-inspect-head">
        <span className="term-inspect-title">{inspecting.title}</span>
        <CopyButton text={inspecting.text} className="term-copy" tip={t("common.copy")} ariaLabel={t("common.copy")} />
        <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={() => { setInspecting(null); field.current?.focus(); }}><Icon name="x" /></button>
      </div>
      <pre className="term-inspect-body">{inspecting.text}</pre>
    </div>}
    {menu && <ul className="term-menu" ref={menuList} role="listbox" aria-label={t("shell.suggestions")}>
      {menu.items.map((item, i) => <li key={`${item.kind}-${item.value}`} role="option" aria-selected={i === menu.index}
        onMouseDown={(e) => { e.preventDefault(); accept({ ...menu, index: i }); }}>
        <span className="term-menu-value">{item.value}</span>
        {item.detail && <span className="term-menu-detail">{item.translateDetail ? t(`shell.completions.${item.detail}` as "shell.completions.search") : item.detail}</span>}
      </li>)}
    </ul>}
    <label className={`term-input ${confirming ? "confirm" : ""} ${search ? "searching" : ""}`}>
      <span className="term-prompt">{current}</span>
      <span className="term-editor" style={search ? { flex: "none", width: `calc(${input.length}ch + 2px)` } : undefined}>
        {!confirming && <div className="term-highlight" ref={editorHighlight} aria-hidden="true">{search ? input : highlight(input, Object.keys(aliases))}{ghost && <span className="term-ghost">{ghost}</span>}</div>}
        <textarea ref={field} disabled={!tabEnv} value={input} rows={Math.min(8, input.split("\n").length)} wrap="off" spellCheck={false} autoComplete="off" autoCorrect="off" aria-invalid={hint ? true : undefined} aria-label={t("shell.command")} aria-description={search ? t("shell.reverseSearchDescription") : undefined}
          className={confirming ? "" : "highlighted"}
          onChange={(e) => { setInput(e.target.value); setCursor(null); if (search) setSearch({ ...search, query: e.target.value, skip: 0 }); else refilter(e.target.value); }}
          onScroll={syncScroll} onKeyUp={syncScroll} onSelect={syncScroll}
          onBlur={() => setMenu(null)} onKeyDown={onKey} onPaste={onPaste} />
      </span>
      {/* Reverse search keeps one line: the query, then what it finds. */}
      {search && <span className="term-search" role="status">{"': "}{shownMatch && <SearchMatch command={shownMatch.command} query={match ? search.query : ""} />}</span>}
      {/* Where the line goes and what it may do there; a broken connection offers its fix in place. */}
      {!search && !confirming && <span className="term-status">
        {!tabEnv ? <span className="chip chip-tone tone-danger">{t("shell.environmentGone")}</span> : <>
          {/* The database stays in view whatever the connection state. */}
          <span className="term-db">{tabEnv.database}</span>
          {connected ? <span className={`chip chip-tone term-mode tone-${modeTone[tabEnv.write_mode]}`}
            data-tip={[t(modeLabels[tabEnv.write_mode] as "mode.readOnly"), server && `${server.version ? `Odoo ${server.version} · ` : ""}${formatNumber(server.latency)} ms`].filter(Boolean).join("\n")}>
            <Icon name={modeIcons[tabEnv.write_mode]} />{t(modeShortLabels[tabEnv.write_mode] as "mode.readOnlyShort")}</span>
            // The state is its own fix: click it to connect again; the tooltip says what went wrong.
            : <button type="button" key={nudge} className={`chip chip-tone chip-action tone-${connecting ? "info" : failure ? "danger" : "neutral"}${nudge ? " nudge" : ""}`}
              disabled={connecting} aria-label={connecting ? connectingLabel : t("shell.retryConnection")} data-tip={!connecting && failure ? connectionProblem(failure) : undefined}
              onClick={() => { retryStarted.current = Date.now(); setRetrying(true); reconnect(); }}>
              <Icon name="refresh" className={connecting ? "spin" : undefined} />{connecting ? connectingLabel : failure ? t("shell.connectionFailed") : t("shell.notConnected")}
            </button>}
        </>}
      </span>}
      {hint && !menu && <span className="term-hint" role="status">{hint}</span>}
      {saveFailed && !hint && <span className="term-hint" role="status">{t("shell.sessionsNotSaved")}</span>}
    </label>
  </div>;
});

/** The command Ctrl+R found, first line only, with the query marked where it occurs. */
function SearchMatch({ command, query }: { command: string; query: string }) {
  const line = command.split("\n")[0];
  const at = query ? line.toLowerCase().indexOf(query.toLowerCase()) : -1;
  return <span className="term-search-match">{at < 0 ? line : <>{line.slice(0, at)}<mark>{line.slice(at, at + query.length)}</mark>{line.slice(at + query.length)}</>}{line !== command && " …"}</span>;
}

/** Entries after a folded command stay in the session but are not rendered. */
function visible(entries: Entry[]) {
  const shown: { entry: Entry; hidden?: number }[] = [];
  let folded: { hidden?: number } | null = null;
  for (const entry of entries) {
    if (entry.kind === "input") {
      const item = { entry, hidden: entry.folded ? 0 : undefined };
      folded = entry.folded ? item : null;
      shown.push(item);
    } else if (folded) folded.hidden = (folded.hidden ?? 0) + 1;
    else shown.push({ entry });
  }
  return shown;
}

/** A command and its output form one block, so hovering either shows the command's actions. */
function groups<T extends { entry: Entry }>(items: T[]): T[][] {
  const out: T[][] = [];
  for (const item of items) {
    if (item.entry.kind === "input" || !out.length) out.push([item]);
    else out[out.length - 1].push(item);
  }
  return out;
}

/** Past this many environments the menu gets a search box, as the tag filter does. */
const MENU_SEARCH_FROM = 8;

/** Environments for a new session; arrows move, Enter opens the first match, Escape and outside clicks close. */
function EnvironmentMenu({ envs, at, choose, close }: { envs: Environment[]; at: { left: number; top: number }; choose: (env: Environment) => void; close: () => void }) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const searchable = envs.length > MENU_SEARCH_FROM;
  const [query, setQuery] = useState("");
  const q = query.trim().toLowerCase();
  const shown = envs.filter((env) => !q || [env.name, hostOf(env.url), env.database, ...env.tags].some((value) => value.toLowerCase().includes(q)));
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>("input, [role=menuitem]")?.focus();
    // The "+" button toggles the menu itself.
    const outside = (event: Event) => { if (!ref.current?.contains(event.target as Node) && !(event.target as Element).closest?.("[aria-haspopup=menu]")) close(); };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("blur", close);
    return () => { window.removeEventListener("pointerdown", outside, true); window.removeEventListener("blur", close); };
  }, [close]);
  return <div ref={ref} className="more-menu-list floating shell-env-menu" role="menu" aria-label={t("shell.newSession")} style={at}
    onKeyDown={(event) => {
      const items = [...ref.current!.querySelectorAll<HTMLElement>("[role=menuitem]")];
      const index = items.indexOf(document.activeElement as HTMLElement);
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
      else if (event.key === "Enter" && index < 0 && shown[0]) { event.preventDefault(); choose(shown[0]); }
      else cycleFocus(event, items);
    }}>
    {searchable && <label className="search shell-env-search"><Icon name="search" />
      <input type="search" value={query} placeholder={t("shell.findEnvironment")} aria-label={t("shell.findEnvironment")} spellCheck={false} onChange={(e) => setQuery(e.target.value)} /></label>}
    <div className="shell-env-options">
      {shown.map((env) => <button key={env.id} type="button" role="menuitem" onClick={() => choose(env)}>
        <EnvIcon env={env} /><span className="grow truncate">{env.name}</span><span className={`status-dot tone-${modeTone[env.write_mode]}`} data-tip={t(modeLabels[env.write_mode] as "mode.readOnly")} />
      </button>)}
      {!shown.length && <p className="hint shell-env-empty">{t("shell.noMatches")}</p>}
    </div>
  </div>;
}

/** ArrowDown and ArrowUp move the focus through `items`, round. */
function cycleFocus(event: KeyboardEvent, items: HTMLElement[]) {
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  event.preventDefault();
  const index = items.indexOf(document.activeElement as HTMLElement);
  items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
}

type MenuItem = { label: string; keys?: string; disabled?: boolean; run: () => void } | null;
/** A tab's right-click menu; `null` items are separators. Arrows move, Escape or a press outside closes it. */
function TabMenu({ at, label, items, close }: { at: { left: number; top: number }; label: string; items: MenuItem[]; close: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(true, () => ref.current, close);
  // Focus the first item and keep the menu inside the window.
  useLayoutEffect(() => {
    placeInWindow(ref.current!, at.left, at.top);
    ref.current!.querySelector<HTMLElement>("[role=menuitem]:not(:disabled)")?.focus();
  }, [at]);
  return <div ref={ref} className="context-menu" role="menu" aria-label={label} style={at} onKeyDown={(event) => {
    if (event.key === "Escape" || event.key === "Tab") { event.preventDefault(); event.stopPropagation(); close(); }
    else cycleFocus(event, [...ref.current!.querySelectorAll<HTMLElement>("[role=menuitem]:not(:disabled)")]);
  }}>
    {items.map((item, i) => item ? <button key={item.label} type="button" role="menuitem" disabled={item.disabled} onClick={() => { close(); item.run(); }}>
      <span>{item.label}</span>{item.keys && <kbd>{item.keys}</kbd>}
    </button> : <hr key={i} />)}
  </div>;
}

/** `\?` as sections of commands; picking one types its first word, ready for arguments. Parsed from the translated
 * help so both views share one text: section titles start a line, commands are indented, the rest are notes. */
function helpSections(text: string) {
  const sections: { title: string; items: { usage: string; description: string }[] }[] = [];
  const notes: string[] = [];
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    if (/^\s/.test(line) && sections.length) {
      const [usage, ...rest] = line.trim().split(/\s{2,}/);
      sections[sections.length - 1].items.push({ usage, description: rest.join(" ") });
    } else if (!notes.length && !/[.:]\s/.test(line) && line.length < 40) sections.push({ title: line, items: [] });
    else notes.push(line);
  }
  return { sections, notes };
}

function HelpPanel({ insert, close }: { insert: (command: string) => void; close: () => void }) {
  const { t } = useTranslation();
  const { sections, notes } = helpSections(t("shell.help"));
  return <aside className="term-help" aria-label={t("shell.helpTitle")} onMouseUp={(e) => e.stopPropagation()}>
    <div className="term-inspect-head"><span className="term-inspect-title">{t("shell.helpTitle")}</span>
      <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={close}><Icon name="x" /></button></div>
    <div className="term-help-body">
      {sections.map((section) => <section key={section.title}><h4>{section.title}</h4>
        <ul>{section.items.map((item) => <li key={item.usage}>
          <button type="button" onClick={() => insert(`${item.usage.split(/\s/)[0]} `)}><code>{item.usage}</code>{item.description && <span>{item.description}</span>}</button>
        </li>)}</ul></section>)}
      {notes.map((note) => <p key={note} className="hint">{note}</p>)}
    </div>
  </aside>;
}

/** What a write will change, shown while the prompt asks to confirm it: the same view as Approvals. */
function WritePreview({ pending, answer }: { pending: Pending; answer: (yes: boolean) => void }) {
  const { t } = useTranslation();
  const { call, env } = pending;
  const ids = recordIds(call);
  const values = writtenValues(call);
  return <section className="term-confirm" aria-label={t("shell.confirmPrompt", { name: env?.name })} onMouseUp={(e) => e.stopPropagation()}>
    <header className="row gap-2">
      <code className={`method tone-${methodTone(call.method)}`}>{call.method}</code><strong className="mono truncate">{call.model}</strong>
      <span className="hint truncate">{env?.name}{ids ? ` · ${ids.join(", ")}` : ""}</span>
      <span className="row gap-2 ml-auto">
        <button type="button" className="btn sm" onClick={() => answer(false)}>{t("common.cancel")}</button>
        <button type="button" className={`btn sm ${call.method === "unlink" ? "danger" : "primary"}`} onClick={() => answer(true)}>{t("shell.send")}</button>
      </span>
    </header>
    {values?.length ? <ValuesTable values={values} /> : <CallDetails item={{ args: call.args, kwargs: call.kwargs, context: {} }} />}
  </section>;
}

function connectionProblem(problem: ConnectionErrorPresentation | AppError): string {
  if ("code" in problem) return formatAppError(problem);
  return i18n.t("shell.connectionError", { name: problem.name, reason: i18n.t(problem.reason === "credential" ? "shell.credentialRejected" : "shell.offline"), error: problem.detail ? ` (${formatAppError(problem.detail)})` : "" });
}

/** An entry as an agent reads it: text as shown, errors in words, tables as columns and (capped) rows. */
function plainEntry(entry: Entry): unknown {
  if (entry.kind === "table") return { columns: entry.table.columns, rows: entry.table.rows.slice(0, 200), total_rows: entry.table.rows.length, model: entry.table.model, footer: entry.footer };
  if (entry.kind === "input") return { kind: "input", text: entry.text };
  const text = entry.connectionError ? connectionProblem(entry.connectionError) : entry.appError ? formatAppError(entry.appError) : entry.translationKey ? i18n.t(entry.translationKey) : entry.text;
  return { kind: entry.kind, text };
}

/** A command block's mark in the margin: errors first, then writes (and their confirmations), then plain reads. */
function outcomeOf(entries: Entry[]): "error" | "write" | "read" | undefined {
  const [first] = entries;
  if (first?.kind !== "input" || !first.text.trim()) return undefined;
  if (entries.some((entry) => entry.kind === "error")) return "error";
  const verb = first.text.trim().split(/\s+/)[0];
  // A confirmation answer is its own block, under the write's question.
  return ["create", "write", "unlink"].includes(verb) || first.prompt.endsWith("]") ? "write" : "read";
}

const SessionActions = createContext<{ busy: boolean; rerun: (command: string) => void; fold: (id: number) => void; aliases: string[] } | null>(null);

function Line({ entry, hidden, onToggle }: { entry: Entry; hidden?: number; onToggle: () => void }): ReactNode {
  const { t } = useTranslation();
  const actions = useContext(SessionActions);
  if (entry.kind === "input") return <div className="term-line term-cmd">{entry.agent && <span className="term-agent" data-tip={t("shell.ranBy", { name: entry.agent })}>{entry.agent}</span>}<span className="term-prompt">{entry.prompt}</span> {highlight(entry.text, actions?.aliases)}
    {hidden !== undefined && <button type="button" className="term-folded" onClick={() => actions?.fold(entry.id)}>{t("shell.linesHidden", { count: hidden })}</button>}
    {entry.command && actions && <span className="term-cmd-actions">
      <button type="button" className="icon-btn sm" aria-label={t("shell.rerun")} data-tip={t("shell.rerun")} disabled={actions.busy} onClick={() => actions.rerun(entry.text)}><Icon name="refresh" /></button>
      <CopyButton text={entry.text} className="icon-btn sm" tip={t("shell.copyCommand")} ariaLabel={t("shell.copyCommand")} iconOnly />
      <button type="button" className="icon-btn sm" aria-label={t(entry.folded ? "shell.unfold" : "shell.fold")} data-tip={t(entry.folded ? "shell.unfold" : "shell.fold")} aria-expanded={!entry.folded} onClick={() => actions.fold(entry.id)}><Icon name={entry.folded ? "chevron" : "caretDown"} /></button>
    </span>}
  </div>;
  // Sessions saved before connection errors moved to the status chip may still hold them as lines.
  if (entry.kind !== "table") return <div className={`term-line ${entry.kind}`}>{entry.connectionError
    ? connectionProblem(entry.connectionError)
    : entry.appError ? <ErrorText error={entry.appError} /> : entry.translationKey ? t(entry.translationKey) : entry.text}</div>;
  return <TableBlock table={entry.table} expanded={entry.expanded} footer={entry.footer} onToggle={onToggle} />;
}

/** Many2one values sort by name; empty values go last in both directions. */
function compareCells(a: unknown, b: unknown): number {
  const key = (v: unknown) => Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string" ? v[1] : v;
  const x = key(a);
  const y = key(b);
  if (typeof x === "number" && typeof y === "number") return x - y;
  return cellText(x).localeCompare(cellText(y), undefined, { numeric: true, sensitivity: "base" });
}
const empty = (v: unknown) => v === false || v === null || v === undefined || v === "";

/** `bare`: only the table, for a window that brings its own controls. `headerEnd`: a control in the header's last cell,
 * kept at the right edge while scrolling sideways. */
export function TableBlock({ table, expanded, footer, onToggle, bare, headerEnd }: { table: Table; expanded: boolean; footer?: string; onToggle: () => void; bare?: boolean; headerEnd?: ReactNode }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const aliases = useContext(SessionActions)?.aliases;
  const [sort, setSort] = useState<{ column: number; desc: boolean } | null>(null);
  const described = table.model ? actions?.fields(table.model) : undefined;
  /** Label, type and relation of a technical column name, when the model's fields are known. */
  const about = (column: string) => {
    const field = described?.find((f) => f.name === column);
    return field ? `${field.label}\n${field.type}${field.relation ? ` → ${field.relation}` : ""}` : undefined;
  };
  const [active, setActive] = useState<{ r: number; c: number } | null>(null);
  // Cells, columns and rows are selected like a spreadsheet's, not their text: `anchor` is where a range starts.
  const [selection, setSelection] = useState<{ cells: Set<string>; anchor: { r: number; c: number } } | null>(null);
  const dragging = useRef(false);
  // The cursor frame marks where the keys act: shown for a lone cell or while moving with the keyboard, not over a mouse selection.
  const [keyboard, setKeyboard] = useState(false);
  // A press anywhere outside the table lets go of its selection.
  useDismiss(!!selection, () => scroller.current, () => setSelection(null));
  const [copied, setCopied] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const rows = sort === null ? table.rows : [...table.rows].sort((a, b) => {
    const [x, y] = [a[sort.column], b[sort.column]];
    if (empty(x) || empty(y)) return Number(empty(x)) - Number(empty(y));
    return sort.desc ? compareCells(y, x) : compareCells(x, y);
  });
  // A sum per numeric column (record ids aside), over the rows shown: \grep filters first. Empty values count as nothing.
  const totals = rows.length > 1 ? table.columns.map((column, j) => {
    if (column === "id" || !rows.every((row) => row[j] === null || row[j] === false || typeof row[j] === "number")) return null;
    const values = rows.map((row) => row[j]).filter((value): value is number => typeof value === "number");
    return values.length ? { sum: values.reduce((a, b) => a + b, 0), count: values.length } : null;
  }) : null;
  const totalsShown = totals?.some(Boolean) ? totals : null;
  const record = (row: unknown[]) => Object.fromEntries(table.columns.map((c, j) => [c, row[j]]));
  // Ascending, descending, then back to the original order.
  const cycle = (column: number) => setSort(sort?.column !== column ? { column, desc: false } : sort.desc ? null : { column, desc: true });
  // The id column stays visible while scrolling sideways.
  const pinned = table.columns[0] === "id" || table.columns[0] === "#";
  const cellKey = (r: number, c: number) => `${r}:${c}`;
  const box = (a: { r: number; c: number }, b: { r: number; c: number }) => {
    const cells = new Set<string>();
    for (let r = Math.min(a.r, b.r); r <= Math.max(a.r, b.r); r++) for (let c = Math.min(a.c, b.c); c <= Math.max(a.c, b.c); c++) cells.add(cellKey(r, c));
    return cells;
  };
  const lastRow = rows.length - 1;
  const lastColumn = table.columns.length - 1;
  /** Shift extends from the anchor, Ctrl adds or removes, otherwise the target replaces the selection. */
  const choose = (target: Set<string>, anchor: { r: number; c: number }, end: { r: number; c: number }, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => {
    if (event.shiftKey && selection) return setSelection({ ...selection, cells: box(selection.anchor, end) });
    if ((event.ctrlKey || event.metaKey) && selection) {
      const cells = new Set(selection.cells);
      const all = [...target].every((key) => cells.has(key));
      for (const key of target) { if (all) cells.delete(key); else cells.add(key); }
      return setSelection({ cells, anchor });
    }
    setSelection({ cells: target, anchor });
  };
  const selectCell = (r: number, c: number, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => { setActive({ r, c }); choose(new Set([cellKey(r, c)]), { r, c }, { r, c }, event); };
  // Whole columns and rows: Shift runs from the anchor's column (or row) to this one, across the table.
  const selectColumn = (c: number, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => {
    if (event.shiftKey && selection) return setSelection({ ...selection, cells: box({ r: 0, c: selection.anchor.c }, { r: lastRow, c }) });
    choose(box({ r: 0, c }, { r: lastRow, c }), { r: 0, c }, { r: lastRow, c }, event);
  };
  const selectRow = (r: number, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => {
    if (event.shiftKey && selection) return setSelection({ ...selection, cells: box({ r: selection.anchor.r, c: 0 }, { r, c: lastColumn }) });
    choose(box({ r, c: 0 }, { r, c: lastColumn }), { r, c: 0 }, { r, c: lastColumn }, event);
  };
  /** Outline the selected blocks like a spreadsheet: a side gets a line where the neighbour is not selected. */
  const edgesOf = (r: number, c: number) => {
    const has = (row: number, column: number) => !!selection?.cells.has(cellKey(row, column));
    if (!has(r, c)) return undefined;
    const line = "var(--accent)";
    return [!has(r - 1, c) && `inset 0 1px 0 ${line}`, !has(r, c + 1) && `inset -1px 0 0 ${line}`, !has(r + 1, c) && `inset 0 -1px 0 ${line}`, !has(r, c - 1) && `inset 1px 0 0 ${line}`]
      .filter(Boolean).join(", ") || undefined;
  };
  const selectedColumn = (c: number) => !!selection && rows.length > 0 && rows.every((_, r) => selection.cells.has(cellKey(r, c)));
  const selectedRow = (r: number) => !!selection && table.columns.every((_, c) => selection.cells.has(cellKey(r, c)));
  /** One cell copies as its value; more copy as TSV with the headers of their columns, blanks where a row skips one. */
  const selectionText = () => {
    const keys = selection?.cells.size ? [...selection.cells].map((key) => key.split(":").map(Number)) : active ? [[active.r, active.c]] : [];
    if (keys.length === 1) return cellText(rows[keys[0][0]][keys[0][1]]);
    const clean = (text: string) => text.replace(/[\t\n\r]/g, " ");
    const used = (index: 0 | 1) => [...new Set(keys.map((key) => key[index]))].sort((a, b) => a - b);
    const columns = used(1);
    return [columns.map((c) => clean(table.columns[c])).join("\t"),
      ...used(0).map((r) => columns.map((c) => selection!.cells.has(cellKey(r, c)) ? clean(cellText(rows[r][c])) : "").join("\t"))].join("\n");
  };
  const copySelection = () => void navigator.clipboard.writeText(selectionText()).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1200); }).catch(() => undefined);
  useEffect(() => {
    const stop = () => { dragging.current = false; };
    window.addEventListener("mouseup", stop);
    return () => window.removeEventListener("mouseup", stop);
  }, []);
  useLayoutEffect(() => {
    if (active) scroller.current?.querySelector(`[data-cell="${active.r}:${active.c}"]`)?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [active]);

  function onKey(event: KeyboardEvent<HTMLDivElement>) {
    if (expanded || !rows.length || event.altKey) return;
    const at = active ?? { r: 0, c: 0 };
    const last = { r: rows.length - 1, c: table.columns.length - 1 };
    const move = (r: number, c: number) => {
      event.preventDefault();
      const to = { r: Math.max(0, Math.min(last.r, r)), c: Math.max(0, Math.min(last.c, c)) };
      setActive(to);
      setKeyboard(true);
      // Shift+arrows grow the selection from its anchor; plain arrows move a one-cell selection.
      setSelection(event.shiftKey && selection ? { ...selection, cells: box(selection.anchor, to) } : { cells: new Set([cellKey(to.r, to.c)]), anchor: to });
    };
    const ctrl = event.ctrlKey || event.metaKey;
    if (ctrl && event.key.toLowerCase() === "a") { event.preventDefault(); setSelection({ cells: box({ r: 0, c: 0 }, last), anchor: { r: 0, c: 0 } }); return; }
    switch (event.key) {
      case "ArrowUp": if (!ctrl) move(at.r - 1, at.c); return;
      case "ArrowDown": if (!ctrl) move(at.r + 1, at.c); return;
      case "ArrowLeft": return move(at.r, at.c - 1);
      case "ArrowRight": return move(at.r, at.c + 1);
      case "Home": return ctrl ? move(0, 0) : move(at.r, 0);
      case "End": return ctrl ? move(last.r, last.c) : move(at.r, last.c);
      case "PageUp": return move(at.r - 10, at.c);
      case "PageDown": return move(at.r + 10, at.c);
      case "Enter": {
        event.preventDefault();
        const td = scroller.current?.querySelector(`[data-cell="${at.r}:${at.c}"]`);
        const button = td?.querySelector("button");
        if (button) button.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, ctrlKey: ctrl }));
        else actions?.inspect?.(`${table.model ? `${table.model} · ` : ""}${table.columns[at.c]}`, rows[at.r][at.c]);
        return;
      }
    }
    if (event.key.toLowerCase() === "c" && !event.shiftKey) {
      event.preventDefault();
      copySelection();
    }
  }

  return <div className="term-block">
    <div className="term-scroll" ref={scroller} tabIndex={expanded ? undefined : 0} data-pinned={pinned || undefined}
      aria-label={t("shell.tableNavigation")} onKeyDown={onKey}
      onFocus={() => { if (!active) setActive({ r: 0, c: 0 }); }}
      onMouseDown={(event) => {
        if (event.button !== 0 || expanded) return;
        const cell = (event.target as HTMLElement).closest<HTMLElement>("[data-cell]")?.dataset.cell;
        if (!cell) return;
        const [r, c] = cell.split(":").map(Number);
        selectCell(r, c, event);
        setKeyboard(false);
        dragging.current = !event.shiftKey && !event.ctrlKey && !event.metaKey;
      }}
      onMouseOver={(event) => {
        if (!dragging.current || !selection) return;
        const cell = (event.target as HTMLElement).closest<HTMLElement>("[data-cell]")?.dataset.cell;
        if (cell) { const [r, c] = cell.split(":").map(Number); setSelection({ ...selection, cells: box(selection.anchor, { r, c }) }); }
      }}>
      {expanded ? rows.map((row, i) => { const values = record(row); return <table key={i} className="term-table expanded"><tbody>
        <tr><th colSpan={2}>{t("shell.record", { index: i + 1 })}</th></tr>
        {table.columns.map((c, j) => <tr key={c}><th data-tip={about(c)}>{c}</th><Cell value={row[j]} column={c} model={table.model} row={values} /></tr>)}
      </tbody></table>; })
        : <table className="term-table" data-copied={copied || undefined}><thead><tr>
          <th className="term-row-head" aria-label={t("shell.selectAll")} data-tip={t("shell.selectAll")} onMouseDown={(event) => { event.preventDefault(); setSelection({ cells: box({ r: 0, c: 0 }, { r: lastRow, c: lastColumn }), anchor: { r: 0, c: 0 } }); }} />
          {table.columns.map((c, j) => <th key={c} className={pinned && j === 0 ? "pin" : undefined} data-selected={selectedColumn(j) || undefined} data-tip={about(c)}
            aria-sort={sort?.column === j ? (sort.desc ? "descending" : "ascending") : undefined}
            onMouseDown={(event) => { if (event.button !== 0 || (event.target as HTMLElement).closest(".term-sort")) return; event.preventDefault(); selectColumn(j, event); }}>
          <span className="term-column">{c}</span>
          <button type="button" className="term-sort" tabIndex={-1} aria-label={t("shell.sortBy", { column: c })} onClick={() => cycle(j)}>
            <Icon name={sort?.column === j && sort.desc ? "caretDown" : "caretUp"} className={sort?.column === j ? "on" : undefined} />
          </button>
        </th>)}{headerEnd && <th className="term-header-end">{headerEnd}</th>}</tr></thead>
          <tbody>{rows.map((row, i) => { const values = record(row); return <tr key={i}>
            <td className="term-row-head" data-selected={selectedRow(i) || undefined} onMouseDown={(event) => { if (event.button !== 0) return; event.preventDefault(); selectRow(i, event); }}>{i + 1}</td>
            {row.map((v, j) => {
            const pos = `${i}:${j}`;
            const on = active?.r === i && active.c === j && (keyboard || (selection?.cells.size ?? 0) <= 1);
            const selected = selection?.cells.has(pos);
            const edges = edgesOf(i, j);
            return table.code?.includes(j)
              ? <td key={j} data-cell={pos} data-active={on || undefined} data-selected={selected || undefined} style={edges ? { boxShadow: edges } : undefined}>{actions?.insert
                ? <button type="button" className="term-link term-command" tabIndex={-1} data-tip={t("shell.editCommand")} onDoubleClick={() => actions.insert?.(String(v))}>{highlight(String(v), aliases)}</button>
                : highlight(String(v), aliases)}</td>
              : <Cell key={j} value={v} column={table.columns[j]} model={table.model} row={values} pos={pos} active={on} selected={selected} edges={edges} pin={pinned && j === 0} />;
          })}{headerEnd && <td className="term-header-end" />}</tr>; })}</tbody>
          {totalsShown && <tfoot><tr><td className="term-row-head" aria-label={t("shell.total")}>Σ</td>{totalsShown.map((total, j) => <td key={j} className={[pinned && j === 0 && "pin", total && "num"].filter(Boolean).join(" ") || undefined}>
            {total && <span data-tip={t("shell.average", { value: formatNumber(total.sum / total.count, { maximumFractionDigits: 2 }) })}>{formatNumber(total.sum, { maximumFractionDigits: 2 })}</span>}
          </td>)}{headerEnd && <td className="term-header-end" />}</tr></tfoot>}
        </table>}
    </div>
    {!bare && <div className="term-foot">
      <span className="term-foot-info">{footer ? `(${footer})` : ""}{copied ? ` · ${t("common.copied")}` : ""}</span>
      <span className="term-foot-actions">
        <button type="button" className="term-copy" aria-label={t(expanded ? "shell.collapseTable" : "shell.expandTable")} data-tip={t(expanded ? "shell.collapseTable" : "shell.expandTable")}
          aria-pressed={expanded} onClick={onToggle}><Icon name={expanded ? "table" : "rows"} /></button>
        {actions?.popOut && <button type="button" className="term-copy" aria-label={t("shell.openInWindow")} data-tip={t("shell.openInWindow")} onClick={() => actions.popOut?.(table, footer)}><Icon name="external" /></button>}
        <CopyTable table={{ ...table, rows }} />
      </span>
    </div>}
  </div>;
}

/** Elapsed time of the call in progress. */
function Running({ since }: { since: number }) {
  const { t } = useTranslation();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 100); return () => clearInterval(timer); }, []);
  const seconds = Math.max(0, now - since) / 1000;
  return <div className="term-line info term-running" role="status">
    <Icon name="refresh" className="spin" /> {formatNumber(seconds, { minimumFractionDigits: 1, maximumFractionDigits: 1 })} s · {t("shell.discardHint")}
  </div>;
}

export const toTsv = (table: Table) => [table.columns, ...table.rows.map((row) => row.map(cellText))]
  .map((cells) => cells.map((c) => String(c).replace(/[\t\n\r]/g, " ")).join("\t"))
  .join("\n");
/** Raw values keyed by column, as Odoo returned them. */
export const toJson = (table: Table) => JSON.stringify(table.rows.map((row) => Object.fromEntries(table.columns.map((c, j) => [c, row[j] ?? null]))), null, 2);
export const toMarkdown = (table: Table) => {
  const line = (cells: string[]) => `| ${cells.map((c) => c.replace(/\|/g, "\\|").replace(/[\n\r]+/g, " ")).join(" | ")} |`;
  return [line(table.columns), line(table.columns.map(() => "---")), ...table.rows.map((row) => line(row.map(cellText)))].join("\n");
};
/** RFC 4180 with a BOM so spreadsheets detect UTF-8. Odoo data can come from outside (web forms), so values
 * that a spreadsheet would run as a formula get a leading apostrophe. */
const csvCell = (value: string) => {
  const safe = /^[=+\-@\t\r]/.test(value) ? `'${value}` : value;
  return /[",\n\r]/.test(safe) ? `"${safe.replace(/"/g, '""')}"` : safe;
};
export const toCsv = (table: Table) => "\uFEFF" + [table.columns, ...table.rows.map((row) => row.map(cellText))]
  .map((cells) => cells.map(csvCell).join(","))
  .join("\r\n");

/** TSV for spreadsheets one click away; other formats behind a small menu. */
function CopyTable({ table }: { table: Table }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const menu = useRef<HTMLSpanElement>(null);
  // Not on blur: the terminal refocuses its editor on mouseup, which would unmount the menu before the click.
  useDismiss(open, () => menu.current, () => setOpen(false));
  const code = table.call ? actions?.callCode?.(table.call) : null;
  const copy = (label: string, text: string) => {
    void navigator.clipboard.writeText(text).then(() => { setCopied(label); setTimeout(() => setCopied(null), 1500); }).catch(() => undefined);
    setOpen(false);
  };
  return <>
    <CopyButton text={() => toTsv(table)} className="term-copy" tip={t("shell.copyTsvTip")} ariaLabel={t("shell.copyTsvLabel")} label="TSV" />
    <span className="term-table-menu" ref={menu} onMouseDown={(e) => e.preventDefault()}
      onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); setOpen(false); } }}>
      <button type="button" className="term-copy" aria-haspopup="menu" aria-expanded={open} aria-label={t("shell.moreTableActions")} data-tip={copied ? t("shell.copiedAs", { format: copied }) : t("shell.moreTableActions")}
        onClick={() => setOpen(!open)}>{copied ? <Icon name="check" /> : "⋯"}</button>
      {open && <div className="term-menu term-table-actions" role="menu">
        <button type="button" role="menuitem" onClick={() => copy("JSON", toJson(table))}>{t("shell.copyAs", { format: "JSON" })}</button>
        <button type="button" role="menuitem" onClick={() => copy("Markdown", toMarkdown(table))}>{t("shell.copyAs", { format: "Markdown" })}</button>
        {code && <button type="button" role="menuitem" onClick={() => copy("Python", code)}>{t("shell.copyAs", { format: "Python" })}</button>}
        {actions?.exportTable && <>
          <button type="button" role="menuitem" onClick={() => { setOpen(false); actions.exportTable?.(table, "csv"); }}>{t("shell.saveAs", { format: "CSV" })}</button>
          <button type="button" role="menuitem" onClick={() => { setOpen(false); actions.exportTable?.(table, "json"); }}>{t("shell.saveAs", { format: "JSON" })}</button>
        </>}
      </div>}
    </span>
  </>;
}

/** Odoo's own message comes first; long messages fold their tail. */
function ErrorText({ error }: { error: AppError }) {
  const { t } = useTranslation();
  const title = formatAppError({ ...error, detail: undefined });
  const detail = error.detail?.trim();
  if (!detail) return <>{title}</>;
  const lines = detail.split("\n");
  const [first, ...rest] = lines;
  return <>
    <span className="term-error-title">{title}</span>{"\n"}{first}
    {rest.length > 0 && <details className="term-error-more">
      <summary>{t("shell.moreErrorLines", { count: rest.length })}</summary>
      {rest.join("\n")}
    </details>}
  </>;
}
