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
import { Banner, Button, Chip, StatusDot, detail as detailClass, detailHeader, detailPane, empty, emptyIcon, envList, envListText, filterBadge, filterHeading, filterOption, filterOptions, filterPanel, filterSection, iconButton, listEmpty, listEmptyIcon, listPane, listToolbar, method, split, type Tone } from "./ui";

/** Ten core pages; past this, narrow the filters instead of growing an unvirtualized list. */
const MAX_EVENTS = 1000;
const outcomes: Record<string, { label: string; tone: Tone }> = {
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

function parts(operation: string) {
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

const PAYLOAD_CODE = "code max-h-[420px] overflow-auto px-3 py-2.5 bg-surface-2 text-fg text-sm leading-[1.55] whitespace-pre-wrap wrap-anywhere border border-line";
const CELL_LINK = "cursor-pointer underline decoration-dotted decoration-transparent underline-offset-3 hover:text-accent-text hover:decoration-current";

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
  return <section><h4 className="flex items-center gap-1.5 mb-2 text-sm">{title}<CopyButton text={payload.text} className={iconButton("sm")} tip={t("common.copy")} ariaLabel={t("common.copy")} iconOnly />
    {records && <Button size="sm" onClick={() => void ipc.openTableWindow({ table: rowsTable(records), title: tableTitle }, tableTitle!).catch(() => undefined)}><Icon name="table" />{t("activity.openTable")}</Button>}</h4>
    <pre className={PAYLOAD_CODE}>{body}</pre>
    {payload.truncated && <p className="text-fg-3 text-sm">{t("activity.truncated")}</p>}
  </section>;
}

function Detail({ event, filterBy, openInShell }: {
  event: ActivityEvent;
  filterBy: (filter: ActivityFilter, clientName?: string | null) => void;
  openInShell?: (env: string, command: string) => void;
}) {
  const { t } = useTranslation();
  const { method: name, model } = parts(event.operation);
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
  return <section className={detailClass} aria-label={t("activity.detailsFor", { operation: event.operation })}>
    <header className={detailHeader}>
      <div className="min-w-0 flex-[1_1_220px]">
        <h2 className="flex items-center gap-2 min-w-0"><code className={method(event.write ? "warn" : "info")}>{name}</code>{model && <span className="mono truncate">{model}</span>}</h2>
        <p className="mt-px mb-0.5 text-fg-3 text-sm">
          {event.environment_name
            ? <button type="button" className={CELL_LINK} data-tip={t("activity.filterBy", { name: event.environment_name })} onClick={() => filterBy({ environment_id: event.environment_id })}>{event.environment_name}</button>
            : t("activity.deleted")}
          {" · "}
          {event.client_id
            ? <button type="button" className={CELL_LINK} data-tip={t("activity.filterBy", { name: event.client_label ?? t("activity.thisClient") })} onClick={() => filterBy({ client_id: event.client_id! }, event.client_label)}>{event.client_label ?? t("activity.revoked")}</button>
            : t("approvals.local")}
          {" · "}{formatDateTime(new Date(event.at), { dateStyle: "medium", timeStyle: "medium" })}
          {detail?.duration_ms != null && ` · ${t("activity.duration", { duration: detail.duration_ms })}`}
        </p>
      </div>
      <Chip tone={outcome.tone}>{outcomes[event.outcome] ? t(outcome.label as "activity.outcomes.succeeded") : outcome.label}</Chip>
      {replay && <Button size="sm" onClick={() => openInShell!(event.environment_name!, replay)}><Icon name="terminal" />{t("activity.replay")}</Button>}
    </header>
    {detail === undefined ? null : !detail ? <p className="text-fg-3 text-sm">{t("activity.oldRecord")}</p> : <>
      {detail.request && <Payload title={t("activity.request")} payload={detail.request} />}
      {detail.error_code ? <section><h4 className="flex items-center gap-1.5 mb-2 text-sm">{t("activity.error")}</h4><p>{formatAppError({ code: detail.error_code, params: detail.error_params ?? undefined })}</p>{detail.error && <pre className={PAYLOAD_CODE}>{detail.error.text}</pre>}</section>
        : detail.error ? <><p>{formatAppError({ code: "unknown" })}</p><Payload title={t("activity.error")} payload={detail.error} /></>
        : detail.response ? <Payload title={t("activity.response")} payload={detail.response} tableTitle={`${model ?? name}${event.environment_name ? ` · ${event.environment_name}` : ""}`} />
        : <p className="text-fg-3 text-sm">{event.outcome === "pending" ? t("activity.pendingExecution") : event.outcome === "attempted" ? t("activity.waitingResponse") : ["rejected", "denied", "cancelled"].includes(event.outcome) ? t("activity.notExecuted") : t("activity.noResponse")}</p>}
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
  return <div className={filterSection}>
    <span className={filterHeading}>{heading}</span>
    <ul className={filterOptions} role="radiogroup" aria-label={heading}>
      {options.map((option) => <li key={option.id}><label className={filterOption}>
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
  return <div className="flex-none" ref={root} onKeyDown={(e) => { if (e.key === "Escape" && open) { e.stopPropagation(); setOpen(false); } }}>
    <Button variant={active ? "accent" : "default"} size="sm" iconOnly className="relative" aria-expanded={open} aria-haspopup="dialog"
      aria-label={t("activity.filters")} data-tip={t("activity.filters")} onClick={() => setOpen(!open)}>
      <Icon name="filter" />{active > 0 && <span className={filterBadge} aria-hidden="true">{active}</span>}
    </Button>
    {open && <div className={filterPanel} role="dialog" aria-label={t("activity.filters")}>
      <div className={filterSection}>
        <span className={filterHeading}>{t("activity.kind")}</span>
        <ul className={filterOptions}>
          <li><label className={filterOption}><input type="checkbox" checked={!!filter.writes_only} onChange={() => change({ ...filter, writes_only: !filter.writes_only || undefined })} /><Icon name="pencil" /><span className="truncate">{t("activity.writes")}</span></label></li>
          <li><label className={filterOption}><input type="checkbox" checked={!!filter.errors_only} onChange={() => change({ ...filter, errors_only: !filter.errors_only || undefined })} /><Icon name="alert" /><span className="truncate">{t("activity.errors")}</span></label></li>
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
    <Chip key={key} token>{<Icon name={icon} />}{label}
      <button type="button" aria-label={t("activity.removeFilter", { name: label })} onClick={() => setFilter({ ...filter, ...drop })}><Icon name="x" /></button></Chip>;
  const chips = [
    filter.environment_id && chip("env", envName ?? t("activity.deleted"), "server", { environment_id: undefined }),
    filter.period && chip("period", t(PERIODS[filter.period]), "clock", { period: undefined }),
    filter.client_id && chip("client", clients.find((client) => client.id === filter.client_id)?.label ?? clientName ?? t("activity.revokedClient"), "clients", { client_id: undefined }),
    filter.writes_only && chip("writes", t("activity.writes"), "pencil", { writes_only: undefined }),
    filter.errors_only && chip("errors", t("activity.errors"), "alert", { errors_only: undefined }),
  ].filter(Boolean);
  const select = (index: number) => { const key = keys[index]; if (key) setSelectedKey(key); };

  if (loaded && !filtered(filter) && !q && events.length === 0 && !error) return <div className={empty}>
    <span className={emptyIcon()}><Icon name="activity" /></span>
    <h2>{t("activity.emptyTitle")}</h2>
  </div>;

  return <div className={split} style={{ "--list-w": `${listWidth}px` } as CSSProperties}>
    <aside className={listPane} aria-label={t("activity.listLabel")}>
      <ListResizer width={listWidth} setWidth={setListWidth} label={t("activity.resizeList")} />
      <div className={listToolbar}>
        <div className="flex items-center gap-2">
          <SearchField className="grow min-w-0" value={q} onChange={setQ} label={t("activity.searchLabel")} placeholder={chips.length ? "" : t("activity.searchPlaceholder")}
            chips={chips.length ? <>{chips}</> : undefined} filtered={filtered(filter) || !!q} onClear={clearFilters} clearLabel={t("activity.clearFilters")} />
          <Filters envs={envs} clients={clients} filter={filter} change={setFilter} />
          <Button size="sm" iconOnly className="flex-none" aria-label={t("activity.refresh")} data-tip={t("common.refresh")} disabled={loading} onClick={() => setReload((n) => n + 1)}><Icon name="refresh" className={loading ? "animate-spin" : undefined} /></Button>
        </div>
        {error && <Banner tone="danger" compact role="alert"><Icon name="alert" />{formatAppError(error)}</Banner>}
      </div>
      {fresh > 0 && <button type="button" className="inline-flex items-center justify-center gap-1.5 mx-2 mt-1.5 px-2.5 py-1 border border-accent rounded-full bg-accent-soft text-accent-text text-xs cursor-pointer" onClick={() => { show(incoming!); if (list.current) list.current.scrollTop = 0; }}><Icon name="caretUp" />{t("activity.newEvents", { count: fresh })}</button>}
      <ul className={envList} role="listbox" aria-label={t("activity.listLabel")} ref={list}
        onScroll={(e) => { if (incoming && e.currentTarget.scrollTop <= 4) show(incoming); }}
        onKeyDown={(e) => {
          if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
          e.preventDefault();
          const to = selectedIndex + (e.key === "ArrowDown" ? 1 : -1);
          select(to);
          e.currentTarget.querySelector<HTMLElement>(`[data-index="${to}"]`)?.focus();
        }}>
        {events.map((event, index) => {
          const { method: name, model } = parts(event.operation);
          const outcome = outcomes[event.outcome] ?? { label: event.outcome, tone: "neutral" };
          const at = new Date(event.at);
          const day = index === 0 || !sameDay(at, new Date(events[index - 1].at));
          const today = sameDay(at, new Date(now));
          const yesterday = sameDay(at, new Date(now - 86_400_000));
          return <Fragment key={keys[index]}>
            {day && <li role="presentation" className="sticky -top-1 z-1 block px-2 pt-2 pb-1 rounded-none bg-bg text-fg-3 text-xs font-semibold cursor-default">{today ? t("activity.today") : yesterday ? t("activity.yesterday") : formatDateTime(at, { day: "numeric", month: "short", year: at.getFullYear() === new Date(now).getFullYear() ? undefined : "numeric" })}</li>}
            <li role="option" data-index={index} aria-selected={index === selectedIndex} tabIndex={index === selectedIndex ? 0 : -1} className="activity-item" onClick={() => select(index)}>
              <code className={method(event.write ? "warn" : "info")}>{name}</code>
              <span className={envListText}>
                <strong className="mono">{model}</strong>
                <small>{event.environment_name ?? t("activity.deleted")} · {event.client_id ? event.client_label ?? t("activity.revoked") : t("approvals.local")}</small>
              </span>
              <span className="inline-flex flex-none items-center gap-1.5 ml-auto text-fg-3 text-xs tabular-nums">
                <StatusDot tone={outcome.tone} data-tip={outcomes[event.outcome] ? t(outcome.label as "activity.outcomes.succeeded") : outcome.label} />
                {/* Today reads best as elapsed time; older days already have their header. */}
                <time dateTime={event.at} data-tip={formatDateTime(at, { dateStyle: "medium", timeStyle: "medium" })}>{today ? relativeTime(event.at, now) : formatDateTime(at, { hour: "2-digit", minute: "2-digit" })}</time>
              </span>
            </li>
          </Fragment>;
        })}
        {events.length === 0 && <li className={listEmpty}>
          <span className={listEmptyIcon}><Icon name="search" /></span>
          <strong>{t("activity.noMatches")}</strong>
          <Button size="sm" className="mt-2" onClick={clearFilters}><Icon name="x" />{t("activity.clearFilters")}</Button>
        </li>}
        {next !== null && events.length >= MAX_EVENTS && <li role="presentation" className="text-fg-3 text-sm justify-center px-2 py-2.5 text-center cursor-default hover:bg-bg">{t("activity.limitReached", { count: MAX_EVENTS })}</li>}
        {next !== null && events.length < MAX_EVENTS && <li role="presentation" className="justify-center px-2 py-2.5 text-center cursor-default hover:bg-bg"><Button size="sm" disabled={loading} onClick={() => void more()}>{loading ? t("common.loading") : t("activity.loadMore")}</Button></li>}
      </ul>
    </aside>
    <section className={detailPane}>
      {selected && <Detail key={keys[selectedIndex]} event={selected} filterBy={filterBy} openInShell={openInShell} />}
    </section>
  </div>;
}
