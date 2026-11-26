import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { ipc, type AlertCondition, type Environment, type SavedView, type SavedViewDraft, type ViewRunResult } from "./ipc";
import { Icon } from "./Icon";
import Select from "./Select";
import SearchField from "./SearchField";
import HeaderInput from "./HeaderInput";
import { ListResizer, useListWidth } from "./ListResizer";
import { ConfirmDialog } from "./ConfirmDialog";
import { StageBadge, stageShortLabel } from "./StageBadge";
import { useDismiss } from "./ContextMenu";
import { MoreMenu } from "./Menu";
import { FilterGroupEditor, applyPlaceholderToTree, createEmptyRoot, domainToTree, formatDomainReadable, treeToDomain, type FilterGroup } from "./ViewFilterBuilder";
import { rowsTable, toCsv, toExcelCsv, toJson, exportName } from "./table";
import { shellCommand } from "./shell/commands";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { relativeTime, useNow } from "./time";
import { modalOpen } from "./modal";
import { Banner, Button, Chip, IconButton, PanelHeader, Props, Segment, Segmented, StatusDot, cx, detail, detailHeader, detailPane, empty, emptyIcon, envList, envListText, field, filterOption, filterOptions, listEmpty, listEmptyIcon, listPane, listToolbar, method, panel, panelBody, searchIcon, searchInput, split, tile, valuesTable, valuesTables } from "./ui";

type Template = {
  id: string;
  nameKey: string;
  descKey: string;
  model: string;
  domain: unknown[];
  fields: string[];
  order: string;
  limit: number;
};

const TEMPLATES: Template[] = [
  {
    id: "failed-emails",
    nameKey: "views.templates.failedEmails",
    descKey: "views.templates.failedEmailsDesc",
    model: "mail.mail",
    domain: [["state", "=", "exception"], ["date", ">=", "{{days_ago:7}}"]],
    fields: ["id", "subject", "email_to", "state", "date"],
    order: "date desc",
    limit: 50,
  },
  {
    id: "overdue-crons",
    nameKey: "views.templates.overdueCrons",
    descKey: "views.templates.overdueCronsDesc",
    model: "ir.cron",
    domain: [["active", "=", true], ["nextcall", "<", "{{hours_ago:1}}"]],
    fields: ["id", "name", "nextcall", "interval_number", "interval_type"],
    order: "nextcall asc",
    limit: 50,
  },
  {
    id: "old-draft-invoices",
    nameKey: "views.templates.draftInvoices",
    descKey: "views.templates.draftInvoicesDesc",
    model: "account.move",
    domain: [["state", "=", "draft"], ["move_type", "in", ["out_invoice", "out_refund"]], ["create_date", "<=", "{{days_ago:7}}"]],
    fields: ["id", "name", "partner_id", "amount_total", "create_date"],
    order: "create_date asc",
    limit: 50,
  },
  {
    id: "pending-modules",
    nameKey: "views.templates.pendingModules",
    descKey: "views.templates.pendingModulesDesc",
    model: "ir.module.module",
    domain: [["state", "in", ["to upgrade", "to install", "to remove"]]],
    fields: ["id", "name", "shortdesc", "state", "installed_version"],
    order: "name asc",
    limit: 100,
  },
];

const INTERVALS = [5, 15, 30, 60, 360, 1440] as const;
const PLACEHOLDERS = ["{{today}}", "{{now}}", "{{days_ago:7}}", "{{hours_ago:24}}"] as const;

function formatInterval(minutes: number, t: (k: string, o?: Record<string, unknown>) => string) {
  if (minutes < 60) return t("views.intervals.minutes", { count: minutes });
  if (minutes < 1440) return t("views.intervals.hours", { count: Math.round(minutes / 60) });
  return t("views.intervals.days", { count: Math.round(minutes / 1440) });
}

function formatCell(value: unknown): string {
  if (value === null || value === undefined || value === false) return "—";
  if (value === true) return "✓";
  if (Array.isArray(value) && value.length === 2 && typeof value[0] === "number" && typeof value[1] === "string") return value[1];
  if (typeof value === "object") return JSON.stringify(value);
  return String(value);
}

const LAST_ENV_KEY = "odx.views.lastEnv";
function readLastEnvs(): Record<string, string> {
  try { return JSON.parse(localStorage.getItem(LAST_ENV_KEY) ?? "{}") as Record<string, string>; } catch { return {}; }
}
function saveLastEnv(viewId: string, envId: string) {
  try { localStorage.setItem(LAST_ENV_KEY, JSON.stringify({ ...readLastEnvs(), [viewId]: envId })); } catch { /* optional */ }
}

const modelCache = new Map<string, string[]>();
const fieldCache = new Map<string, string[]>();

