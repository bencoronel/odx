import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { ipc, type Environment, type HistoryEntry, type ShellError } from "./ipc";
import { highlight } from "./shell/highlight";
import { CommandError, parse, searchCall, type Command, type OdooCall } from "./shell/commands";
import { completions, type Completion, type Field } from "./shell/complete";
import { modeLabels } from "./ModeSelect";
import { Icon } from "./Icon";
import CopyButton from "./CopyButton";
import { useTranslation } from "react-i18next";
import { formatDateTime, formatNumber } from "./i18n/format";
import { formatAppError, normalizeAppError, type AppError } from "./appError";

const MAX_LINES = 400;

/** Column indexes containing commands to highlight. */
type Table = { columns: string[]; rows: unknown[][]; code?: number[] };
type ConnectionErrorPresentation = { name: string; reason: "credential" | "offline"; detail?: AppError };
type Entry =
  | { id: number; kind: "input"; prompt: string; text: string }
  | { id: number; kind: "text" | "info" | "error" | "warn" | "ok"; text: string; translationKey?: "shell.initial" | "shell.help"; appError?: AppError; connectionError?: ConnectionErrorPresentation }
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
type ShellTab = { id: string; name: string; defaultName: boolean; session: Session; busy: boolean };
const newTab = (): ShellTab => ({ id: crypto.randomUUID(), name: "", defaultName: true, session: fresh(), busy: false });

function cellText(v: unknown): string {
  if (v === null || v === undefined) return "null";
  if (typeof v === "string") return v;
  if (Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string") return `${v[1]} (${v[0]})`;
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}
function Cell({ value }: { value: unknown }) {
  const text = cellText(value);
  const dim = value === false || value === null || value === undefined;
  const short = text.length > 80 ? `${text.slice(0, 80)}…` : text;
  return <td className={`${dim ? "dim" : ""} ${typeof value === "number" ? "num" : ""}`} data-tip={short !== text ? text : undefined}>{short}</td>;
}
function rowsTable(rows: Record<string, unknown>[]): Table {
  const columns: string[] = [];
  for (const row of rows) for (const key of Object.keys(row)) if (!columns.includes(key)) columns.push(key);
  return { columns, rows: rows.map((row) => columns.map((c) => row[c])) };
}
const size = (bytes: number) => bytes < 1024 ? `${formatNumber(bytes)} B` : `${formatNumber(bytes / 1024, { maximumFractionDigits: 1 })} KB`;

export default function ShellPage({ envs, connectTo, onConnectHandled, visible = true }: {
  envs: Environment[]; connectTo?: string | null; onConnectHandled?: () => void; visible?: boolean;
}) {
  const { t } = useTranslation();
  const [tabs, setTabs] = useState(() => [newTab()]);
  const [activeId, setActiveId] = useState(tabs[0].id);
  const [renaming, setRenaming] = useState<{ id: string; value: string; width: number } | null>(null);
  const refreshTabs = () => setTabs((current) => [...current]);
  const select = (id: string) => setActiveId(id);
  const add = () => {
    const tab = newTab();
    setTabs((current) => [...current, tab]);
    select(tab.id);
  };
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
      <ShellSession tab={tab} envs={envs} active={visible && activeId === tab.id} onChange={refreshTabs}
        connectTo={activeId === tab.id ? connectTo : null} onConnectHandled={onConnectHandled} />
    </div>)}
  </div>;
}

