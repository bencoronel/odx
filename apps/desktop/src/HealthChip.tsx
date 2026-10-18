import { useEffect, useState } from "react";
import { ipc, type Environment, type Health } from "./ipc";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "./i18n/format";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { protocolLabels } from "./EnvironmentForm";

type Entry = { report?: Health; error?: AppError; at: number; checking?: boolean };
const FRESH_MS = 60_000;
const statusKeys = { ok: "health.connected", unreachable: "health.offline", auth_failed: "health.credentialRejected" } as const;
const networkKeys = { timeout: "health.network.timeoutLabel", dns: "health.network.dnsLabel", tls: "health.network.tlsLabel", refused: "health.network.refusedLabel", unreachable: "health.network.unreachableLabel", reset: "health.network.resetLabel", network: "health.network.networkLabel" } as const;
const networkHelpKeys = { timeout: "health.network.timeout", dns: "health.network.dns", tls: "health.network.tls", refused: "health.network.refused", unreachable: "health.network.unreachable", reset: "health.network.reset", network: "health.network.network" } as const;
type HealthKey = typeof statusKeys[keyof typeof statusKeys] | "health.checking" | "health.notChecked" | "health.httpStatus" | typeof networkKeys[keyof typeof networkKeys] | typeof networkHelpKeys[keyof typeof networkHelpKeys] | "health.offline";

/** Share health results between list and detail by environment and connection. */
const cache = new Map<string, Entry>();
const listeners = new Set<() => void>();
const keyOf = (env: Environment) => `${env.id}|${env.url}|${env.database}|${env.login}|${env.protocol}`;
const notify = () => listeners.forEach((l) => l());

/** Invalidate health results after connection changes. */
export function forgetHealth(environmentId: string) {
  for (const key of cache.keys()) if (key.startsWith(`${environmentId}|`)) cache.delete(key);
  notify();
}

export async function checkHealth(env: Environment) {
  const key = keyOf(env);
  const previous = cache.get(key);
  if (previous?.checking) return;
  cache.set(key, { ...(previous ?? { at: 0 }), checking: true });
  notify();
  let next: Entry;
  try { next = { report: await ipc.check(env.id), at: Date.now() }; }
  catch (reason) { next = { error: normalizeAppError(reason), at: Date.now() }; }
  cache.set(key, next);
  notify();
}

/** With `auto`, check health unless a recent result exists. */
export function useHealth(env: Environment, auto: boolean) {
  const key = keyOf(env);
  const [entry, setEntry] = useState(() => cache.get(key));
  useEffect(() => {
    const update = () => setEntry(cache.get(key));
    listeners.add(update);
    update();
    const cached = cache.get(key);
    if (auto && (!cached || (!cached.checking && Date.now() - cached.at > FRESH_MS))) void checkHealth(env);
    return () => { listeners.delete(update); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, auto]);
  return entry;
}

/** Show latency on success or an error code on failure; `title` has full status. */
function describe(entry: Entry | undefined, t: (key: HealthKey, options?: { status: number }) => string) {
  const report = entry?.report;
  const checking = !!entry?.checking && !report && !entry?.error;
  const tone = checking || !entry ? "neutral" : report?.status === "ok" ? "ok" : report ? "danger" : "warn";
  const title = checking || !entry ? t("health.checking") : report ? t(statusKeys[report.status]) : t("health.notChecked");
  let label = title;
  let detail = "";
  let help = "";
  if (report?.status === "ok") {
    label = `${report.latency_ms} ms`;
    detail = report.server_version ? `Odoo ${report.server_version}` : "";
  } else if (report?.status === "unreachable") {
    const code = report.network_error as keyof typeof networkKeys | undefined;
    label = report.http_status ? `HTTP ${report.http_status}` : code && code in networkKeys ? t(networkKeys[code]) : t("health.offline");
    help = report.http_status ? t("health.httpStatus", { status: report.http_status }) : code && code in networkHelpKeys ? t(networkHelpKeys[code]) : "";
  }
  return { report, checking, tone, title, label, detail, help };
}

/** Compact environment health indicator. */
export function HealthDot({ env }: { env: Environment }) {
  const { t } = useTranslation();
  const entry = useHealth(env, true);
  const { tone, title, label, detail } = describe(entry, t);
  return <span className={`health-dot tone-${tone} ${entry?.checking ? "pulse" : ""}`} data-tip={[title, [label !== title ? label : "", detail].filter(Boolean).join(" · ")].filter(Boolean).join("\n")} aria-label={t("health.connectionLabel", { title, label })} />;
}

export function HealthChip({ env }: { env: Environment }) {
  const { t } = useTranslation();
  const entry = useHealth(env, true);
  const { report, tone, title, label, detail, help } = describe(entry, t);
  const checking = !!entry?.checking;
  const error = report?.error_code
    ? formatAppError({ code: report.error_code, params: report.error_params ?? undefined, detail: report.error ?? undefined })
    : report?.error ?? (entry?.error ? formatAppError(entry.error) : "");
  const when = entry?.at ? formatDateTime(new Date(entry.at), { hour: "2-digit", minute: "2-digit", second: "2-digit" }) : "";

  return <button type="button" className={`health tone-${tone}`} disabled={checking} aria-describedby={`health-tip-${env.id}`} aria-label={t("health.stateLabel", { title, label, detail: detail ? `, ${detail}` : "", retry: t("health.retry") })} onClick={() => void checkHealth(env)}>
    {checking ? <Icon name="refresh" className="spin" /> : <span className="status-dot" />}
    <span>{label}</span>
    {detail && <span className="health-detail">{detail}</span>}
    <span className="tooltip end" role="tooltip" id={`health-tip-${env.id}`}>
      <strong>{title}</strong>
      {(label !== title || detail) && <span className="tip-mono">{[label !== title ? label : "", detail].filter(Boolean).join(" · ")}</span>}
      {report?.protocol && protocolLabels[report.protocol] && <span>{t(protocolLabels[report.protocol] as "environmentForm.protocols.auto")}</span>}
      {help && <span>{help}</span>}
      {error && report?.status !== "ok" && <span className="tip-mono tip-error">{error}</span>}
      {when && <span className="tip-meta">{t("health.checkedAt", { time: when })}</span>}
      <span className="tip-meta">{t("health.retryHint")}</span>
    </span>
  </button>;
}
