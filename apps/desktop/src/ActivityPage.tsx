import { Fragment, useEffect, useRef, useState } from "react";
import { ipc, type AuditPayload, type ActivityCursor, type ActivityEvent, type ActivityFilter, type Environment } from "./ipc";
import { Icon } from "./Icon";
import SearchField from "./SearchField";
import Select from "./Select";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "./i18n/format";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";

/** Ten core pages; past this, narrow the filters instead of growing an unvirtualized table. */
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
function Payload({ title, payload }: { title: string; payload: AuditPayload }) {
  const { t } = useTranslation();
  return <section className="activity-payload"><h4>{title}</h4>
    <pre className="code">{payload.text}</pre>
    {payload.truncated && <p className="hint">{t("activity.truncated")}</p>}
  </section>;
}
function Detail({ event }: { event: ActivityEvent }) {
  const { t } = useTranslation();
  const detail = event.details;
  if (!detail) return <p className="hint">{t("activity.oldRecord")}</p>;
  return <>
    {detail.duration_ms != null && <p className="hint">{t("activity.duration", { duration: detail.duration_ms })}</p>}
    <div className="activity-payloads">
      {detail.request && <Payload title={t("activity.request")} payload={detail.request} />}
      {detail.error_code ? <section className="activity-payload"><h4>{t("activity.error")}</h4><p>{formatAppError({ code: detail.error_code, params: detail.error_params ?? undefined })}</p>{detail.error && <pre className="code">{detail.error.text}</pre>}</section>
        : detail.error ? <><p>{formatAppError({ code: "unknown" })}</p><Payload title={t("activity.error")} payload={detail.error} /></>
        : detail.response ? <Payload title={t("activity.response")} payload={detail.response} />
        : <p className="hint">{event.outcome === "pending" ? t("activity.pendingExecution") : event.outcome === "attempted" ? t("activity.waitingResponse") : ["rejected", "denied", "cancelled"].includes(event.outcome) ? t("activity.notExecuted") : t("activity.noResponse")}</p>}
    </div>
  </>;
}

