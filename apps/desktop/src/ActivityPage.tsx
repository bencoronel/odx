import { Fragment, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import CopyButton from "./CopyButton";
import { jsonNodes, jsonTextNodes } from "./json";
import { rowsTable } from "./table";
import { ipc, type AuditDetails, type AuditPayload, type ActivityCursor, type ActivityEvent, type ActivityFilter, type ActivityPage as Page, type Environment, type Client } from "./ipc";
import { Icon } from "./Icon";
import SearchField from "./SearchField";
import { ListResizer, useListWidth } from "./ListResizer";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "./i18n/format";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { isRecord, shellCommand } from "./shell/commands";
import { relativeTime, useNow } from "./time";
import { useDismiss } from "./ContextMenu";

/** Ten core pages; past this, narrow the filters instead of growing an unvirtualized list. */
const MAX_EVENTS = 1000;
const outcomes: Record<string, { label: string; tone: string }> = {
  succeeded: { label: "activity.outcomes.succeeded", tone: "ok" },
  attempted: { label: "activity.outcomes.attempted", tone: "neutral" },
  pending: { label: "activity.outcomes.pending", tone: "info" },
  failed: { label: "activity.outcomes.failed", tone: "danger" },
  uncertain: { label: "activity.outcomes.uncertain", tone: "warn" },
  rejected: { label: "activity.outcomes.rejected", tone: "danger" },
  denied: { label: "activity.outcomes.denied", tone: "danger" },
  cancelled: { label: "activity.outcomes.cancelled", tone: "neutral" },
  not_sent: { label: "activity.outcomes.notSent", tone: "neutral" },
  expired: { label: "activity.outcomes.expired", tone: "neutral" },
  revoked: { label: "activity.outcomes.revoked", tone: "neutral" },
};

function split(operation: string) {
  const [method, model] = operation.split(":");
  return { method, model };
}
const keyOf = (event: ActivityEvent, index: number) => event.request_id ?? `${event.at}-${index}`;
const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();
const filtered = (filter: ActivityFilter & { period?: string }) => !!(filter.q || filter.environment_id || filter.client_id || filter.period || filter.writes_only || filter.errors_only);

/** The recorded call as a Terminal command: reads only, and only when the core kept it whole and unredacted. */
export function replayCommand(event: ActivityEvent, request: AuditPayload | null | undefined): string | null {
  if (event.write || !request || request.truncated || request.text.includes("[redacted]")) return null;
  try {
    const call = JSON.parse(request.text);
    return typeof call?.model === "string" && typeof call?.method === "string" ? shellCommand(call) : null;
  } catch { return null; }
}

/** `tableTitle`: a list of records also opens in a table window, like a Terminal result. */
function Payload({ title, payload, tableTitle }: { title: string; payload: AuditPayload; tableTitle?: string }) {
  const { t } = useTranslation();
  // Parsed once per payload, not on every list refresh.
  const [body, records] = useMemo((): [ReactNode, Record<string, unknown>[] | null] => {
    if (!payload.truncated) try {
      const value: unknown = JSON.parse(payload.text);
      return [jsonNodes(value), tableTitle && Array.isArray(value) && value.length && value.every(isRecord) ? value : null];
    } catch { /* Not JSON: shown as recorded. */ }
    return [payload.truncated && /^\s*[[{]/.test(payload.text) ? jsonTextNodes(payload.text) : payload.text, null];
  }, [payload, tableTitle]);
  return <section className="activity-payload"><h4>{title}<CopyButton text={payload.text} className="icon-btn sm" tip={t("common.copy")} ariaLabel={t("common.copy")} iconOnly />
    {records && <button type="button" className="btn sm" onClick={() => void ipc.openTableWindow({ table: rowsTable(records), title: tableTitle }, tableTitle!).catch(() => undefined)}><Icon name="table" />{t("activity.openTable")}</button>}</h4>
    <pre className="code">{body}</pre>
    {payload.truncated && <p className="hint">{t("activity.truncated")}</p>}
  </section>;
}

function Detail({ event, filterBy, openInShell }: {
  event: ActivityEvent;
  filterBy: (filter: ActivityFilter, clientName?: string | null) => void;
  openInShell?: (env: string, command: string) => void;
}) {
  const { t } = useTranslation();
  const { method, model } = split(event.operation);
  const outcome = outcomes[event.outcome] ?? { label: event.outcome, tone: "neutral" };
  // The list carries no details; read them for the opened request, again when its outcome changes. undefined: loading.
  const [detail, setDetail] = useState<AuditDetails | null | undefined>(undefined);
  useEffect(() => {
    if (!event.request_id) { setDetail(null); return; }
    let live = true;
    ipc.activityDetail(event.request_id).then((found) => { if (live) setDetail(found); }, () => { if (live) setDetail(null); });
    return () => { live = false; };
  }, [event.request_id, event.outcome]);
  const replay = event.environment_name && openInShell ? replayCommand(event, detail?.request) : null;
  return <section className="detail" aria-label={t("activity.detailsFor", { operation: event.operation })}>
    <header className="detail-header">
      <div className="grow min-w-0 detail-title">
        <h2 className="row gap-2 min-w-0"><code className={`method tone-${event.write ? "warn" : "info"}`}>{method}</code>{model && <span className="mono truncate">{model}</span>}</h2>
        <p className="hint detail-subtitle">
          {event.environment_name
            ? <button type="button" className="cell-link" data-tip={t("activity.filterBy", { name: event.environment_name })} onClick={() => filterBy({ environment_id: event.environment_id })}>{event.environment_name}</button>
            : t("activity.deleted")}
          {" · "}
          {event.client_id
            ? <button type="button" className="cell-link" data-tip={t("activity.filterBy", { name: event.client_label ?? t("activity.thisClient") })} onClick={() => filterBy({ client_id: event.client_id! }, event.client_label)}>{event.client_label ?? t("activity.revoked")}</button>
            : t("approvals.local")}
          {" · "}{formatDateTime(new Date(event.at), { dateStyle: "medium", timeStyle: "medium" })}
          {detail?.duration_ms != null && ` · ${t("activity.duration", { duration: detail.duration_ms })}`}
        </p>
      </div>
      <span className={`chip chip-tone tone-${outcome.tone}`}>{outcomes[event.outcome] ? t(outcome.label as "activity.outcomes.succeeded") : outcome.label}</span>
      {replay && <button type="button" className="btn sm" onClick={() => openInShell!(event.environment_name!, replay)}><Icon name="terminal" />{t("activity.replay")}</button>}
    </header>
    {detail === undefined ? null : !detail ? <p className="hint">{t("activity.oldRecord")}</p> : <>
      {detail.request && <Payload title={t("activity.request")} payload={detail.request} />}
      {detail.error_code ? <section className="activity-payload"><h4>{t("activity.error")}</h4><p>{formatAppError({ code: detail.error_code, params: detail.error_params ?? undefined })}</p>{detail.error && <pre className="code">{detail.error.text}</pre>}</section>
        : detail.error ? <><p>{formatAppError({ code: "unknown" })}</p><Payload title={t("activity.error")} payload={detail.error} /></>
        : detail.response ? <Payload title={t("activity.response")} payload={detail.response} tableTitle={`${model ?? method}${event.environment_name ? ` · ${event.environment_name}` : ""}`} />
        : <p className="hint">{event.outcome === "pending" ? t("activity.pendingExecution") : event.outcome === "attempted" ? t("activity.waitingResponse") : ["rejected", "denied", "cancelled"].includes(event.outcome) ? t("activity.notExecuted") : t("activity.noResponse")}</p>}
    </>}
  </section>;
}

type Period = "hour" | "today" | "week";
const PERIODS: Record<Period, "activity.lastHour" | "activity.today" | "activity.lastWeek"> = { hour: "activity.lastHour", today: "activity.today", week: "activity.lastWeek" };
/** The page's filter. `period` becomes `since` each time a page is asked for, so "last hour" moves with the refresh. */
type Filter = ActivityFilter & { period?: Period };
function query({ period, ...filter }: Filter): ActivityFilter {
  if (!period) return filter;
  const since = new Date();
  if (period === "today") since.setHours(0, 0, 0, 0);
  else since.setTime(since.getTime() - (period === "hour" ? 3_600_000 : 7 * 86_400_000));
  return { ...filter, since: since.toISOString() };
}

function RadioSection({ name, heading, options, value, pick }: {
  name: string; heading: string; options: { id: string; name: string }[]; value: string | undefined; pick: (id: string | undefined) => void;
}) {
  return <div className="filter-section">
    <span className="filter-heading">{heading}</span>
    <ul className="filter-options" role="radiogroup" aria-label={heading}>
      {options.map((option) => <li key={option.id}><label className="filter-option">
        <input type="radio" name={name} checked={(value ?? "") === option.id} onChange={() => pick(option.id || undefined)} />
        <span className="truncate">{option.name}</span>
      </label></li>)}
    </ul>
  </div>;
}

/** Filter button with the kind of event, period, environment and client; the active ones show as chips in the search box. */
function Filters({ envs, clients, filter, change }: { envs: Environment[]; clients: Client[]; filter: Filter; change: (filter: Filter) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  useDismiss(open, () => root.current, () => setOpen(false));
  const active = [filter.environment_id, filter.client_id, filter.period, filter.writes_only, filter.errors_only].filter(Boolean).length;
  return <div className="filter-menu" ref={root} onKeyDown={(e) => { if (e.key === "Escape" && open) { e.stopPropagation(); setOpen(false); } }}>
    <button type="button" className={`btn sm icon-only${active ? " filter-active" : ""}`} aria-expanded={open} aria-haspopup="dialog"
      aria-label={t("activity.filters")} data-tip={t("activity.filters")} onClick={() => setOpen(!open)}>
      <Icon name="filter" />{active > 0 && <span className="filter-badge" aria-hidden="true">{active}</span>}
    </button>
    {open && <div className="filter-panel" role="dialog" aria-label={t("activity.filters")}>
      <div className="filter-section">
        <span className="filter-heading">{t("activity.kind")}</span>
        <ul className="filter-options">
          <li><label className="filter-option"><input type="checkbox" checked={!!filter.writes_only} onChange={() => change({ ...filter, writes_only: !filter.writes_only || undefined })} /><Icon name="pencil" /><span className="truncate">{t("activity.writes")}</span></label></li>
          <li><label className="filter-option"><input type="checkbox" checked={!!filter.errors_only} onChange={() => change({ ...filter, errors_only: !filter.errors_only || undefined })} /><Icon name="alert" /><span className="truncate">{t("activity.errors")}</span></label></li>
        </ul>
      </div>
      <RadioSection name="activity-period" heading={t("activity.period")} value={filter.period} pick={(id) => change({ ...filter, period: id as Period | undefined })}
        options={[{ id: "", name: t("activity.anyTime") }, ...(Object.keys(PERIODS) as Period[]).map((id) => ({ id, name: t(PERIODS[id]) }))]} />
      <RadioSection name="activity-environment" heading={t("common.environment")} value={filter.environment_id} pick={(id) => change({ ...filter, environment_id: id })}
        options={[{ id: "", name: t("activity.allEnvironments") }, ...envs]} />
      {clients.length > 0 && <RadioSection name="activity-client" heading={t("activity.client")} value={filter.client_id} pick={(id) => change({ ...filter, client_id: id })}
        options={[{ id: "", name: t("activity.allClients") }, ...clients.map((client) => ({ id: client.id, name: client.label }))]} />}
    </div>}
  </div>;
}

export default function ActivityPage({ envs, refreshKey, openInShell }: {
  envs: Environment[];
  refreshKey: number;
  /** Repeat a recorded read in the Terminal, connected to its environment. */
  openInShell?: (env: string, command: string) => void;
}) {
  const { t } = useTranslation();
  const [listWidth, setListWidth] = useListWidth("odx.activity.listWidth");
  const [filter, setFilter] = useState<Filter>({});
  const [reload, setReload] = useState(0);
  const [q, setQ] = useState("");
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [next, setNext] = useState<ActivityCursor | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ErrorState>("");
  const [clientName, setClientName] = useState<string | null>(null);
  const [clients, setClients] = useState<Client[]>([]);
  // A refreshed first page waits here while the reader is scrolled down, instead of shifting the rows under them.
  const [incoming, setIncoming] = useState<Page | null>(null);
  const [loaded, setLoaded] = useState(false);
  const list = useRef<HTMLUListElement>(null);
  const extra = useRef(false); // Disable automatic refresh after loading more pages.
  // Discard responses requested with an earlier filter.
  const generation = useRef(0);
  const now = useNow(events.length > 0, 30_000);

  useEffect(() => {
    const timer = window.setTimeout(() => setFilter((f) => (f.q === (q.trim() || undefined) ? f : { ...f, q: q.trim() || undefined })), 250);
    return () => window.clearTimeout(timer);
  }, [q]);

  useEffect(() => { ipc.clients().then((all) => setClients(all.filter((client) => !client.revoked)), () => undefined); }, []);

  const show = (page: Page) => { setEvents(page.events); setNext(page.next_cursor ?? null); setIncoming(null); setLoaded(true); };

  // Refresh the first page silently every 5 s.
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const timer = window.setInterval(() => { if (!extra.current && document.visibilityState === "visible") setTick((n) => n + 1); }, 5000);
    return () => window.clearInterval(timer);
  }, []);
  useEffect(() => {
    if (!tick) return;
    const current = generation.current;
    ipc.activity(null, query(filter))
      .then((page) => {
        if (current !== generation.current || extra.current) return;
        if ((list.current?.scrollTop ?? 0) > 4) setIncoming(page); else show(page);
      })
      .catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick]);
  // App refreshes reload the first page like the timer, so pages loaded with "Load more" are kept.
  const shownKey = useRef(refreshKey);
  useEffect(() => { if (shownKey.current !== refreshKey) { shownKey.current = refreshKey; if (!extra.current) setTick((n) => n + 1); } }, [refreshKey]);

  useEffect(() => {
    const current = ++generation.current;
    setLoading(true);
    setError("");
    extra.current = false;
    ipc.activity(null, query(filter))
      .then((page) => { if (current === generation.current) show(page); })
      .catch((reason) => { if (current === generation.current) setError(normalizeAppError(reason)); })
      .finally(() => { if (current === generation.current) setLoading(false); });
  }, [filter, reload]);

  async function more() {
    if (next === null) return;
    const current = generation.current;
    setLoading(true);
    try {
      const page = await ipc.activity(next, query(filter));
      if (current !== generation.current) return;
      extra.current = true;
      setEvents((shown) => [...shown, ...page.events]);
      setNext(page.next_cursor ?? null);
    } catch (reason) { if (current === generation.current) setError(normalizeAppError(reason)); } finally { if (current === generation.current) setLoading(false); }
  }

  const keys = events.map(keyOf);
  const shownKeys = new Set(keys);
  const fresh = incoming ? incoming.events.filter((event, i) => !shownKeys.has(keyOf(event, i))).length : 0;
  const selectedIndex = Math.max(0, keys.indexOf(selectedKey ?? ""));
  const selected = events[selectedIndex];
  const filterBy = (change: Filter, name?: string | null) => { if (change.client_id) setClientName(name ?? null); setFilter({ ...filter, ...change }); };
  const clearFilters = () => { setQ(""); setClientName(null); setFilter({}); };
  const envName = envs.find((env) => env.id === filter.environment_id)?.name ?? events.find((event) => event.environment_id === filter.environment_id)?.environment_name;
  const chip = (key: string, label: string, icon: "server" | "clients" | "clock" | "pencil" | "alert", drop: Filter) =>
    <span key={key} className="chip token">{<Icon name={icon} />}{label}
      <button type="button" aria-label={t("activity.removeFilter", { name: label })} onClick={() => setFilter({ ...filter, ...drop })}><Icon name="x" /></button></span>;
  const chips = [
    filter.environment_id && chip("env", envName ?? t("activity.deleted"), "server", { environment_id: undefined }),
    filter.period && chip("period", t(PERIODS[filter.period]), "clock", { period: undefined }),
    filter.client_id && chip("client", clients.find((client) => client.id === filter.client_id)?.label ?? clientName ?? t("activity.revokedClient"), "clients", { client_id: undefined }),
    filter.writes_only && chip("writes", t("activity.writes"), "pencil", { writes_only: undefined }),
    filter.errors_only && chip("errors", t("activity.errors"), "alert", { errors_only: undefined }),
  ].filter(Boolean);
  const select = (index: number) => { const key = keys[index]; if (key) setSelectedKey(key); };

  if (loaded && !filtered(filter) && !q && events.length === 0 && !error) return <div className="empty">
    <span className="empty-icon"><Icon name="activity" /></span>
    <h2>{t("activity.emptyTitle")}</h2>
  </div>;

  return <div className="split" style={{ "--list-w": `${listWidth}px` } as CSSProperties}>
    <aside className="list-pane" aria-label={t("activity.listLabel")}>
      <ListResizer width={listWidth} setWidth={setListWidth} label={t("activity.resizeList")} />
      <div className="list-toolbar">
        <div className="row gap-2">
          <SearchField className="grow" value={q} onChange={setQ} label={t("activity.searchLabel")} placeholder={chips.length ? "" : t("activity.searchPlaceholder")}
            chips={chips.length ? <>{chips}</> : undefined} filtered={filtered(filter) || !!q} onClear={clearFilters} clearLabel={t("activity.clearFilters")} />
          <Filters envs={envs} clients={clients} filter={filter} change={setFilter} />
          <button type="button" className="btn sm icon-only" aria-label={t("activity.refresh")} data-tip={t("common.refresh")} disabled={loading} onClick={() => setReload((n) => n + 1)}><Icon name="refresh" className={loading ? "spin" : undefined} /></button>
        </div>
        {error && <p className="banner danger compact" role="alert"><Icon name="alert" />{formatAppError(error)}</p>}
      </div>
      {fresh > 0 && <button type="button" className="new-events" onClick={() => { show(incoming!); if (list.current) list.current.scrollTop = 0; }}><Icon name="caretUp" />{t("activity.newEvents", { count: fresh })}</button>}
      <ul className="env-list activity-list" role="listbox" aria-label={t("activity.listLabel")} ref={list}
        onScroll={(e) => { if (incoming && e.currentTarget.scrollTop <= 4) show(incoming); }}
        onKeyDown={(e) => {
          if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
          e.preventDefault();
          const to = selectedIndex + (e.key === "ArrowDown" ? 1 : -1);
          select(to);
          e.currentTarget.querySelector<HTMLElement>(`[data-index="${to}"]`)?.focus();
        }}>
        {events.map((event, index) => {
          const { method, model } = split(event.operation);
          const outcome = outcomes[event.outcome] ?? { label: event.outcome, tone: "neutral" };
          const at = new Date(event.at);
          const day = index === 0 || !sameDay(at, new Date(events[index - 1].at));
          const today = sameDay(at, new Date(now));
          const yesterday = sameDay(at, new Date(now - 86_400_000));
          return <Fragment key={keys[index]}>
            {day && <li role="presentation" className="list-day">{today ? t("activity.today") : yesterday ? t("activity.yesterday") : formatDateTime(at, { day: "numeric", month: "short", year: at.getFullYear() === new Date(now).getFullYear() ? undefined : "numeric" })}</li>}
            <li role="option" data-index={index} aria-selected={index === selectedIndex} tabIndex={index === selectedIndex ? 0 : -1} className="activity-item" onClick={() => select(index)}>
              <code className={`method tone-${event.write ? "warn" : "info"}`}>{method}</code>
              <span className="env-list-text">
                <strong className="mono">{model}</strong>
                <small>{event.environment_name ?? t("activity.deleted")} · {event.client_id ? event.client_label ?? t("activity.revoked") : t("approvals.local")}</small>
              </span>
              <span className="activity-item-end">
                <span className={`status-dot tone-${outcome.tone}`} data-tip={outcomes[event.outcome] ? t(outcome.label as "activity.outcomes.succeeded") : outcome.label} />
                {/* Today reads best as elapsed time; older days already have their header. */}
                <time dateTime={event.at} data-tip={formatDateTime(at, { dateStyle: "medium", timeStyle: "medium" })}>{today ? relativeTime(event.at, now) : formatDateTime(at, { hour: "2-digit", minute: "2-digit" })}</time>
              </span>
            </li>
          </Fragment>;
        })}
        {events.length === 0 && <li className="list-empty">
          <span className="list-empty-icon"><Icon name="search" /></span>
          <strong>{t("activity.noMatches")}</strong>
          <button type="button" className="btn sm" onClick={clearFilters}><Icon name="x" />{t("activity.clearFilters")}</button>
        </li>}
        {next !== null && events.length >= MAX_EVENTS && <li role="presentation" className="list-note hint">{t("activity.limitReached", { count: MAX_EVENTS })}</li>}
        {next !== null && events.length < MAX_EVENTS && <li role="presentation" className="list-note"><button className="btn sm" disabled={loading} onClick={() => void more()}>{loading ? t("common.loading") : t("activity.loadMore")}</button></li>}
      </ul>
    </aside>
    <section className="detail-pane">
      {selected && <Detail key={keys[selectedIndex]} event={selected} filterBy={filterBy} openInShell={openInShell} />}
    </section>
  </div>;
}
