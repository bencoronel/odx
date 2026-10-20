import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { ipc, type Environment, type HistoryEntry, type ShellError } from "./ipc";
import { highlight } from "./shell/highlight";
import { CommandError, expandAlias, parse, searchCall, substitute, unfinished, type Command, type OdooCall } from "./shell/commands";
import { completions, type Completion, type Field } from "./shell/complete";
import { modeLabels } from "./ModeSelect";
import { Icon } from "./Icon";
import CopyButton from "./CopyButton";
import { useTranslation } from "react-i18next";
import { formatDateTime, formatNumber } from "./i18n/format";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { hasWindowState, useWindowState, usePreventUnload } from "./windowState";

const MAX_LINES = 400;
const FONT_KEY = "odx.shell.fontSize";
const FONT_RANGE = [10, 24] as const;
const DEFAULT_FONT = 12;
function storedFont(): number {
  try { const n = Number(localStorage.getItem(FONT_KEY)); return n >= FONT_RANGE[0] && n <= FONT_RANGE[1] ? n : DEFAULT_FONT; } catch { return DEFAULT_FONT; }
}
const GROUP_FIELDS = ["all_group_ids", "groups_id", "group_ids"];

/** `code`: column indexes containing commands to highlight. `model`: source of the rows, for clickable ids and relations. */
type Table = { columns: string[]; rows: unknown[][]; code?: number[]; model?: string };
type ConnectionErrorPresentation = { name: string; reason: "credential" | "offline"; detail?: AppError };
type Entry =
  /** `command`: a typed command that can be re-run; `folded` hides its output. `transient`: shown but never saved (restore notes). */
  | { id: number; kind: "input"; prompt: string; text: string; command?: boolean; folded?: boolean; transient?: boolean }
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
/** `reconnect`: restored from the vault; check the saved environment when the tab is first shown. */
type ShellTab = { id: string; name: string; defaultName: boolean; session: Session; busy: boolean; reconnect?: boolean };

/** Saved in the vault (encrypted) so tabs and output survive restarts. Pending confirmations are never saved.
 * `aliases` are shared by every tab. */
type SavedSessions = { version: 1; activeId: string; aliases?: Record<string, string>; tabs: { id: string; name: string; defaultName: boolean; session: Pick<Session, "entries" | "env" | "context" | "expanded" | "timing" | "last" | "previous" | "vars"> }[] };
const SAVE_LIMIT = 7 * 1024 * 1024;
function snapshot(tabs: ShellTab[], activeId: string, aliases: Record<string, string>): string {
  const saved: SavedSessions = { version: 1, activeId, aliases, tabs: tabs.map((tab) => ({
    id: tab.id, name: tab.name, defaultName: tab.defaultName,
    session: { entries: tab.session.entries.filter((entry) => !("transient" in entry && entry.transient)), env: tab.session.env, context: tab.session.context, expanded: tab.session.expanded, timing: tab.session.timing, last: tab.session.last, previous: tab.session.previous, vars: tab.session.vars },
  })) };
  // Over the limit, drop the oldest output of the largest tab until it fits. The vault limits UTF-8 bytes, not characters.
  const bytes = (value: unknown) => new TextEncoder().encode(JSON.stringify(value)).length;
  const sizes = saved.tabs.map(bytes);
  while (sizes.reduce((a, b) => a + b, 0) > SAVE_LIMIT) {
    const i = sizes.indexOf(Math.max(...sizes));
    const entries = saved.tabs[i].session.entries;
    if (!entries.length) break;
    saved.tabs[i].session.entries = entries.slice(Math.max(1, Math.ceil(entries.length / 10)));
    sizes[i] = bytes(saved.tabs[i]);
  }
  return JSON.stringify(saved);
}
/** An untouched tab: default name, no environment and only informational lines. */
const pristine = (tab: Pick<ShellTab, "defaultName"> & { session?: Partial<Pick<Session, "env" | "entries">> }) =>
  tab.defaultName && !tab.session?.env && (tab.session?.entries ?? []).every((e) => e.kind === "info");