export default function ActivityPage({ envs, refreshKey }: { envs: Environment[]; refreshKey: number }) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState<ActivityFilter>({});
  const [reload, setReload] = useState(0);
  const [q, setQ] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [events, setEvents] = useState<ActivityEvent[]>([]);
  const [next, setNext] = useState<ActivityCursor | null>(null);
  const [complete, setComplete] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<ErrorState>("");
  const [clientName, setClientName] = useState<string | null>(null);
  const extra = useRef(false); // Disable automatic refresh after loading more pages.
  // Discard responses requested with an earlier filter.
  const generation = useRef(0);

  useEffect(() => {
    const timer = window.setTimeout(() => setFilter((f) => (f.q === (q.trim() || undefined) ? f : { ...f, q: q.trim() || undefined })), 250);
    return () => window.clearTimeout(timer);
  }, [q]);

  // Refresh the first page silently every 5 s.
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const timer = window.setInterval(() => { if (!extra.current && document.visibilityState === "visible") setTick((n) => n + 1); }, 5000);
    return () => window.clearInterval(timer);
  }, []);
  useEffect(() => {
    if (!tick) return;
    const current = generation.current;
    ipc.activity(null, filter)
      .then((page) => { if (current === generation.current && !extra.current) { setEvents(page.events); setNext(page.next_cursor ?? null); setComplete(page.complete); } })
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
    ipc.activity(null, filter)
      .then((page) => { if (current === generation.current) { setEvents(page.events); setNext(page.next_cursor ?? null); setComplete(page.complete); } })
      .catch((reason) => { if (current === generation.current) setError(normalizeAppError(reason)); })
      .finally(() => { if (current === generation.current) setLoading(false); });
  }, [filter, reload]);

  async function more() {
    if (next === null) return;
    const current = generation.current;
    setLoading(true);
    try {
      const page = await ipc.activity(next, filter);
      if (current !== generation.current) return;
      extra.current = true;
      setEvents((shown) => [...shown, ...page.events]);
      setNext(page.next_cursor ?? null);
    } catch (reason) { if (current === generation.current) setError(normalizeAppError(reason)); } finally { if (current === generation.current) setLoading(false); }
  }

  return <div className="page-scroll"><div className="activity">
    <div className="activity-toolbar">
      <SearchField className="grow" value={q} onChange={setQ} label={t("activity.searchLabel")} placeholder={t("activity.searchPlaceholder")} />
      <Select className="input sm-select" label={t("common.environment")} value={filter.environment_id ?? ""} options={[
        { value: "", label: t("activity.allEnvironments") },
        ...envs.map((env) => ({ value: env.id, label: env.name })),
      ]} onChange={(environmentId) => setFilter({ ...filter, environment_id: environmentId || undefined })} />
      <div className="segmented" role="group" aria-label={t("activity.filters")}>
        <button type="button" aria-pressed={!!filter.writes_only} onClick={() => setFilter({ ...filter, writes_only: !filter.writes_only })}><Icon name="pencil" />{t("activity.writes")}</button>
        <button type="button" aria-pressed={!!filter.errors_only} onClick={() => setFilter({ ...filter, errors_only: !filter.errors_only })}><Icon name="alert" />{t("activity.errors")}</button>
      </div>
      {filter.client_id && <button type="button" className="chip chip-tone tone-info filter-chip" onClick={() => { setClientName(null); setFilter({ ...filter, client_id: undefined }); }} aria-label={t("activity.removeClientFilter")}><Icon name="clients" />{clientName ?? t("activity.revokedClient")}<Icon name="x" /></button>}
      <button type="button" className="icon-btn" aria-label={t("activity.refresh")} data-tip={t("common.refresh")} disabled={loading} onClick={() => setReload((n) => n + 1)}><Icon name="refresh" className={loading ? "spin" : undefined} /></button>
    </div>
    {!complete && <p className="banner info compact"><Icon name="alert" />{t("activity.incompleteHistory")}</p>}
    {error && <p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p>}
    {events.length === 0 ? <div className="empty">
      <span className="empty-icon"><Icon name="activity" /></span>
      <h2>{t("activity.emptyTitle")}</h2>
      <p>{filter.q || filter.environment_id || filter.writes_only || filter.errors_only ? t("activity.noMatches") : t("activity.emptyHint")}</p>
    </div> : <section className="panel">
      <table className="activity-table">
        <thead><tr><th aria-label={t("activity.details")} /><th>{t("activity.date")}</th><th>{t("common.environment")}</th><th>{t("clients.name")}</th><th>{t("activity.operation")}</th><th>{t("activity.result")}</th></tr></thead>
        <tbody>{events.map((event, index) => {
          const { method, model } = split(event.operation);
          const outcome = outcomes[event.outcome] ?? { label: event.outcome, tone: "neutral" };
          const key = event.request_id ?? `${event.at}-${index}`;
          const open = expanded.has(key);
          const detailId = `activity-detail-${event.request_id ?? index}`;
          return <Fragment key={key}><tr>
            <td className="activity-toggle"><button type="button" className="icon-btn" aria-label={t(open ? "activity.hideDetails" : "activity.showDetails", { operation: event.operation })} aria-expanded={open} aria-controls={detailId}
              onClick={() => setExpanded(current => { const next = new Set(current); if (next.has(key)) next.delete(key); else next.add(key); return next; })}><Icon name="chevron" style={{ transform: open ? "rotate(90deg)" : undefined }} /></button></td>
            <td className="mono hint">{formatDateTime(new Date(event.at), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit" })}</td>
            <td>{event.environment_name
              ? <button type="button" className="cell-link" data-tip={t("activity.filterBy", { name: event.environment_name })} onClick={() => setFilter({ ...filter, environment_id: event.environment_id })}>{event.environment_name}</button>
              : <span className="hint">{t("activity.deleted")}</span>}</td>
            <td>{event.client_id
              ? <button type="button" className="cell-link" data-tip={t("activity.filterBy", { name: event.client_label ?? t("activity.thisClient") })} onClick={() => { setClientName(event.client_label ?? null); setFilter({ ...filter, client_id: event.client_id! }); }}>{event.client_label ?? <span className="hint">{t("activity.revoked")}</span>}</button>
              : <span className="hint">{t("approvals.local")}</span>}</td>
            <td><span className="row gap-2"><code className={`method tone-${event.write ? "warn" : "info"}`}>{method}</code>{model && <span className="mono truncate">{model}</span>}</span></td>
            <td><span className={`chip chip-tone tone-${outcome.tone}`}>{outcomes[event.outcome] ? t(outcome.label as "activity.outcomes.succeeded") : outcome.label}</span></td>
          </tr>{open && <tr className="activity-detail"><td colSpan={6}>
            <section id={detailId} aria-label={t("activity.detailsFor", { operation: event.operation })} className="activity-details"><Detail event={event} /></section>
          </td></tr>}</Fragment>;
        })}</tbody>
      </table>
      {next !== null && events.length >= MAX_EVENTS && <p className="panel-body hint">{t("activity.limitReached", { count: MAX_EVENTS })}</p>}
      {next !== null && events.length < MAX_EVENTS && <div className="panel-body row justify-end"><button className="btn sm" disabled={loading} onClick={() => void more()}>{loading ? t("common.loading") : t("activity.loadMore")}</button></div>}
    </section>}
  </div></div>;
}