function ShellSession({ tab, envs, active, onChange, connectTo, onConnectHandled }: {
  tab: ShellTab; envs: Environment[]; active: boolean; onChange: () => void;
  connectTo?: string | null; onConnectHandled?: () => void;
}) {
  const { t } = useTranslation();
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [session, setSession] = useState<Session>(tab.session);
  const [input, setInput] = useState("");
  const [cursor, setCursor] = useState<number | null>(null);
  const [busy, setBusyState] = useState(false);
  const setBusy = (value: boolean) => { if (!mounted.current) return; tab.busy = value; setBusyState(value); onChange(); };
  const [confirming, setConfirming] = useState<Pending | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const menuList = useRef<HTMLUListElement>(null);
  const out = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);

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
  const syncScroll = () => { if (editorHighlight.current && field.current) editorHighlight.current.scrollLeft = field.current.scrollLeft; };

  const handled = useRef<string | null>(null);
  useEffect(() => {
    if (!connectTo) { handled.current = null; return; }
    if (busy) return;
    if (handled.current === connectTo) return; // One attempt per request.
    handled.current = connectTo;
    onConnectHandled?.();
    if (confirming) { setConfirming(null); text("info", t("shell.cancelled")); }
    if (tab.session.env?.name !== connectTo) {
      print({ kind: "input", prompt: prompt(tab.session), text: `\\c ${connectTo}` });
      void connect(connectTo);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectTo, busy]);
  const print = (...entries: NewEntry[]) => update((s) => ({ ...s, entries: [...s.entries, ...entries.map((e) => ({ ...e, id: nextId++ }) as Entry)].slice(-MAX_LINES) }));
  const text = (kind: "text" | "info" | "error" | "warn" | "ok", t: string) => print({ kind, text: t });
  const prompt = (s: Session) => s.env ? `odoo@${s.env.name}>` : "odoo>";

  async function run(pending: Pending, confirmed: boolean) {
    const env = pending.env ?? tab.session.env;
    if (!env) return text("error", t("shell.noConnection"));
    setBusy(true);
    try {
      const result = await ipc.shell({ environment_id: env.id, ...pending.call, context: tab.session.context, confirmed });
      render(pending, result.data, tab.session.timing ? `${result.upstream_ms} ms · ${size(result.upstream_bytes)}` : "");
      // \next repeats search_read pages only; name_search has no offset.
      if (pending.page && pending.call.method === "search_read") update((s) => ({ ...s, last: pending }));
    } catch (reason) {
      const e = reason as ShellError;
      // The core classifies writes and requires confirmation before sending them.
      if (e?.code === "confirmation_required" && !confirmed) {
        const c = pending.call;
        print({ kind: "warn", text: `${c.method} ${c.model} ${JSON.stringify(c.args)}${Object.keys(c.kwargs).length ? ` ${JSON.stringify(c.kwargs)}` : ""}` });
        setConfirming({ ...pending, env });
      } else if (e?.code === "uncertain_result") print({ kind: "warn", text: "", appError: normalizeAppError(reason) });
      else print({ kind: "error", text: "", appError: normalizeAppError(reason) });
    } finally { setBusy(false); }
  }

  function render(p: Pending, data: unknown, meta: string) {
    const expanded = tab.session.expanded;
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
      return print({ kind: "table", expanded, table: { columns: ["id", t("shell.name")], rows }, footer: t("shell.count", { count: rows.length, item: t("shell.results") }) + m + (more ? t("shell.moreRefine") : "") });
    }
    if (Array.isArray(data) && data.length && data.every((r) => r && typeof r === "object" && !Array.isArray(r))) {
      let rows = data as Record<string, unknown>[];
      let more = false;
      if (p.page) { more = rows.length > p.page.limit; rows = rows.slice(0, p.page.limit); }
      const where = p.page ? t("shell.fromOffset", { offset: p.page.offset }) : "";
      return print({ kind: "table", expanded, table: rowsTable(rows), footer: t("shell.row", { count: rows.length }) + where + m + (more ? t("shell.moreNext") : "") });
    }
    if (Array.isArray(data) && !data.length) return text("info", `${t("shell.noResults")}${m}`);
    print({ kind: "text", text: typeof data === "object" ? JSON.stringify(data, null, 2) : String(data) });
    if (meta) text("info", meta);
  }

  async function connect(name: string) {
    const lower = name.toLowerCase();
    const exact = envs.filter((e) => e.name.toLowerCase() === lower);
    const matches = exact.length ? exact : envs.filter((e) => e.name.toLowerCase().startsWith(lower));
    if (matches.length !== 1) return text("error", matches.length ? t("shell.ambiguousEnvironment", { names: matches.map((e) => e.name).join(", ") }) : t("shell.unknownEnvironment", { name }));
    const env = matches[0];
    setBusy(true);
    try {
      const health = await ipc.check(env.id);
      if (health.status !== "ok") return print({ kind: "error", text: "", connectionError: {
        name: env.name,
        reason: health.status === "auth_failed" ? "credential" : "offline",
        detail: health.error_code ? { code: health.error_code, params: health.error_params ?? undefined, detail: health.error ?? undefined } : undefined,
      } });
      update((s) => ({ ...s, env: { id: env.id, name: env.name }, last: undefined }));
      text("ok", t("shell.connected", { name: env.name, database: env.database, version: health.server_version ? ` · Odoo ${health.server_version}` : "", latency: health.latency_ms }));
      if (!tab.session.models[env.id]) {
        void ipc.shell({ environment_id: env.id, model: "ir.model", method: "search_read", args: [], kwargs: { fields: ["model"], limit: 5000 } })
          .then((r) => update((s) => ({ ...s, models: { ...s.models, [env.id]: (r.data as { model: string }[]).map((m) => m.model).sort() } })))
          .catch(() => undefined);
      }
    } catch (reason) { print({ kind: "error", text: "", appError: normalizeAppError(reason) }); } finally { setBusy(false); }
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
    print({ kind: "input", prompt: prompt(tab.session), text: line });
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
    try { command = parse(line); } catch (e) { return text("error", e instanceof CommandError ? e.message : String(e)); }
    if (!command) return;
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
        const kw = last.call.kwargs as { domain: unknown[]; fields: string[]; order?: string };
        return run({ ...last, page: { ...last.page, offset }, call: searchCall(last.call.model, kw.domain, kw.fields, last.page.limit, offset, kw.order) }, false);
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
      const r = await ipc.shell({ environment_id: env.id, model: needs.fields, method: "fields_get", args: [], kwargs: { attributes: ["string", "type"] } });
      const fields = Object.entries(r.data as Record<string, { string: string; type: string }>)
        .map(([name, f]) => ({ name, label: f.string, type: f.type }))
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

  function onKey(event: KeyboardEvent<HTMLInputElement>) {
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
    if (event.key === "ArrowUp" || event.key === "ArrowDown") {
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
  const current = search ? t("shell.reverseSearch") : confirming ? t("shell.confirmPrompt", { name: confirming.env?.name }) : prompt(session);
  return <div className="term" onMouseUp={() => { if (!window.getSelection()?.toString()) field.current?.focus(); }}>
    <div className="term-out" ref={out} role="log" aria-live="polite" aria-label={t("shell.output")}>
      {session.entries.map((entry) => <Line key={entry.id} entry={entry} />)}
      {busy && <div className="term-line info">…</div>}
    </div>
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
        {!search && !confirming && <div className="term-highlight" ref={editorHighlight} aria-hidden="true">{highlight(input)}</div>}
        <input ref={field} value={input} disabled={busy} spellCheck={false} autoComplete="off" aria-label={t("shell.command")} aria-description={search ? t("shell.reverseSearchDescription") : undefined}
          className={search || confirming ? "" : "highlighted"}
          onChange={(e) => { setInput(e.target.value); setCursor(null); if (search) setSearch({ query: e.target.value, skip: 0 }); else refilter(e.target.value); }}
          onScroll={syncScroll} onKeyUp={syncScroll} onSelect={syncScroll}
          onBlur={() => setMenu(null)} onKeyDown={onKey} />
      </span>
    </label>
    {search && <div className="term-search" role="status">{match ? <><span className="term-search-match">{highlight(match.command)}</span><span className="term-search-meta">{match.environment ? `${match.environment} · ` : ""}{formatDateTime(new Date(match.at), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" })} · {t("shell.searchHelp")}</span></> : <span className="term-search-meta">{t("shell.noHistoryMatches")}</span>}</div>}
  </div>;
}

function Line({ entry }: { entry: Entry }): ReactNode {
  const { t } = useTranslation();
  if (entry.kind === "input") return <div className="term-line"><span className="term-prompt">{entry.prompt}</span> {highlight(entry.text)}</div>;
  if (entry.kind !== "table") return <div className={`term-line ${entry.kind}`}>{entry.connectionError
    ? t("shell.connectionError", { name: entry.connectionError.name, reason: t(entry.connectionError.reason === "credential" ? "shell.credentialRejected" : "shell.offline"), error: entry.connectionError.detail ? ` (${formatAppError(entry.connectionError.detail)})` : "" })
    : entry.appError ? formatAppError(entry.appError) : entry.translationKey ? t(entry.translationKey) : entry.text}</div>;
  const { table, expanded, footer } = entry;
  return <div className="term-block">
    <CopyTable table={table} />
    {expanded ? table.rows.map((row, i) => <table key={i} className="term-table expanded"><tbody>
      <tr><th colSpan={2}>{t("shell.record", { index: i + 1 })}</th></tr>
      {table.columns.map((c, j) => <tr key={c}><th>{c}</th><Cell value={row[j]} /></tr>)}
    </tbody></table>)
      : <table className="term-table"><thead><tr>{table.columns.map((c) => <th key={c}>{c}</th>)}</tr></thead>
        <tbody>{table.rows.map((row, i) => <tr key={i}>{row.map((v, j) => table.code?.includes(j) ? <td key={j}>{highlight(String(v))}</td> : <Cell key={j} value={v} />)}</tr>)}</tbody></table>}
    {footer && <div className="term-line info">({footer})</div>}
  </div>;
}

/** Copy headers and rows as TSV for spreadsheets. */
function CopyTable({ table }: { table: Table }) {
  const { t } = useTranslation();
  const tsv = () => [table.columns, ...table.rows.map((row) => row.map(cellText))]
    .map((cells) => cells.map((c) => String(c).replace(/[\t\n\r]/g, " ")).join("\t"))
    .join("\n");
  return <CopyButton text={tsv} className="term-copy" tip={t("shell.copyTsvTip")} ariaLabel={t("shell.copyTsvLabel")} label="TSV" />;
}