function restore(data: string, note: string): { tabs: ShellTab[]; activeId: string } | null {
  try {
    const saved = JSON.parse(data) as SavedSessions;
    if (saved?.version !== 1 || !Array.isArray(saved.tabs)) return null;
    // Untouched tabs are not a session worth announcing; with only those, start fresh.
    const used = saved.tabs.filter((tab) => !pristine(tab));
    if (!used.length) return null;
    const tabs = used.map((tab): ShellTab => {
      const entries = Array.isArray(tab.session?.entries) ? tab.session.entries : [];
      for (const entry of entries) nextId = Math.max(nextId, entry.id + 1);
      return {
        id: tab.id, name: tab.name, defaultName: tab.defaultName, busy: false, reconnect: !!tab.session.env,
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
const newTab = (): ShellTab => ({ id: crypto.randomUUID(), name: "", defaultName: true, session: fresh(), busy: false });

function cellText(v: unknown): string {
  if (v === null || v === undefined) return "null";
  if (typeof v === "string") return v;
  if (Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string") return `${v[1]} (${v[0]})`;
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}
const isM2o = (v: unknown): v is [number, string] => Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string";
const isIds = (v: unknown): v is number[] => Array.isArray(v) && v.length > 0 && v.every((n) => Number.isInteger(n) && n > 0);
type Link = { model: string; column: string; value: unknown; row: Record<string, unknown> };
/** Cell clicks: follow a record or relation, or open a long value. */
const CellActions = createContext<{ follow: (link: Link, inOdoo?: boolean) => void; inspect: (title: string, value: unknown) => void; exportTable: (table: Table, format: "csv" | "json") => void } | null>(null);

/** `pos` ("row:column") and `active` support keyboard navigation; `pin` keeps the column visible when scrolling sideways. */
function Cell({ value, column, model, row, pos, active, pin }: { value: unknown; column: string; model?: string; row: Record<string, unknown>; pos?: string; active?: boolean; pin?: boolean }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const text = cellText(value);
  const dim = value === false || value === null || value === undefined || (Array.isArray(value) && !value.length);
  const short = text.length > 80 ? `${text.slice(0, 80)}…` : text;
  const className = `${dim ? "dim" : ""} ${typeof value === "number" ? "num" : ""} ${pin ? "pin" : ""}`;
  const cell = { "data-cell": pos, "data-active": active || undefined };
  // ir.model.data rows point at another model through res_id.
  const linked = model && actions && (
    (column === "id" && typeof value === "number")
    || (model === "ir.model.data" && column === "res_id" && typeof value === "number" && typeof row.model === "string")
    || (column !== "id" && (isM2o(value) || isIds(value))));
  if (linked) return <td className={className} {...cell}><button type="button" className="term-link" tabIndex={-1} data-tip={t("shell.openRecord")}
    onClick={(event) => actions.follow({ model, column, value, row }, event.ctrlKey || event.metaKey)}>{short}</button></td>;
  if (actions && (short !== text || (value !== null && typeof value === "object"))) return <td className={className} {...cell}>
    <button type="button" className="term-more" tabIndex={-1} data-tip={t("shell.inspectValue")}
      onClick={() => actions.inspect(`${model ? `${model} ` : ""}${row.id !== undefined ? `${cellText(row.id)} · ` : ""}${column}`, value)}>{short}</button>
  </td>;
  return <td className={className} {...cell} data-tip={short !== text ? text : undefined}>{short}</td>;
}
function rowsTable(rows: Record<string, unknown>[]): Table {
  const columns: string[] = [];
  for (const row of rows) for (const key of Object.keys(row)) if (!columns.includes(key)) columns.push(key);
  return { columns, rows: rows.map((row) => columns.map((c) => row[c])) };
}
const size = (bytes: number) => bytes < 1024 ? `${formatNumber(bytes)} B` : `${formatNumber(bytes / 1024, { maximumFractionDigits: 1 })} KB`;

export default function ShellPage({ envs, connectTo, command, onConnectHandled, visible = true }: {
  /** `command` runs once connected to `connectTo`. */
  envs: Environment[]; connectTo?: string | null; command?: string | null; onConnectHandled?: () => void; visible?: boolean;
}) {
  const { t } = useTranslation();
  const [tabs, setTabs] = useWindowState("shell.tabs", () => [newTab()], (tabs) => tabs.map((tab) => ({
    ...tab, busy: false, session: { ...tab.session, history: null, models: {}, fields: {} },
  })));
  // Restored entries keep their ids; new entries must not reuse them.
  const seeded = useRef(false);
  if (!seeded.current) {
    seeded.current = true;
    for (const tab of tabs) for (const entry of tab.session.entries) nextId = Math.max(nextId, entry.id + 1);
  }
  const [activeId, setActiveId] = useWindowState("shell.activeId", tabs[0].id);
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
  const [saveFailed, setSaveFailed] = useState(false);
  const save = () => {
    clearTimeout(saveTimer.current);
    saveTimer.current = undefined;
    void Promise.resolve(ipc.saveShellSessions(snapshot(state.current.tabs, state.current.activeId, state.current.aliases)))
      .then(() => setSaveFailed(false)).catch(() => setSaveFailed(true));
  };
  const persist = () => {
    if (!loaded) return;
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(save, 800);
  };
  useEffect(persist, [tabs, activeId, aliases, loaded]);
  // Flush on quit or when the window goes away.
  useEffect(() => {
    const flush = () => { if (saveTimer.current !== undefined) save(); };
    window.addEventListener("pagehide", flush);
    return () => { window.removeEventListener("pagehide", flush); flush(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [renaming, setRenaming] = useState<{ id: string; value: string; width: number } | null>(null);
  const [fontSize, setFontSizeState] = useState(storedFont);
  const setFontSize = (size: number) => {
    const next = Math.min(FONT_RANGE[1], Math.max(FONT_RANGE[0], size));
    setFontSizeState(next);
    try { localStorage.setItem(FONT_KEY, String(next)); } catch { /* per-window only */ }
  };
  usePreventUnload(tabs.some((tab) => tab.busy) || renaming !== null);
  const refreshTabs = () => setTabs((current) => [...current]);
  const select = (id: string) => setActiveId(id);
  const add = () => {
    const tab = newTab();
    setTabs((current) => [...current, tab]);
    select(tab.id);
    return tab.id;
  };
  // A requested environment reuses a tab already connected to it, then an untouched active tab, else opens a new
  // one: a session in use never switches environment. Waits for saved tabs, since a restore changes the active tab.
  const [targetId, setTargetId] = useState<string | null>(null);
  useEffect(() => {
    if (!connectTo || !loaded) { setTargetId(null); return; }
    const name = connectTo.toLowerCase();
    const connected = tabs.filter((tab) => tab.session.env?.name.toLowerCase() === name);
    const active = tabs.find((tab) => tab.id === activeId);
    const target = connected.find((tab) => tab === active) ?? connected[0] ?? (active && pristine(active) ? active : undefined);
    const id = target?.id ?? add();
    select(id);
    setTargetId(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectTo, loaded]);
  const close = (tab: ShellTab) => {
    if (tab.busy) return;
    const index = tabs.indexOf(tab);
    const remaining = tabs.filter((item) => item !== tab);
    if (!remaining.length) remaining.push(newTab());
    setTabs(remaining);
    if (activeId === tab.id) select(remaining[Math.min(index, remaining.length - 1)].id);
    if (renaming?.id === tab.id) setRenaming(null);
  };
  const rename = (tab: ShellTab) => {
    const width = document.getElementById(`shell-tab-${tab.id}`)?.getBoundingClientRect().width;
    if (width === undefined) return;
    select(tab.id);
    setRenaming({ id: tab.id, value: tab.defaultName ? t("navigation.shell.title") : tab.name, width });
  };
  const finishRename = (tab: ShellTab) => {
    if (renaming?.id !== tab.id) return;
    tab.name = renaming.value.trim() || (tab.defaultName ? t("navigation.shell.title") : tab.name);
    tab.defaultName = false;
    setRenaming(null);
    refreshTabs();
  };
  return <div className="shell-sessions">
    <div className="shell-tabs" role="tablist" aria-label={t("shell.sessions")} aria-orientation="horizontal">
      {tabs.map((tab, index) => <div className="shell-tab" key={tab.id} role="presentation" data-active={activeId === tab.id}
        onMouseDown={(event) => { if (event.button === 1) event.preventDefault(); }}
        onAuxClick={(event) => { if (event.button === 1) { event.preventDefault(); close(tab); } }}>
        {renaming?.id === tab.id ? <div className="shell-tab-editor" style={{ width: renaming.width }}>
          <Icon name={tab.busy ? "refresh" : "terminal"} className={tab.busy ? "spin" : undefined} />
          <input className="shell-tab-name" aria-label={t("shell.sessionName")} autoFocus maxLength={80} spellCheck={false} autoCorrect="off" autoComplete="off"
          value={renaming.value} onFocus={(event) => event.currentTarget.select()}
          onChange={(event) => setRenaming({ ...renaming, value: event.target.value })}
          onBlur={() => finishRename(tab)} onKeyDown={(event) => {
            if (event.key === "Enter") { event.preventDefault(); finishRename(tab); }
            if (event.key === "Escape") { event.preventDefault(); setRenaming(null); }
          }} />
          {tab.session.env && <span className="shell-tab-env">{tab.session.env.name}</span>}
        </div> : <button type="button" role="tab" id={`shell-tab-${tab.id}`} aria-controls={`shell-panel-${tab.id}`}
          aria-selected={activeId === tab.id} tabIndex={activeId === tab.id ? 0 : -1}
          onClick={() => select(tab.id)} onDoubleClick={(event) => { event.preventDefault(); rename(tab); }} onKeyDown={(event) => {
            if (event.key === "F2") { event.preventDefault(); rename(tab); return; }
            let target: ShellTab | undefined;
            if (event.key === "ArrowRight") target = tabs[(index + 1) % tabs.length];
            if (event.key === "ArrowLeft") target = tabs[(index + tabs.length - 1) % tabs.length];
            if (event.key === "Home") target = tabs[0];
            if (event.key === "End") target = tabs[tabs.length - 1];
            if (target) { event.preventDefault(); select(target.id); document.getElementById(`shell-tab-${target.id}`)?.focus(); }
          }}>
          <Icon name={tab.busy ? "refresh" : "terminal"} className={tab.busy ? "spin" : undefined} />
          <span>{tab.defaultName ? t("navigation.shell.title") : tab.name}</span>{tab.session.env && <span className="shell-tab-env">{tab.session.env.name}</span>}
        </button>}
        <button type="button" className="icon-btn sm shell-tab-close" aria-label={t("shell.closeSession", { name: tab.defaultName ? t("navigation.shell.title") : tab.name })}
          disabled={tab.busy} data-tip={tab.busy ? t("shell.callInProgress") : t("shell.closeSessionLabel")} onClick={() => close(tab)}><Icon name="x" /></button>
      </div>)}
      <button type="button" className="icon-btn sm" aria-label={t("shell.newSession")} data-tip={t("shell.newSessionLabel")} onClick={add}><Icon name="plus" /></button>
    </div>
    {tabs.map((tab) => <div className="shell-panel" key={tab.id} role="tabpanel" id={`shell-panel-${tab.id}`}
      aria-labelledby={`shell-tab-${tab.id}`} hidden={activeId !== tab.id}>
      <ShellSession tab={tab} envs={envs} active={visible && activeId === tab.id} onChange={refreshTabs} onPersist={persist} saveFailed={saveFailed} fontSize={fontSize} setFontSize={setFontSize}
        aliases={aliases} setAliases={setAliases}
        connectTo={tab.id === targetId ? connectTo : null} command={command} onConnectHandled={onConnectHandled} />
    </div>)}
  </div>;
}

function ShellSession({ tab, envs, active, onChange, onPersist, saveFailed, connectTo, command, onConnectHandled, fontSize, setFontSize, aliases, setAliases }: {
  tab: ShellTab; envs: Environment[]; active: boolean; onChange: () => void; onPersist: () => void; saveFailed: boolean;
  connectTo?: string | null; command?: string | null; onConnectHandled?: () => void;
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
  const setBusy = (value: boolean) => { if (!mounted.current) return; tab.busy = value; setBusyState(value); onChange(); };
  const [started, setStarted] = useState(0);
  // Ctrl+C bumps the ticket: the call keeps running in Odoo, but its stale result is ignored.
  const ticket = useRef(0);
  const writing = useRef(false);
  const begin = (write = false) => {
    const id = ++ticket.current;
    writing.current = write;
    setStarted(Date.now());
    setBusy(true);
    return () => ticket.current === id;
  };
  const finish = (current: () => boolean) => { if (current()) setBusy(false); };
  const discard = () => {
    ticket.current++;
    setBusy(false);
    print({ kind: "input", prompt: prompt(tab.session), text: "^C" });
    text("warn", t(writing.current ? "shell.discardedWrite" : "shell.discarded"));
  };
  const [confirming, setConfirming] = useWindowState<Pending | null>(`shell.${tab.id}.confirming`, null);
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
  const [search, setSearch] = useState<{ query: string; skip: number } | null>(null);
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

  const handled = useRef<string | null>(null);
  useEffect(() => {
    if (!connectTo) { handled.current = null; return; }
    if (busy) return;
    if (handled.current === connectTo) return; // One attempt per request.
    handled.current = connectTo;
    onConnectHandled?.();
    if (confirming) { setConfirming(null); text("info", t("shell.cancelled")); }
    void (async () => {
      if (tab.session.env?.name !== connectTo) {
        print({ kind: "input", prompt: prompt(tab.session), text: `\\c ${connectTo}`, command: true });
        await connect(connectTo);
      }
      // Run only where requested; a failed connection already explains itself.
      if (command && tab.session.env?.name === connectTo) await submit(command);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectTo, busy]);
  // A tab restored from the vault reconnects the first time it is shown.
  useEffect(() => {
    if (!active || !tab.reconnect || busy || !envs.length) return;
    tab.reconnect = false;
    // A requested connection wins; reconnecting here would cancel it.
    if (connectTo) return;
    const saved = tab.session.env;
    if (!saved) return;
    const env = envs.find((e) => e.id === saved.id);
    // Disconnected until the health check passes; \\next must not reuse the old connection.
    update((s) => ({ ...s, env: undefined, last: undefined }));
    if (!env) return print({ kind: "error", text: t("shell.restoredEnvironmentGone", { name: saved.name }), transient: true });
    print({ kind: "input", prompt: prompt(tab.session), text: `\\c ${env.name}`, command: true, transient: true });
    void open(env, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, envs, busy]);
  const print = (...entries: NewEntry[]) => update((s) => ({ ...s, entries: [...s.entries, ...entries.map((e) => ({ ...e, id: nextId++ }) as Entry)].slice(-MAX_LINES) }));
  const text = (kind: "text" | "info" | "error" | "warn" | "ok", t: string) => print({ kind, text: t });
  const prompt = (s: Session) => s.env ? `odoo@${s.env.name}>` : "odoo>";
  /** Alias, then `$_` and `$name`; \\alias keeps its command as typed. */
  const expand = (line: string) => /^\s*\\alias(\s|$)/.test(line) ? line : substitute(expandAlias(line, aliases), tab.session.previous?.ids, tab.session.vars);
  /** The latest table's record ids become `$_`. */
  const remember = (table: Table) => {
    const idColumn = table.columns.indexOf("id");
    if (idColumn >= 0) update((s) => ({ ...s, previous: { model: table.model, ids: table.rows.map((row) => row[idColumn]).filter((id): id is number => typeof id === "number") } }));
  };

  async function run(pending: Pending, confirmed: boolean) {
    const env = pending.env ?? tab.session.env;
    if (!env) return text("error", t("shell.noConnection"));
    const current = begin(confirmed);
    try {
      const result = await ipc.shell({ environment_id: env.id, ...pending.call, context: tab.session.context, confirmed });
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
      const e = reason as ShellError;
      // The core classifies writes and requires confirmation before sending them.
      if (e?.code === "confirmation_required" && !confirmed) {
        const c = pending.call;
        print({ kind: "warn", text: `${c.method} ${c.model} ${JSON.stringify(c.args)}${Object.keys(c.kwargs).length ? ` ${JSON.stringify(c.kwargs)}` : ""}` });
        setConfirming({ ...pending, env });
      } else if (e?.code === "uncertain_result") print({ kind: "warn", text: "", appError: normalizeAppError(reason) });
      else print({ kind: "error", text: "", appError: normalizeAppError(reason) });
    } finally { finish(current); }
  }

  function render(p: Pending, data: unknown, meta: string) {
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
      return print({ kind: "table", expanded, table: { columns: [t("shell.field"), t("shell.type"), t("shell.relationship"), t("shell.label"), t("shell.required"), t("shell.stored")], rows }, footer: t("shell.count", { count: rows.length, item: t("shell.fields") }) + m });
    }
    if (p.display === "names" && Array.isArray(data)) {
      const more = !!p.page && data.length > p.page.limit;
      const rows = (p.page ? data.slice(0, p.page.limit) : data) as unknown[][];
      return print({ kind: "table", expanded, table: { columns: ["id", t("shell.name")], rows, model: p.call.model }, footer: t("shell.count", { count: rows.length, item: t("shell.results") }) + m + (more ? t("shell.moreRefine") : "") });
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
      return print({ kind: "table", expanded, table: { ...rowsTable(rows), model: p.call.model }, footer: t("shell.count", { count: rows.length, item: t("shell.groups") }) + m + (more ? t("shell.moreGroups") : "") });
    }
    if (Array.isArray(data) && data.length && data.every((r) => r && typeof r === "object" && !Array.isArray(r))) {
      let rows = data as Record<string, unknown>[];
      let more = false;
      if (p.page) { more = rows.length > p.page.limit; rows = rows.slice(0, p.page.limit); }
      const where = p.page ? t("shell.fromOffset", { offset: p.page.offset }) : "";
      // A single wide record reads better as a vertical list.
      const wide = rows.length === 1 && Object.keys(rows[0]).length > 12;
      return print({ kind: "table", expanded: expanded || wide, table: { ...rowsTable(rows), model: p.call.model }, footer: t("shell.row", { count: rows.length }) + where + m + (more ? t("shell.moreNext") : "") });
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
      const r = await ipc.shell({ environment_id: env.id, ...call, context: tab.session.context, confirmed: false });
      ms += r.upstream_ms;
      bytes += r.upstream_bytes;
      return r.data;
    };
    try {
      const { table, footer } = await work(rpc, env);
      if (!current()) return;
      remember(table);
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

  async function connect(name: string) {
    const lower = name.toLowerCase();
    const exact = envs.filter((e) => e.name.toLowerCase() === lower);
    const matches = exact.length ? exact : envs.filter((e) => e.name.toLowerCase().startsWith(lower));
    if (matches.length !== 1) return text("error", matches.length ? t("shell.ambiguousEnvironment", { names: matches.map((e) => e.name).join(", ") }) : t("shell.unknownEnvironment", { name }));
    return open(matches[0]);
  }

  /** `transient`: an automatic reconnection after a restart, whose lines are not saved. */
  async function open(env: Environment, transient = false) {
    const say = (entry: NewEntry) => print(transient ? { ...entry, transient: true } as NewEntry : entry);
    const current = begin();
    try {
      const health = await ipc.check(env.id);
      if (!current()) return;
      if (health.status !== "ok") return say({ kind: "error", text: "", connectionError: {
        name: env.name,
        reason: health.status === "auth_failed" ? "credential" : "offline",
        detail: health.error_code ? { code: health.error_code, params: health.error_params ?? undefined, detail: health.error ?? undefined } : undefined,
      } });
      update((s) => ({ ...s, env: { id: env.id, name: env.name }, last: undefined }));
      say({ kind: "ok", text: t("shell.connected", { name: env.name, database: env.database, version: health.server_version ? ` · Odoo ${health.server_version}` : "", latency: health.latency_ms }) });
      if (!tab.session.models[env.id]) {
        void ipc.shell({ environment_id: env.id, model: "ir.model", method: "search_read", args: [], kwargs: { fields: ["model"], limit: 5000 } })
          .then((r) => update((s) => ({ ...s, models: { ...s.models, [env.id]: (r.data as { model: string }[]).map((m) => m.model).sort() } })))
          .catch(() => undefined);
      }
    } catch (reason) { if (current()) say({ kind: "error", text: "", appError: normalizeAppError(reason) }); } finally { finish(current); }
  }

  /** Relation of a field, cached per environment. */
  const relations = useRef(new Map<string, string | undefined>());
  async function relationOf(envId: string, model: string, field: string) {
    const key = `${envId}:${model}.${field}`;
    if (!relations.current.has(key)) {
      const r = await ipc.shell({ environment_id: envId, model, method: "fields_get", args: [], kwargs: { allfields: [field], attributes: ["relation"] } });
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

  async function submit(line: string) {
    if (confirming) {
      const yes = /^(y|s|si|sí|yes)$/i.test(line.trim());
      print({ kind: "input", prompt: t("shell.confirmPrompt", { name: confirming.env?.name }), text: line });
      const p = confirming;
      setConfirming(null);
      if (yes) await run(p, true); else text("info", t("shell.cancelled"));
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
    print({ kind: "input", prompt: prompt(tab.session), text: line, command: !!line.trim() });
    if (line.trim()) {
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
      case "help": return print({ kind: "text", text: "", translationKey: "shell.help" });
      case "clear": return update((s) => ({ ...s, entries: [] }));
      case "history": {
        if (command.clear) {
          await ipc.shellHistoryClear().catch((e) => print({ kind: "error", text: "", appError: normalizeAppError(e) }));
          update((s) => ({ ...s, history: [] }));
          return text("info", t("shell.historyCleared"));
        }
        const q = command.q?.toLowerCase();
        const all = tab.session.history ?? [];
        const rows = all.map((h, i) => [i + 1, formatDateTime(new Date(h.at), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }), h.environment ?? "", h.command] as unknown[])
          .filter((r) => !q || String(r[3]).toLowerCase().includes(q) || String(r[2]).toLowerCase().includes(q))
          .slice(-50);
        if (!rows.length) return text("info", q ? t("shell.noCommandMatches") : t("shell.emptyHistory"));
        return print({ kind: "table", expanded: false, table: { columns: ["#", t("shell.columns.date"), t("common.environment"), t("shell.columns.command")], rows, code: [3] }, footer: t("shell.historyFooter", { shown: rows.length, total: all.length }) });
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
      case "connect": return connect(command.env);
      case "disconnect":
        update((s) => ({ ...s, env: undefined, last: undefined }));
        return text("info", t("shell.disconnected"));
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
    envs: envs.map((e) => ({ name: e.name, detail: e.database })),
    models: sess.env ? sess.models[sess.env.id] : undefined,
    fields: (model: string) => (sess.env ? sess.fields[`${sess.env.id}:${model}`] : undefined),
    connected: !!sess.env,
    aliases,
  });

  /** Load missing models or fields; report whether loading was needed. */
  async function load(needs: NonNullable<Completion["needs"]>) {
    const env = tab.session.env;
    if (!env) return;
    if ("models" in needs) {
      const r = await ipc.shell({ environment_id: env.id, model: "ir.model", method: "search_read", args: [], kwargs: { fields: ["model"], limit: 5000 } });
      const models = (r.data as { model: string }[]).map((m) => m.model).sort();
      update((s) => ({ ...s, models: { ...s.models, [env.id]: models } }));
    } else {
      const r = await ipc.shell({ environment_id: env.id, model: needs.fields, method: "fields_get", args: [], kwargs: { attributes: ["string", "type", "selection"] } });
      const fields = Object.entries(r.data as Record<string, { string: string; type: string; selection?: [string, string][] }>)
        .map(([name, f]) => ({ name, label: f.string, type: f.type, ...(Array.isArray(f.selection) ? { selection: f.selection } : {}) }))
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
      if (event.key === "Enter" || event.key === "ArrowRight" || event.key === "Tab") { event.preventDefault(); setSearch(null); setInput(match?.command ?? ""); return; }
      if (event.key === "Escape" || (event.ctrlKey && ["g", "c"].includes(event.key.toLowerCase()))) { event.preventDefault(); setSearch(null); setInput(""); return; }
      return;
    }
    if (event.ctrlKey && event.key.toLowerCase() === "r" && !confirming) { event.preventDefault(); setMenu(null); setSearch({ query: input, skip: 0 }); setInput(input); return; }
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
    if (event.key === "Enter") { event.preventDefault(); const line = input; setInput(""); setCursor(null); void submit(line); return; }
    if (event.key === "Tab") { event.preventDefault(); if (!confirming) void openMenu(input); return; }
    if (event.ctrlKey && event.key.toLowerCase() === "l") { event.preventDefault(); update((s) => ({ ...s, entries: [] })); return; }
    if ((event.ctrlKey && event.key.toLowerCase() === "c" && !window.getSelection()?.toString()) || event.key === "Escape") {
      event.preventDefault();
      if (confirming) { setConfirming(null); text("info", t("shell.cancelled")); }
      else if (input) { print({ kind: "input", prompt: prompt(tab.session), text: `${input}^C` }); setInput(""); }
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
  // fish-style suggestion: the newest single-line history command that extends the input.
  const ghost = !search && !confirming && !menu && input.trim() && !input.includes("\n")
    ? [...(tab.session.history ?? [])].reverse().find((h) => h.command.length > input.length && h.command.startsWith(input) && !h.command.includes("\n"))?.command.slice(input.length)
    : undefined;
  const current = search ? t("shell.reverseSearch") : confirming ? t("shell.confirmPrompt", { name: confirming.env?.name }) : prompt(session);
  return <div className="term" style={{ fontSize }} onKeyDown={onTermKey} onMouseUp={(event) => {
    if (!window.getSelection()?.toString() && !(event.target as HTMLElement).closest(".term-find, .term-scroll")) field.current?.focus();
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
        <CellActions.Provider value={{ follow: (link, inOdoo) => void follow(link, inOdoo), inspect, exportTable: (table, format) => void exportTable(table, format) }}>
          {groups(visible(session.entries)).map((group) => <div key={group[0].entry.id} className="term-group">
            {group.map(({ entry, hidden }) => <Line key={entry.id} entry={entry} hidden={hidden} onToggle={() => update((s) => ({
              ...s, entries: s.entries.map((e) => e.id === entry.id && e.kind === "table" ? { ...e, expanded: !e.expanded } : e),
            }))} />)}
          </div>)}
        </CellActions.Provider>
      </SessionActions.Provider>
      {busy && <Running since={started} />}
    </div>
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
    <label className={`term-input ${confirming ? "confirm" : ""}`}>
      <span className="term-prompt">{current}</span>
      <span className="term-editor">
        {!search && !confirming && <div className="term-highlight" ref={editorHighlight} aria-hidden="true">{highlight(input, Object.keys(aliases))}{ghost && <span className="term-ghost">{ghost}</span>}</div>}
        <textarea ref={field} value={input} rows={Math.min(8, input.split("\n").length)} wrap="off" spellCheck={false} autoComplete="off" autoCorrect="off" aria-invalid={hint ? true : undefined} aria-label={t("shell.command")} aria-description={search ? t("shell.reverseSearchDescription") : undefined}
          className={search || confirming ? "" : "highlighted"}
          onChange={(e) => { setInput(e.target.value); setCursor(null); if (search) setSearch({ query: e.target.value, skip: 0 }); else refilter(e.target.value); }}
          onScroll={syncScroll} onKeyUp={syncScroll} onSelect={syncScroll}
          onBlur={() => setMenu(null)} onKeyDown={onKey} />
      </span>
      {hint && !menu && <span className="term-hint" role="status">{hint}</span>}
      {saveFailed && !hint && <span className="term-hint" role="status">{t("shell.sessionsNotSaved")}</span>}
    </label>
    {search && <div className="term-search" role="status">{match ? <><span className="term-search-match">{highlight(match.command, Object.keys(aliases))}</span><span className="term-search-meta">{match.environment ? `${match.environment} · ` : ""}{formatDateTime(new Date(match.at), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" })} · {t("shell.searchHelp")}</span></> : <span className="term-search-meta">{t("shell.noHistoryMatches")}</span>}</div>}
  </div>;
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

const SessionActions = createContext<{ busy: boolean; rerun: (command: string) => void; fold: (id: number) => void; aliases: string[] } | null>(null);

function Line({ entry, hidden, onToggle }: { entry: Entry; hidden?: number; onToggle: () => void }): ReactNode {
  const { t } = useTranslation();
  const actions = useContext(SessionActions);
  if (entry.kind === "input") return <div className="term-line term-cmd"><span className="term-prompt">{entry.prompt}</span> {highlight(entry.text, actions?.aliases)}
    {hidden !== undefined && <button type="button" className="term-folded" onClick={() => actions?.fold(entry.id)}>{t("shell.linesHidden", { count: hidden })}</button>}
    {entry.command && actions && <span className="term-cmd-actions">
      <button type="button" className="icon-btn sm" aria-label={t("shell.rerun")} data-tip={t("shell.rerun")} disabled={actions.busy} onClick={() => actions.rerun(entry.text)}><Icon name="refresh" /></button>
      <CopyButton text={entry.text} className="icon-btn sm" tip={t("shell.copyCommand")} ariaLabel={t("shell.copyCommand")} iconOnly />
      <button type="button" className="icon-btn sm" aria-label={t(entry.folded ? "shell.unfold" : "shell.fold")} data-tip={t(entry.folded ? "shell.unfold" : "shell.fold")} aria-expanded={!entry.folded} onClick={() => actions.fold(entry.id)}><Icon name={entry.folded ? "chevron" : "caretDown"} /></button>
    </span>}
  </div>;
  if (entry.kind !== "table") return <div className={`term-line ${entry.kind}`}>{entry.connectionError
    ? t("shell.connectionError", { name: entry.connectionError.name, reason: t(entry.connectionError.reason === "credential" ? "shell.credentialRejected" : "shell.offline"), error: entry.connectionError.detail ? ` (${formatAppError(entry.connectionError.detail)})` : "" })
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

function TableBlock({ table, expanded, footer, onToggle }: { table: Table; expanded: boolean; footer?: string; onToggle: () => void }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const aliases = useContext(SessionActions)?.aliases;
  const [sort, setSort] = useState<{ column: number; desc: boolean } | null>(null);
  const [active, setActive] = useState<{ r: number; c: number } | null>(null);
  const [copied, setCopied] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const rows = sort === null ? table.rows : [...table.rows].sort((a, b) => {
    const [x, y] = [a[sort.column], b[sort.column]];
    if (empty(x) || empty(y)) return Number(empty(x)) - Number(empty(y));
    return sort.desc ? compareCells(y, x) : compareCells(x, y);
  });
  const record = (row: unknown[]) => Object.fromEntries(table.columns.map((c, j) => [c, row[j]]));
  // Ascending, descending, then back to the original order.
  const cycle = (column: number) => setSort(sort?.column !== column ? { column, desc: false } : sort.desc ? null : { column, desc: true });
  // The id column stays visible while scrolling sideways.
  const pinned = table.columns[0] === "id" || table.columns[0] === "#";
  useLayoutEffect(() => {
    if (active) scroller.current?.querySelector(`[data-cell="${active.r}:${active.c}"]`)?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [active]);

  function onKey(event: KeyboardEvent<HTMLDivElement>) {
    if (expanded || !rows.length || event.altKey) return;
    const at = active ?? { r: 0, c: 0 };
    const last = { r: rows.length - 1, c: table.columns.length - 1 };
    const move = (r: number, c: number) => { event.preventDefault(); setActive({ r: Math.max(0, Math.min(last.r, r)), c: Math.max(0, Math.min(last.c, c)) }); };
    const ctrl = event.ctrlKey || event.metaKey;
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
        if (button) button.dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: ctrl }));
        else actions?.inspect(`${table.model ? `${table.model} · ` : ""}${table.columns[at.c]}`, rows[at.r][at.c]);
        return;
      }
    }
    if ((event.key.toLowerCase() === "c" && !event.shiftKey) && (ctrl ? !window.getSelection()?.toString() : true)) {
      event.preventDefault();
      void navigator.clipboard.writeText(cellText(rows[at.r][at.c])).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1200); }).catch(() => undefined);
    }
  }

  return <div className="term-block">
    <div className="term-scroll" ref={scroller} tabIndex={expanded ? undefined : 0} data-pinned={pinned || undefined}
      aria-label={t("shell.tableNavigation")} onKeyDown={onKey}
      onFocus={() => { if (!active) setActive({ r: 0, c: 0 }); }}
      onMouseDown={(event) => {
        const cell = (event.target as HTMLElement).closest<HTMLElement>("[data-cell]")?.dataset.cell;
        if (cell) { const [r, c] = cell.split(":").map(Number); setActive({ r, c }); }
      }}>
      {expanded ? rows.map((row, i) => <table key={i} className="term-table expanded"><tbody>
        <tr><th colSpan={2}>{t("shell.record", { index: i + 1 })}</th></tr>
        {table.columns.map((c, j) => <tr key={c}><th>{c}</th><Cell value={row[j]} column={c} model={table.model} row={record(row)} /></tr>)}
      </tbody></table>)
        : <table className="term-table"><thead><tr>{table.columns.map((c, j) => <th key={c} className={pinned && j === 0 ? "pin" : undefined} aria-sort={sort?.column === j ? (sort.desc ? "descending" : "ascending") : undefined}>
          <button type="button" className="term-sort" tabIndex={-1} aria-label={t("shell.sortBy", { column: c })} onClick={() => cycle(j)}>
            {c}<Icon name={sort?.column === j && sort.desc ? "caretDown" : "caretUp"} className={sort?.column === j ? "on" : undefined} />
          </button>
        </th>)}</tr></thead>
          <tbody>{rows.map((row, i) => <tr key={i}>{row.map((v, j) => {
            const pos = `${i}:${j}`;
            const on = active?.r === i && active.c === j;
            return table.code?.includes(j)
              ? <td key={j} data-cell={pos} data-active={on || undefined}>{highlight(String(v), aliases)}</td>
              : <Cell key={j} value={v} column={table.columns[j]} model={table.model} row={record(row)} pos={pos} active={on} pin={pinned && j === 0} />;
          })}</tr>)}</tbody></table>}
    </div>
    <div className="term-foot">
      <span className="term-foot-info">{footer ? `(${footer})` : ""}{copied ? ` · ${t("common.copied")}` : ""}</span>
      <span className="term-foot-actions">
        <button type="button" className="term-copy" aria-label={t(expanded ? "shell.collapseTable" : "shell.expandTable")} data-tip={t(expanded ? "shell.collapseTable" : "shell.expandTable")}
          aria-pressed={expanded} onClick={onToggle}><Icon name={expanded ? "table" : "rows"} /></button>
        <CopyTable table={{ ...table, rows }} />
      </span>
    </div>
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

const toTsv = (table: Table) => [table.columns, ...table.rows.map((row) => row.map(cellText))]
  .map((cells) => cells.map((c) => String(c).replace(/[\t\n\r]/g, " ")).join("\t"))
  .join("\n");
/** Raw values keyed by column, as Odoo returned them. */
const toJson = (table: Table) => JSON.stringify(table.rows.map((row) => Object.fromEntries(table.columns.map((c, j) => [c, row[j] ?? null]))), null, 2);
const toMarkdown = (table: Table) => {
  const line = (cells: string[]) => `| ${cells.map((c) => c.replace(/\|/g, "\\|").replace(/[\n\r]+/g, " ")).join(" | ")} |`;
  return [line(table.columns), line(table.columns.map(() => "---")), ...table.rows.map((row) => line(row.map(cellText)))].join("\n");
};
/** RFC 4180 with a BOM so spreadsheets detect UTF-8. Odoo data can come from outside (web forms), so values
 * that a spreadsheet would run as a formula get a leading apostrophe. */
const csvCell = (value: string) => {
  const safe = /^[=+\-@\t\r]/.test(value) ? `'${value}` : value;
  return /[",\n\r]/.test(safe) ? `"${safe.replace(/"/g, '""')}"` : safe;
};
const toCsv = (table: Table) => "\uFEFF" + [table.columns, ...table.rows.map((row) => row.map(cellText))]
  .map((cells) => cells.map(csvCell).join(","))
  .join("\r\n");

/** TSV for spreadsheets one click away; other formats behind a small menu. */
function CopyTable({ table }: { table: Table }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const menu = useRef<HTMLSpanElement>(null);
  // Close on outside clicks, not on blur: WebKit does not focus clicked buttons, and the
  // terminal refocuses its editor on mouseup, so a blur would unmount the menu before the click.
  useEffect(() => {
    if (!open) return;
    const outside = (event: MouseEvent) => { if (!menu.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", outside);
    return () => document.removeEventListener("mousedown", outside);
  }, [open]);
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
        {actions && <>
          <button type="button" role="menuitem" onClick={() => { setOpen(false); actions.exportTable(table, "csv"); }}>{t("shell.saveAs", { format: "CSV" })}</button>
          <button type="button" role="menuitem" onClick={() => { setOpen(false); actions.exportTable(table, "json"); }}>{t("shell.saveAs", { format: "JSON" })}</button>
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