export default function ViewsPage({ envs, openInShell, initialDraft, onDraftConsumed }: { envs: Environment[]; openInShell?: (env: string, command: string) => void; initialDraft?: SavedViewDraft | null; onDraftConsumed?: () => void }) {
  const { t } = useTranslation();
  const [listWidth, setListWidth] = useListWidth("odx.views.listWidth");
  const [views, setViews] = useState<SavedView[] | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<ErrorState>("");
  const [editing, setEditing] = useState<SavedViewDraft | null>(null);
  const [deleting, setDeleting] = useState<SavedView | null>(null);
  const [busyDelete, setBusyDelete] = useState(false);

  useEffect(() => {
    if (!initialDraft) return;
    setEditing(initialDraft);
    onDraftConsumed?.();
  }, [initialDraft, onDraftConsumed]);
  const [runEnvId, setRunEnvId] = useState<string>(() => envs[0]?.id ?? "");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<ViewRunResult | null>(null);
  const [runError, setRunError] = useState<ErrorState>("");
  const [exportNote, setExportNote] = useState("");
  const search = useRef<HTMLInputElement>(null);
  const now = useNow(true, 30_000);

  useEffect(() => {
    if (!runEnvId && envs[0]) setRunEnvId(envs[0].id);
    else if (runEnvId && !envs.some((e) => e.id === runEnvId)) setRunEnvId(envs[0]?.id ?? "");
  }, [envs, runEnvId]);

  const load = () => ipc.views().then((all) => {
    setViews([...all].sort((a, b) => a.name.localeCompare(b.name)));
  }, (reason) => setError(normalizeAppError(reason)));

  useEffect(() => { void load(); }, []);
  useEffect(() => {
    const stop = listen("views-changed", () => { void load(); });
    return () => { void stop.then((unlisten) => unlisten(), () => undefined); };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.altKey || modalOpen()) return;
      const command = event.ctrlKey || event.metaKey;
      if (command && event.shiftKey) return;
      const key = event.key.toLowerCase();
      const typing = event.target instanceof Element && event.target.closest("input, textarea, select, [contenteditable]");
      if ((command && key === "f") || (!command && key === "/" && !typing)) {
        if (!search.current) return;
        event.preventDefault();
        search.current.focus();
        search.current.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const visible = useMemo(() => {
    const list = views ?? [];
    const q = filter.trim().toLowerCase();
    return q ? list.filter((v) => [v.name, v.model, v.description].some((s) => s.toLowerCase().includes(q))) : list;
  }, [views, filter]);

  const selected = views?.find((v) => v.id === selectedId) ?? visible[0];
  useEffect(() => { if (selected && selected.id !== selectedId) setSelectedId(selected.id); }, [selected, selectedId]);
  useEffect(() => {
    setResult(null);
    setRunError("");
    setExportNote("");
    if (selected) {
      const remembered = readLastEnvs()[selected.id];
      const preferred = (remembered && envs.some((e) => e.id === remembered) ? remembered : null)
        ?? selected.schedule?.environment_ids.find((id) => envs.some((e) => e.id === id))
        ?? envs[0]?.id ?? "";
      if (preferred) setRunEnvId(preferred);
    }
  }, [selected?.id, envs]);
  const chooseRunEnv = (envId: string) => {
    setRunEnvId(envId);
    if (selected) saveLastEnv(selected.id, envId);
  };

  const openBlank = () => setEditing({
    name: "", description: "", model: "", domain: [], fields: ["id", "display_name"], order: "", limit: 50,
    schedule: null,
  });

  const openTemplate = (tpl: Template) => setEditing({
    name: t(tpl.nameKey as never),
    description: t(tpl.descKey as never),
    model: tpl.model,
    domain: tpl.domain,
    fields: tpl.fields,
    order: tpl.order,
    limit: tpl.limit,
    schedule: null,
  });

  const openEdit = (view: SavedView) => setEditing({
    id: view.id,
    name: view.name,
    description: view.description,
    model: view.model,
    domain: view.domain,
    fields: view.fields,
    order: view.order ?? "",
    limit: view.limit,
    schedule: view.schedule ? {
      enabled: view.schedule.enabled,
      environment_ids: view.schedule.environment_ids,
      interval_minutes: view.schedule.interval_minutes,
      alert_on: view.schedule.alert_on,
    } : null,
  });

  async function runSelected() {
    if (!selected || !runEnvId || running) return;
    setRunning(true);
    setRunError("");
    setExportNote("");
    try {
      const res = await ipc.runView(selected.id, runEnvId);
      setResult(res);
    } catch (reason) {
      setRunError(normalizeAppError(reason));
    } finally {
      setRunning(false);
    }
  }

  async function exportResult(format: "csv" | "csv-excel" | "json") {
    if (!result || !selected) return;
    const table = { ...rowsTable(result.rows), model: selected.model };
    const ext = format === "json" ? "json" : "csv";
    const content = format === "csv" ? toCsv(table) : format === "csv-excel" ? toExcelCsv(table) : toJson(table);
    const name = exportName(table, ext);
    try {
      const savedPath = await ipc.shellExport(name, content);
      setExportNote(t("views.exported", { path: savedPath }));
    } catch (reason) {
      setRunError(normalizeAppError(reason));
    }
  }

  if (views && views.length === 0 && !editing) {
    return <div className={empty}>
      <span className={emptyIcon()}><Icon name="table" /></span>
      <h2>{t("views.emptyTitle")}</h2>
      <div className="flex items-center gap-2 mt-2">
        <Button variant="primary" onClick={openBlank}><Icon name="plus" />{t("views.create")}</Button>
      </div>
      <div className="flex flex-wrap items-center justify-center gap-1.5 mt-4 max-w-[520px]">
        {TEMPLATES.map((tpl) => <Button key={tpl.id} size="sm" variant="ghost" onClick={() => openTemplate(tpl)}>
          {t(tpl.nameKey as never)}
        </Button>)}
      </div>
    </div>;
  }

  const runEnvOptions = envs.map((e) => ({
    value: e.id,
    label: e.stage ? `[${stageShortLabel[e.stage]}] ${e.name}` : e.name,
  }));
  const resultColumns = result ? (selected?.fields.length ? selected.fields : Object.keys(result.rows[0] ?? {})) : [];

  return <div className={split} style={{ "--list-w": `${listWidth}px` } as CSSProperties}>
    <aside className={listPane} aria-label={t("views.listLabel")}>
      <ListResizer width={listWidth} setWidth={setListWidth} label={t("views.resizeList")} />
      <div className={listToolbar}>
        <div className="flex items-center gap-2">
          <SearchField className="grow min-w-0" inputRef={search} value={filter} onChange={setFilter} label={t("views.filterLabel")} placeholder={t("views.filterPlaceholder")} shortcut
            filtered={!!filter.trim()} onClear={() => setFilter("")} clearLabel={t("environments.clearFilters")}
            inputProps={{ "aria-keyshortcuts": "Control+F /" }} />
          <MoreMenu label={t("views.templatesTitle")} items={TEMPLATES.map((tpl) => ({ label: t(tpl.nameKey as never), icon: "table", run: () => openTemplate(tpl) }))} />
          <Button variant="primary" size="sm" iconOnly className="flex-none" onClick={openBlank} aria-label={t("views.create")} data-tip={t("views.create")}><Icon name="plus" /></Button>
        </div>
        {error && <Banner tone="danger" compact role="alert"><Icon name="alert" />{formatAppError(error)}</Banner>}
      </div>
      <ul className={envList} role="listbox" aria-label={t("views.listLabel")}>
        {visible.map((view) => {
          const sched = view.schedule;
          const active = !!sched?.enabled;
          const hasAlert = active && sched!.last_runs.some((r) => r.triggered || r.error);
          return <li key={view.id} role="option" aria-selected={!editing && view.id === selected?.id} tabIndex={view.id === selected?.id ? 0 : -1} onClick={() => { setEditing(null); setSelectedId(view.id); }}>
            <span className={envListText}>
              <span className="flex items-center gap-1.5 min-w-0">
                <strong className="truncate">{view.name}</strong>
                {active && <Chip tone={hasAlert ? "warn" : "info"} className="h-4! text-3xs! px-1!"><Icon name="clock" className="size-2.5!" />{formatInterval(sched!.interval_minutes, t as never)}</Chip>}
              </span>
              <small>{view.model}</small>
            </span>
            {active && <StatusDot className="ml-auto" kind="small" tone={hasAlert ? "warn" : "ok"} />}
          </li>;
        })}
        {views && visible.length === 0 && <li className={listEmpty}>
          <span className={listEmptyIcon}><Icon name="search" /></span>
          <strong>{t("views.noMatches")}</strong>
          <Button size="sm" className="mt-2" onClick={() => setFilter("")}><Icon name="x" />{t("environments.clearFilters")}</Button>
        </li>}
      </ul>
    </aside>

    <section className={detailPane}>
      {editing ? <ViewForm key={editing.id ?? `new-${editing.model}-${editing.name}`} initial={editing} envs={envs} envId={runEnvId || envs[0]?.id}
        onCancel={() => setEditing(null)}
        onSaved={(saved) => { setEditing(null); setSelectedId(saved.id); void load(); }} />
      : selected && <div className={cx(detail, "max-w-[1040px]!")}>
        <header className={detailHeader}>
          <div className="min-w-0 flex-[1_1_220px]">
            <h2 className="flex items-center gap-2 min-w-0">
              <span className="truncate">{selected.name}</span>
              <code className={method("info")}>{selected.model}</code>
            </h2>
            {selected.description && <p className="text-fg-3 text-sm mt-0.5">{selected.description}</p>}
          </div>
          <span className="flex items-center gap-2 flex-wrap">
            {envs.length > 0 && <Select className="w-[210px]!" label={t("common.environment")} value={runEnvId} options={runEnvOptions} onChange={chooseRunEnv} />}
            <Button variant="primary" size="sm" disabled={!runEnvId || running} onClick={() => void runSelected()}>
              <Icon name={running ? "refresh" : "arrow"} className={running ? "animate-spin" : undefined} />
              {running ? t("views.running") : t("views.run")}
            </Button>
            {openInShell && runEnvId && <IconButton size="sm" aria-label={t("environments.openShell")} data-tip={t("environments.openShell")} onClick={() => {
              const env = envs.find((e) => e.id === runEnvId);
              if (!env) return;
              const cmd = shellCommand({
                model: selected.model,
                method: "search_read",
                args: [selected.domain],
                kwargs: {
                  ...(selected.fields.length ? { fields: selected.fields } : {}),
                  ...(selected.order ? { order: selected.order } : {}),
                  limit: selected.limit,
                },
              });
              openInShell(env.name, cmd);
            }}><Icon name="terminal" /></IconButton>}
            <IconButton size="sm" aria-label={t("common.edit")} data-tip={t("common.edit")} onClick={() => openEdit(selected)}><Icon name="pencil" /></IconButton>
            <IconButton size="sm" aria-label={t("common.delete")} data-tip={t("common.delete")} onClick={() => setDeleting(selected)}><Icon name="trash" /></IconButton>
          </span>
        </header>

        <section className={panel}>
          <PanelHeader>
            <h3>{t("views.queryTitle")}</h3>
            <span className="text-xs text-fg-3 mono">{selected.order ? `${selected.order} · ` : ""}{t("views.limit")}: {selected.limit}</span>
          </PanelHeader>
          <Props compact>
            <div><dt>{t("views.fields")}</dt><dd className="mono">{selected.fields.length ? selected.fields.join(", ") : t("views.allDefaultFields")}</dd></div>
            <div><dt>{t("views.filters")}</dt><dd className="mono">{formatDomainReadable(selected.domain) || t("views.noFilters")}</dd></div>
          </Props>
        </section>

        <section className={panel}>
          <PanelHeader>
            <h3>{t("views.scheduleTitle")}</h3>
            <span className="flex items-center gap-2">
              <Chip tone={selected.schedule?.enabled ? "ok" : undefined}>{t(selected.schedule?.enabled ? "views.scheduleActive" : "views.scheduleOff")}</Chip>
              <Button size="sm" onClick={() => openEdit(selected)}><Icon name="clock" />{t("views.configureSchedule")}</Button>
            </span>
          </PanelHeader>
          {selected.schedule?.enabled && <div className={cx(panelBody, "grid gap-2.5")}>
            <div className="flex flex-wrap items-center gap-2 text-sm text-fg-2">
              <Chip tone="info"><Icon name="clock" />{formatInterval(selected.schedule.interval_minutes, t as never)}</Chip>
              <Chip>{selected.schedule.alert_on.kind === "any_rows" ? t("views.alerts.anyRows")
                : selected.schedule.alert_on.kind === "changed" ? t("views.alerts.changed")
                : t("views.alerts.countAbove", { threshold: selected.schedule.alert_on.threshold })}</Chip>
              {selected.schedule.last_run_at && <span className="text-fg-3 text-xs ml-auto">{t("views.lastRun", { time: relativeTime(selected.schedule.last_run_at, now) })}</span>}
            </div>
            <div className="flex flex-wrap items-center gap-1.5">
              {selected.schedule.environment_ids.map((id) => {
                const env = envs.find((e) => e.id === id);
                const run = selected.schedule!.last_runs.find((r) => r.environment_id === id);
                return <Chip key={id} tone={run?.error ? "danger" : run?.triggered ? "warn" : run ? "ok" : undefined}>
                  <StageBadge stage={env?.stage} compact />
                  <span>{env?.name ?? run?.environment_name ?? id}</span>
                  {run && <span className="mono text-2xs opacity-85">· {run.error ? t("service.error") : `${run.count}`}</span>}
                </Chip>;
              })}
            </div>
          </div>}
        </section>

        {runError && <Banner tone="danger" role="alert"><Icon name="alert" />{formatAppError(runError)}</Banner>}
        {exportNote && <Banner tone="ok" compact role="status"><Icon name="check" />{exportNote}</Banner>}

        {result && <section className={panel}>
          <PanelHeader>
            <h3>{t("views.resultsTitle", { shown: result.rows.length, total: result.total_count })} <span className="font-normal text-fg-3">· {result.environment_name} · {result.duration_ms} ms</span></h3>
            <span className="flex items-center gap-1.5">
              {result.rows.length > 0 && <>
                <Button size="sm" onClick={() => void ipc.openTableWindow({ table: { ...rowsTable(result.rows), model: selected.model }, title: `${selected.name} · ${result.environment_name}` }, `${selected.name} · ${result.environment_name}`).catch(() => undefined)}><Icon name="table" />{t("activity.openTable")}</Button>
                <Button size="sm" variant="ghost" onClick={() => void exportResult("csv")}>CSV</Button>
                <Button size="sm" variant="ghost" onClick={() => void exportResult("csv-excel")}>CSV (;)</Button>
                <Button size="sm" variant="ghost" onClick={() => void exportResult("json")}>JSON</Button>
              </>}
            </span>
          </PanelHeader>
          {result.rows.length === 0 ? <p className={cx(panelBody, "text-fg-3 text-sm")}>{t("views.noRows")}</p>
            : <div className={cx(valuesTables, "overflow-x-auto")}>
              <table className={valuesTable}>
                <thead><tr>{resultColumns.map((col) => <th key={col} scope="col" className="mono">{col}</th>)}</tr></thead>
                <tbody>{result.rows.map((row, idx) => <tr key={String(row.id ?? idx)}>
                  {resultColumns.map((col) => <td key={col} className="mono text-xs">{formatCell(row[col])}</td>)}
                </tr>)}</tbody>
              </table>
            </div>}
        </section>}
      </div>}
    </section>

    <ConfirmDialog
      open={deleting !== null}
      busy={busyDelete}
      destructive
      title={t("views.deleteTitle")}
      description={deleting ? t("views.deleteDescription", { name: deleting.name }) : ""}
      confirmLabel={t("common.delete")}
      onOpenChange={(open) => { if (!open && !busyDelete) setDeleting(null); }}
      onConfirm={async () => {
        if (!deleting) return;
        setBusyDelete(true);
        try { await ipc.deleteView(deleting.id); setDeleting(null); await load(); }
        finally { setBusyDelete(false); }
      }}
    />
  </div>;
}

function TargetEnvsSelect({ envs, selectedIds, onChange }: {
  envs: Environment[];
  selectedIds: string[];
  onChange: (ids: string[]) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const root = useRef<HTMLDivElement>(null);
  useDismiss(open, () => root.current, () => setOpen(false));

  const selected = envs.filter((env) => selectedIds.includes(env.id));
  const q = query.trim().toLowerCase();
  const visible = envs.filter((env) => !q || env.name.toLowerCase().includes(q) || (env.project ?? "").toLowerCase().includes(q));
  const toggle = (id: string) => onChange(selectedIds.includes(id) ? selectedIds.filter((x) => x !== id) : [...selectedIds, id]);

  return <div className="relative" ref={root} onKeyDown={(e) => { if (e.key === "Escape" && open) { e.stopPropagation(); setOpen(false); } }}>
    <button
      type="button"
      aria-haspopup="dialog"
      aria-expanded={open}
      aria-label={t("views.targetEnvironments")}
      className="w-full h-8 px-2.5 rounded-md border border-line bg-surface-2 text-sm text-fg flex items-center justify-between gap-2 cursor-pointer hover:border-line-2 transition-colors"
      onClick={() => setOpen((o) => !o)}
    >
      <span className="flex items-center gap-1.5 min-w-0 truncate">
        {selected.length === 0 ? <span className="text-fg-3">{t("views.selectEnvironments")}</span>
          : selected.length === 1 ? <><StageBadge stage={selected[0].stage} compact /><span className="truncate">{selected[0].name}</span></>
            : <><span>{t("views.environmentsSelected", { count: selected.length })}</span><span className="text-fg-3 truncate">· {selected.map((e) => e.name).join(", ")}</span></>}
      </span>
      <Icon name="caretDown" className={cx("size-3.5 text-fg-3 shrink-0 transition-transform", open && "rotate-180")} />
    </button>
    {open && <div role="dialog" aria-label={t("views.targetEnvironments")}
      className="absolute left-0 right-0 bottom-full mb-1 z-30 rounded-lg border border-line bg-surface shadow-lg overflow-hidden">
      {envs.length > 6 && <div className="p-2 border-b border-line">
        <label className="relative block">
          <Icon name="search" className={searchIcon} />
          <input className={searchInput} autoFocus type="search" value={query} placeholder={t("environments.filterPlaceholder")} aria-label={t("environments.filterLabel")} onChange={(e) => setQuery(e.target.value)} />
        </label>
      </div>}
      {envs.length > 1 && <div className="flex items-center justify-between px-2.5 py-1.5 border-b border-line bg-surface-2/60 text-xs">
        <span className="text-fg-3">{t("views.environmentsSelected", { count: selected.length })}</span>
        <div className="flex items-center gap-1">
          <Button size="sm" variant="ghost" onClick={() => onChange(envs.map((e) => e.id))}>{t("views.selectAll")}</Button>
          <Button size="sm" variant="ghost" onClick={() => onChange([])}>{t("views.selectNone")}</Button>
        </div>
      </div>}
      <ul className={cx(filterOptions, "max-h-56 overflow-y-auto p-1.5")} aria-label={t("views.targetEnvironments")}>
        {visible.map((env) => {
          const checked = selectedIds.includes(env.id);
          return <li key={env.id}>
            <label className={filterOption}>
              <input type="checkbox" checked={checked} onChange={() => toggle(env.id)} />
              <StageBadge stage={env.stage} compact />
              <span className="truncate">{env.name}</span>
              {env.project && <span className="ml-auto text-fg-3 text-2xs truncate max-w-[120px]">{env.project}</span>}
            </label>
          </li>;
        })}
        {visible.length === 0 && <li className="px-2 py-1.5 text-fg-3 text-xs">{t("environments.noMatches")}</li>}
      </ul>
    </div>}
  </div>;
}

function ViewForm({ initial, envs, envId, onCancel, onSaved }: {
  initial: SavedViewDraft;
  envs: Environment[];
  envId?: string;
  onCancel: () => void;
  onSaved: (view: SavedView) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(initial.name);
  const [description, setDescription] = useState(initial.description);
  const [model, setModel] = useState(initial.model);
  const [fieldsText, setFieldsText] = useState(initial.fields.join(", "));
  const [order, setOrder] = useState(initial.order ?? "");
  const [limit, setLimit] = useState(String(initial.limit || 50));
  const nextNodeId = useRef(1);
  const allocId = () => nextNodeId.current++;
  const [initialTree] = useState<FilterGroup | null>(() => domainToTree(initial.domain ?? [], () => nextNodeId.current++));
  const [filterMode, setFilterMode] = useState<"visual" | "json">(initialTree ? "visual" : "json");
  const [filterTree, setFilterTree] = useState<FilterGroup>(() => initialTree ?? createEmptyRoot(() => nextNodeId.current++));
  const [domainText, setDomainText] = useState(() => JSON.stringify(initial.domain ?? [], null, 2));
  const [scheduleEnabled, setScheduleEnabled] = useState(initial.schedule?.enabled ?? false);
  const [intervalMinutes, setIntervalMinutes] = useState(String(initial.schedule?.interval_minutes ?? 60));
  const [alertKind, setAlertKind] = useState<AlertCondition["kind"]>(initial.schedule?.alert_on.kind ?? "any_rows");
  const [threshold, setThreshold] = useState(String(initial.schedule?.alert_on.kind === "count_above" ? initial.schedule.alert_on.threshold : 0));
  const [targetEnvs, setTargetEnvs] = useState<string[]>(initial.schedule?.environment_ids ?? (envs[0] ? [envs[0].id] : []));
  const [busy, setBusy] = useState(false);
  const [modelSuggestions, setModelSuggestions] = useState<string[]>(() => (envId && modelCache.get(envId)) || []);
  const [fieldSuggestions, setFieldSuggestions] = useState<string[]>(() => (envId && model.trim() && fieldCache.get(`${envId}:${model.trim()}`)) || []);

  // Load Odoo model names once per environment for live autocomplete.
  useEffect(() => {
    if (!envId || typeof ipc.shell !== "function") return;
    const cached = modelCache.get(envId);
    if (cached) { setModelSuggestions(cached); return; }
    let live = true;
    ipc.shell({ environment_id: envId, model: "ir.model", method: "search_read", args: [[]], kwargs: { fields: ["model"], limit: 1500 }, context: {}, confirmed: false })
      .then((res) => {
        if (!live || !Array.isArray(res.data)) return;
        const names = (res.data as { model?: string }[]).map((r) => r.model).filter((m): m is string => typeof m === "string").sort();
        modelCache.set(envId, names);
        setModelSuggestions(names);
      }, () => undefined);
    return () => { live = false; };
  }, [envId]);

  // Load field names for the typed Odoo model.
  useEffect(() => {
    const target = model.trim();
    if (!envId || !target || !target.includes(".") || typeof ipc.shell !== "function") { setFieldSuggestions([]); return; }
    const key = `${envId}:${target}`;
    const cached = fieldCache.get(key);
    if (cached) { setFieldSuggestions(cached); return; }
    let live = true;
    const timer = setTimeout(() => {
      ipc.shell({ environment_id: envId, model: target, method: "fields_get", args: [], kwargs: { attributes: ["string"] }, context: {}, confirmed: false })
        .then((res) => {
          if (!live || !res.data || typeof res.data !== "object") return;
          const names = Object.keys(res.data as Record<string, unknown>).sort();
          fieldCache.set(key, names);
          setFieldSuggestions(names);
        }, () => { if (live) setFieldSuggestions([]); });
    }, 300);
    return () => { live = false; clearTimeout(timer); };
  }, [envId, model]);

  let parsedJsonDomain: unknown = null;
  try { parsedJsonDomain = JSON.parse(domainText); } catch { /* shown below */ }
  const jsonValid = Array.isArray(parsedJsonDomain);
  const activeDomain = filterMode === "visual" ? treeToDomain(filterTree) : (jsonValid ? (parsedJsonDomain as unknown[]) : null);
  const domainValid = activeDomain !== null;
  const limitNum = Number(limit);
  const limitValid = Number.isInteger(limitNum) && limitNum >= 1 && limitNum <= 500;
  const canSave = !!name.trim() && !!model.trim() && domainValid && limitValid && (!scheduleEnabled || targetEnvs.length > 0);
  const row2 = "grid grid-cols-2 gap-3 max-sm:grid-cols-1";

  const alertOn: AlertCondition = alertKind === "count_above"
    ? { kind: "count_above", threshold: Math.max(0, Math.floor(Number(threshold) || 0)) }
    : { kind: alertKind };

  function switchMode(next: "visual" | "json") {
    if (next === filterMode) return;
    if (next === "json") {
      setDomainText(JSON.stringify(treeToDomain(filterTree), null, 2));
      setFilterMode("json");
    } else if (jsonValid) {
      const converted = domainToTree(parsedJsonDomain as unknown[], allocId);
      if (converted) {
        setFilterTree(converted);
        setFilterMode("visual");
      }
    }
  }

  function insertPlaceholder(ph: string) {
    if (filterMode === "json") {
      setDomainText((cur) => cur.trim() === "[]" ? `[["create_date", ">=", "${ph}"]]` : cur);
      return;
    }
    setFilterTree((tree) => applyPlaceholderToTree(tree, ph, allocId));
  }

  async function save(event: React.FormEvent) {
    event.preventDefault();
    if (!canSave || busy || !activeDomain) return;
    setBusy(true);
    try {
      const saved = await ipc.saveView({
        id: initial.id,
        name: name.trim(),
        description: description.trim(),
        model: model.trim(),
        domain: activeDomain,
        fields: fieldsText.split(/[\s,]+/).filter(Boolean),
        order: order.trim() || null,
        limit: limitNum,
        schedule: scheduleEnabled || initial.schedule ? {
          enabled: scheduleEnabled,
          environment_ids: targetEnvs,
          interval_minutes: Number(intervalMinutes),
          alert_on: alertOn,
        } : null,
      });
      onSaved(saved);
    } finally { setBusy(false); }
  }

  const canSwitchToVisual = jsonValid && domainToTree(parsedJsonDomain as unknown[], () => 0) !== null;

  return <form className={detail} aria-labelledby="view-form-title" onSubmit={(e) => void save(e)}>
    <header className={detailHeader}>
      <span className={tile()}><Icon name="table" /></span>
      <h2 id="view-form-title" className="grow">{t(initial.id ? "views.editTitle" : "views.createTitle")}</h2>
    </header>
    <fieldset disabled={busy} className="grid gap-3 min-w-0 m-0 p-0 border-0">
      <section className={panel}>
        <PanelHeader><h3>{t("views.generalTitle")}</h3></PanelHeader>
        <div className={cx(panelBody, "grid grid-cols-2 gap-x-3.5 gap-y-3 max-sm:grid-cols-1 pt-3.5 pb-3.5")}>
          <label className={field}><span>{t("views.name")}</span><input autoFocus required maxLength={120} value={name} placeholder={t("views.namePlaceholder")} onChange={(e) => setName(e.target.value)} /></label>
          <div className={field}><span id="view-model-label">{t("views.model")}</span><HeaderInput aria-labelledby="view-model-label" required className="mono" value={model} placeholder="res.partner" suggestions={modelSuggestions} onChange={setModel} onBlur={() => undefined} /></div>
          <label className={cx(field, "col-span-2 max-sm:col-auto")}><span>{t("views.description")}</span><input maxLength={300} value={description} placeholder={t("views.descriptionPlaceholder")} onChange={(e) => setDescription(e.target.value)} /></label>
        </div>
      </section>

      <section className={panel}>
        <PanelHeader>
          <h3>{t("views.queryTitle")}</h3>
          <Segmented role="radiogroup" aria-label={t("views.filters")}>
            <Segment role="radio" aria-checked={filterMode === "visual"} aria-pressed={filterMode === "visual"} disabled={filterMode === "json" && !canSwitchToVisual} onClick={() => switchMode("visual")}>{t("views.filtersVisual")}</Segment>
            <Segment role="radio" aria-checked={filterMode === "json"} aria-pressed={filterMode === "json"} onClick={() => switchMode("json")}>JSON</Segment>
          </Segmented>
        </PanelHeader>
        <div className={cx(panelBody, "grid gap-3 pt-3.5 pb-3.5")}>
          {filterMode === "visual" ? (
            <FilterGroupEditor
              group={filterTree}
              fieldSuggestions={fieldSuggestions}
              placeholders={PLACEHOLDERS}
              nextId={allocId}
              onChange={setFilterTree}
              onInsertPlaceholder={insertPlaceholder}
            />
          ) : (
            <div className="grid gap-2">
              <span className="text-xs font-medium text-fg-2">{t("views.filters")}</span>
              <textarea rows={3} className="mono" aria-label={t("views.domain")} value={domainText} onChange={(e) => setDomainText(e.target.value)} />
              {!jsonValid && <small className="text-danger text-xs">{t("views.domainInvalid")}</small>}
              <div className="flex flex-wrap items-center gap-1">
                <small className="text-fg-3 text-xs mr-1">{t("views.placeholders")}:</small>
                {PLACEHOLDERS.map((ph) => <button key={ph} type="button" className="px-1.5 py-0.5 rounded-sm border border-line bg-surface-2 font-mono text-2xs text-fg-2 cursor-pointer hover:border-accent hover:text-fg"
                  onClick={() => insertPlaceholder(ph)}>{ph}</button>)}
              </div>
            </div>
          )}
          <label className={field}><span>{t("views.fields")}</span><input className="mono" value={fieldsText} placeholder="id, name, state, create_date" onChange={(e) => setFieldsText(e.target.value)} /></label>
          <div className={row2}>
            <label className={field}><span>{t("views.order")}</span><input className="mono" value={order} placeholder="create_date desc" onChange={(e) => setOrder(e.target.value)} /></label>
            <label className={field}><span>{t("views.limit")}</span><input type="number" min={1} max={500} value={limit} onChange={(e) => setLimit(e.target.value)} /></label>
          </div>
        </div>
      </section>

      <section className={panel}>
        <PanelHeader>
          <label className="inline-flex items-center gap-2 text-sm font-medium text-fg cursor-pointer">
            <input type="checkbox" checked={scheduleEnabled} onChange={(e) => setScheduleEnabled(e.target.checked)} />
            <span>{t("views.enableSchedule")}</span>
          </label>
        </PanelHeader>
        {scheduleEnabled && <div className={cx(panelBody, "grid gap-3 pt-3.5 pb-3.5")}>
          <div className={row2}>
            <label className={field}><span>{t("views.interval")}</span>
              <Select label={t("views.interval")} value={intervalMinutes} onChange={setIntervalMinutes}
                options={INTERVALS.map((m) => ({ value: String(m), label: formatInterval(m, t as never) }))} />
            </label>
            <label className={field}><span>{t("views.alertCondition")}</span>
              <Select label={t("views.alertCondition")} value={alertKind} onChange={(v) => setAlertKind(v as AlertCondition["kind"])} options={[
                { value: "any_rows", label: t("views.alerts.anyRows") },
                { value: "count_above", label: t("views.alerts.countAboveOption") },
                { value: "changed", label: t("views.alerts.changed") },
              ]} />
            </label>
          </div>
          {alertKind === "count_above" && <label className={field}><span>{t("views.threshold")}</span>
            <input type="number" min={0} value={threshold} onChange={(e) => setThreshold(e.target.value)} /></label>}
          <div className={field}>
            <span>{t("views.targetEnvironments")}</span>
            {envs.length === 0 ? <small className="text-fg-3 text-xs">{t("clients.noEnvironmentsYet")}</small>
              : <TargetEnvsSelect envs={envs} selectedIds={targetEnvs} onChange={setTargetEnvs} />}
          </div>
        </div>}
      </section>
    </fieldset>
    <footer className="flex items-center gap-2 justify-end">
      <Button disabled={busy} onClick={onCancel}>{t("common.cancel")}</Button>
      <Button type="submit" variant="primary" disabled={busy || !canSave}>{busy ? t("common.saving") : t(initial.id ? "common.save" : "views.create")}</Button>
    </footer>
  </form>;
}
