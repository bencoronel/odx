import { useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { CellActions, TableBlock, cellText, rowsTable, toCsv, toJson, toMarkdown, toTsv, type Table } from "./ShellPage";
import type { Field } from "./shell/complete";
import { ipc } from "./ipc";
import { TooltipLayer } from "./TooltipLayer";
import { WindowTitleBar } from "./WindowTitleBar";
import SearchField from "./SearchField";
import { Icon } from "./Icon";
import { useDismiss } from "./ContextMenu";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";

/** `envId` and `context` let the window repeat its read to add a column; it can do nothing else with them. */
type Payload = { table: Table; footer?: string; fields?: Field[]; title?: string; envId?: string; context?: Record<string, unknown> };
// Taken once per window (the app hands each window its own table, once), outside React so StrictMode cannot ask twice.
const taken = invoke<Payload | null>("table_window_take").catch(() => null);
const copies = { TSV: toTsv, JSON: toJson, Markdown: toMarkdown };
const saves = { CSV: toCsv, JSON: toJson };
/** Reads whose `fields` can grow by one column and run again as they were. */
const FETCHABLE = ["search_read", "read"];

/** A Terminal table in a window of its own, with a window's controls: filter, columns, table or records, copy and save.
 * Sorting, field labels and totals come with the table; nothing links back to the session it came from. */
export default function TableWindow() {
  const { t } = useTranslation();
  const [payload, setPayload] = useState<Payload | null | undefined>(undefined);
  const [data, setData] = useState<Table | null>(null);
  const [hidden, setHidden] = useState<string[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [query, setQuery] = useState("");
  const [note, setNote] = useState<{ text: string; error?: boolean } | null>(null);
  useLayoutEffect(() => {
    void taken.then((taken) => { setPayload(taken); setData(taken?.table ?? null); });
    // Same theme as the main window's preference.
    let stored: string | null = null;
    try { stored = localStorage.getItem("odx.theme"); } catch { /* Preference only. */ }
    const theme = stored === "light" || stored === "dark" ? stored : window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
  }, []);
  if (payload === undefined) return null;
  const q = query.trim().toLowerCase();
  const shownColumns = data ? data.columns.flatMap((column, j) => hidden.includes(column) ? [] : [j]) : [];
  const table = data && {
    ...data,
    columns: shownColumns.map((j) => data.columns[j]),
    rows: data.rows.filter((row) => !q || row.some((value) => cellText(value).toLowerCase().includes(q))).map((row) => shownColumns.map((j) => row[j])),
  };
  const say = (text: string, error = false) => { setNote({ text, error }); setTimeout(() => setNote(null), 4000); };
  // A new column repeats the same read with that field added; the rows stay the ones shown.
  async function addColumn(name: string) {
    const call = data?.call;
    if (!data || !call || !payload?.envId) return;
    const fields = [...call.kwargs.fields as string[], name];
    const next = { ...call, kwargs: { ...call.kwargs, fields } };
    try {
      const result = await ipc.shell({ environment_id: payload.envId, ...next, context: payload.context ?? {}, confirmed: false });
      setData({ ...rowsTable((result.data as Record<string, unknown>[]).slice(0, data.rows.length)), model: data.model, call: next });
    } catch (reason) { say(formatAppError(normalizeAppError(reason) as ErrorState), true); }
  }
  const fetchable = !!data?.call && FETCHABLE.includes(data.call.method) && Array.isArray(data.call.kwargs.fields) && !!payload?.envId;
  return <div className="app-window">
    <TooltipLayer />
    <WindowTitleBar title={payload?.title} />
    {payload && data && table ? <>
      <div className="table-window-bar">
        <SearchField value={query} onChange={setQuery} label={t("tableWindow.filter")} placeholder={t("tableWindow.filter")}
          filtered={!!q} count={{ shown: table.rows.length, total: data.rows.length, text: t("shell.filtered", { shown: table.rows.length, count: data.rows.length }) }} />
        {note && <span className={`hint ${note.error ? "text-danger" : "text-ok"}`} role={note.error ? "alert" : "status"}>{note.text}</span>}
        <span className="row gap-2 ml-auto">
          {!q && <span className="hint">{t("shell.row", { count: table.rows.length })}</span>}
          <div className="segmented" role="radiogroup" aria-label={t("tableWindow.view")}>
            <button type="button" role="radio" aria-checked={!expanded} aria-pressed={!expanded} onClick={() => setExpanded(false)}><Icon name="table" />{t("tableWindow.table")}</button>
            <button type="button" role="radio" aria-checked={expanded} aria-pressed={expanded} onClick={() => setExpanded(true)}><Icon name="rows" />{t("tableWindow.records")}</button>
          </div>
          <Actions table={table} say={say} />
        </span>
      </div>
      <div className="term table-window">
        <CellActions.Provider value={{ fields: () => payload.fields }}>
          <div className="term-out">
            <TableBlock table={table} expanded={expanded} onToggle={() => setExpanded(!expanded)} bare
              headerEnd={<ColumnPicker columns={data.columns} hidden={hidden} fields={fetchable ? payload.fields ?? [] : []}
                toggle={(name) => data.columns.includes(name) ? setHidden(hidden.includes(name) ? hidden.filter((c) => c !== name) : [...hidden, name]) : void addColumn(name)} />} />
          </div>
        </CellActions.Provider>
      </div>
    </> : <p className="empty">{t("shell.tableGone")}</p>}
  </div>;
}

/** Every column the table has, plus (when the read can be repeated) every other field of the model, ticked when shown. */
function ColumnPicker({ columns, hidden, fields, toggle }: { columns: string[]; hidden: string[]; fields: Field[]; toggle: (name: string) => void }) {
  const { t } = useTranslation();
  // Fixed position: the table scrolls and would clip a panel inside it.
  const [open, setOpen] = useState<{ top: number; right: number } | null>(null);
  const [query, setQuery] = useState("");
  const root = useRef<HTMLDivElement>(null);
  useDismiss(!!open, () => root.current, () => setOpen(null));
  const label = (name: string) => fields.find((f) => f.name === name)?.label;
  const names = [...columns, ...fields.map((f) => f.name).filter((name) => !columns.includes(name)).sort()];
  const q = query.trim().toLowerCase();
  const listed = names.filter((name) => !q || name.toLowerCase().includes(q) || (label(name) ?? "").toLowerCase().includes(q));
  return <div className="table-columns" ref={root} onKeyDown={(e) => { if (e.key === "Escape" && open) { e.stopPropagation(); setOpen(null); } }}>
    <button type="button" className="icon-btn sm" aria-haspopup="dialog" aria-expanded={!!open} aria-label={t("tableWindow.columns")} data-tip={open ? undefined : t("tableWindow.columns")}
      onClick={(event) => {
        const rect = event.currentTarget.getBoundingClientRect();
        setOpen(open ? null : { top: rect.bottom + 4, right: window.innerWidth - rect.right });
      }}><Icon name="eye" /></button>
    {open && <div className="filter-panel" role="dialog" aria-label={t("tableWindow.columns")} style={open}>
      <label className="search"><Icon name="search" /><input autoFocus type="search" value={query} placeholder={t("tableWindow.findColumn")} aria-label={t("tableWindow.findColumn")} onChange={(e) => setQuery(e.target.value)} /></label>
      <ul className="filter-options">{listed.map((name) => <li key={name}><label className="filter-option">
        <input type="checkbox" checked={columns.includes(name) && !hidden.includes(name)} onChange={() => toggle(name)} />
        <span className="truncate mono">{name}</span>{label(name) && <span className="filter-count truncate">{label(name)}</span>}
      </label></li>)}</ul>
    </div>}
  </div>;
}

/** The ⋯ menu: copy the rows shown, or save them to Downloads. */
function Actions({ table, say }: { table: Table; say: (text: string, error?: boolean) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  useDismiss(open, () => root.current, () => setOpen(false));
  const copy = (format: keyof typeof copies) => {
    setOpen(false);
    void navigator.clipboard.writeText(copies[format](table)).then(() => say(t("shell.copiedAs", { format })), () => undefined);
  };
  const save = (format: keyof typeof saves) => {
    setOpen(false);
    const stamp = new Date().toISOString().slice(0, 19).replace(/[-:]/g, "").replace("T", "-");
    void ipc.shellExport(`odx-${table.model ?? "table"}-${stamp}.${format.toLowerCase()}`, saves[format](table))
      .then((path) => say(t("shell.exported", { path })), (reason) => say(formatAppError(normalizeAppError(reason)), true));
  };
  return <span className="more-menu" ref={root} onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); setOpen(false); } }}>
    <button type="button" className="btn sm icon-only" aria-haspopup="menu" aria-expanded={open} aria-label={t("environments.moreActions")} data-tip={open ? undefined : t("environments.moreActions")} onClick={() => setOpen(!open)}>⋯</button>
    {open && <div className="more-menu-list" role="menu" aria-label={t("environments.moreActions")}>
      {(Object.keys(copies) as (keyof typeof copies)[]).map((format) => <button key={format} type="button" role="menuitem" onClick={() => copy(format)}><Icon name="copy" />{t("shell.copyAs", { format })}</button>)}
      {(Object.keys(saves) as (keyof typeof saves)[]).map((format) => <button key={format} type="button" role="menuitem" onClick={() => save(format)}><Icon name="external" />{t("shell.saveAs", { format })}</button>)}
    </div>}
  </span>;
}
