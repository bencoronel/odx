import { useEffect, useState } from "react";
import { ApprovalDialog } from "@/components/ui/approval-dialog";
import { Icon } from "./Icon";
import Select from "./Select";
import { ipc, type ProxyView } from "./ipc";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type AppError, type ErrorState } from "./appError";
import { usePreventUnload } from "./windowState";

/** Returning the backend placeholder preserves saved credentials. */
export const PROXY_MASK = "••••••••••••";

export type ProxyKind = "http" | "https" | "socks5h";
export type ProxyDraft = { kind: ProxyKind; host: string; port: string; username: string; password: string };
type ProxyMessage = { key: "proxy.saved" | "proxy.disabled"; failed: false } | { error: AppError; failed: true };
export const proxyKindLabels: Record<ProxyKind, string> = { http: "HTTP", https: "HTTPS", socks5h: "SOCKS5" };
const proxyKindOptions = (Object.keys(proxyKindLabels) as ProxyKind[]).map((kind) => ({ value: kind, label: proxyKindLabels[kind] }));
const defaultPorts: Record<ProxyKind, string> = { http: "3128", https: "3128", socks5h: "1080" };

export const emptyProxy = (kind: ProxyKind = "http"): ProxyDraft => ({ kind, host: "", port: defaultPorts[kind], username: "", password: "" });

/** The backend normalizes URLs to `scheme://host:port`. */
export function draftFromView(view: ProxyView): ProxyDraft {
  const match = /^(http|https|socks5h):\/\/(\[[^\]]+\]|[^:/]+):(\d+)$/.exec(view.url);
  const kind = (match?.[1] ?? "http") as ProxyKind;
  return { kind, host: match?.[2] ?? "", port: match?.[3] ?? defaultPorts[kind], username: view.username, password: view.password };
}

export function proxyInput(draft: ProxyDraft): ProxyView {
  const host = draft.host.trim();
  const bracketed = host.includes(":") && !host.startsWith("[") ? `[${host}]` : host;
  return { url: `${draft.kind}://${bracketed}:${draft.port.trim()}`, username: draft.username, password: draft.password };
}

/** Draft validation error, or empty if valid. The backend validates again. */
export function proxyError(draft: ProxyDraft): ErrorState | null {
  const port = Number(draft.port);
  if (!draft.host.trim() || /[\s/?#@]/.test(draft.host.trim())) return { code: "validation.proxy.host_invalid" };
  if (!/^\d+$/.test(draft.port.trim()) || port < 1 || port > 65535) return { code: "validation.proxy.port_invalid" };
  if (draft.password && !draft.username) return { code: "validation.proxy.password_missing_username" };
  if (draft.username !== PROXY_MASK && draft.username.includes(":")) return { code: "validation.proxy.username_invalid" };
  return null;
}

/** Saved proxy credentials arrive as placeholders and remain hidden. */
export function ProxyFields({ idPrefix, value, change, error }: { idPrefix: string; value: ProxyDraft; change: (draft: ProxyDraft) => void;
  /** Shown by the caller in an element with id `${idPrefix}-error`; marks the field it refers to. */
  error?: ErrorState | null }) {
  const { t } = useTranslation();
  const code = error && typeof error === "object" ? error.code : "";
  const field = code.includes("host") ? "host" : code.includes("port") ? "port" : code.includes("username") ? "username" : "";
  const invalid = (name: string) => field === name ? { "aria-invalid": true, "aria-describedby": `${idPrefix}-error` } : {};
  const credential = (key: "username" | "password") => (text: string) =>
    change({ ...value, [key]: value[key] === PROXY_MASK ? text.replaceAll("•", "") : text });
  const masked = (text: string) => text === PROXY_MASK ? "secret-mask" : "";
  const tip = t("proxy.savedSecret");
  return <div className="form-grid">
    <label className="field" htmlFor={`${idPrefix}-host`}><span>{t("proxy.host")}</span>
      <input id={`${idPrefix}-host`} aria-label={t("proxy.hostLabel")} className="mono" required spellCheck={false} {...invalid("host")} value={value.host} placeholder="proxy.example.com" onChange={(e) => change({ ...value, host: e.target.value })} /></label>
    <label className="field" htmlFor={`${idPrefix}-port`}><span>{t("proxy.port")}</span>
      <input id={`${idPrefix}-port`} aria-label={t("proxy.portLabel")} className="mono w-28" required inputMode="numeric" {...invalid("port")} value={value.port} onChange={(e) => change({ ...value, port: e.target.value })} /></label>
    <label className="field" htmlFor={`${idPrefix}-username`}><span>{t("proxy.usernameOptional")}</span>
      <input id={`${idPrefix}-username`} aria-label={t("proxy.usernameLabel")} className={masked(value.username)} {...invalid("username")} autoComplete="off" spellCheck={false} value={value.username} data-tip={value.username === PROXY_MASK ? tip : undefined} onChange={(e) => credential("username")(e.target.value)} /></label>
    <label className="field" htmlFor={`${idPrefix}-password`}><span>{t("proxy.passwordOptional")}</span>
      <input id={`${idPrefix}-password`} aria-label={t("proxy.passwordLabel")} type="password" autoComplete="new-password" value={value.password} data-tip={value.password === PROXY_MASK ? tip : undefined} onChange={(e) => credential("password")(e.target.value)} /></label>
  </div>;
}

export function ProxyKindSelect({ id, value, change, allowNone, className }: { id: string; value: ProxyKind | "none"; change: (kind: ProxyKind | "none") => void; allowNone?: boolean; className?: string }) {
  const { t } = useTranslation();
  const options = allowNone ? [{ value: "none" as const, label: t("proxy.none") }, ...proxyKindOptions] : proxyKindOptions;
  return <Select id={id} className={className} label={t("proxy.kind")} value={value} options={options} onChange={change} />;
}

/** Preserve host and credentials; update the port only if it was the default. */
export function withKind(draft: ProxyDraft, kind: ProxyKind): ProxyDraft {
  return { ...draft, kind, port: draft.port === defaultPorts[draft.kind] ? defaultPorts[kind] : draft.port };
}

const signature = (input: ProxyView | null) => JSON.stringify(input);

/** Saving the global proxy revokes pending approvals in inheriting environments. */
export default function ProxyPanel({ pendingCount = 0, onSaved }: { pendingCount?: number; onSaved?: () => void | Promise<void> }) {
  const { t } = useTranslation();
  const [stored, setStored] = useState<ProxyView | null | undefined>(undefined);
  const [draft, setDraft] = useState<ProxyDraft | null>(null);
  const [loadError, setLoadError] = useState<AppError | null>(null);
  const [message, setMessage] = useState<ProxyMessage | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  usePreventUnload(saving || confirming || (stored !== undefined
    && JSON.stringify(draft) !== JSON.stringify(stored ? draftFromView(stored) : null)));
  const reset = (view: ProxyView | null) => { setStored(view); setDraft(view ? draftFromView(view) : null); };
  useEffect(() => { ipc.proxy().then(reset).catch((e) => setLoadError(normalizeAppError(e))); }, []);
  if (loadError) return <section className="panel"><header className="panel-header"><h3>{t("proxy.title")}</h3></header>
    <p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(loadError)}</p></section>;
  if (stored === undefined) return <section className="panel" aria-busy="true"><header className="panel-header"><h3>{t("proxy.title")}</h3></header></section>;

  const next = draft ? proxyInput(draft) : null;
  const initial = stored ? proxyInput(draftFromView(stored)) : null;
  const changed = signature(next) !== signature(initial);
  const invalid = draft ? proxyError(draft) : null;

  async function save() {
    setSaving(true);
    setMessage(null);
    try {
      await ipc.saveProxy(next);
      reset(await ipc.proxy());
      setMessage({ key: next ? "proxy.saved" : "proxy.disabled", failed: false });
      await onSaved?.();
    } catch (e) {
      setMessage({ error: normalizeAppError(e), failed: true });
      throw e;
    } finally { setSaving(false); }
  }

  return <form className="panel" aria-label={t("proxy.egressLabel")} autoComplete="off" onSubmit={(event) => {
    event.preventDefault();
    if (!changed || saving || invalid) return;
    if (pendingCount > 0) setConfirming(true); else void save().catch(() => undefined);
  }}>
    <header className="panel-header"><h3>{t("proxy.title")}</h3>
      <span className="row gap-2">
        {changed && !saving && <span className="hint">{t("common.unsaved")}</span>}
        <button type="button" className="btn sm" disabled={saving || !changed} onClick={() => { reset(stored); setMessage(null); }}>{t("common.discard")}</button>
        <button type="submit" className="btn sm primary" disabled={saving || !changed || Boolean(invalid)}>{saving ? t("common.saving") : t("common.save")}</button>
      </span>
    </header>
    <fieldset className="panel-body" disabled={saving}>
      <p className="hint">{t("proxy.help")}</p>
      <label className="field" htmlFor="proxy-kind"><span>{t("proxy.type")}</span>
        <ProxyKindSelect id="proxy-kind" className="w-48" allowNone value={draft?.kind ?? "none"} change={(kind) => { setMessage(null); setDraft(kind === "none" ? null : draft ? withKind(draft, kind) : emptyProxy(kind)); }} /></label>
      {draft && <ProxyFields idPrefix="proxy" value={draft} error={changed ? invalid : null} change={(d) => { setMessage(null); setDraft(d); }} />}
      {draft?.kind === "socks5h" && <p className="hint">{t("proxy.socksHint")}</p>}
      {changed && invalid && <p id="proxy-error" className="hint text-danger" role="alert">{formatAppError(invalid)}</p>}
      {message && <p className={`banner ${message.failed ? "danger" : "ok"}`} role={message.failed ? "alert" : "status"}><Icon name={message.failed ? "alert" : "check"} />{message.failed ? formatAppError(message.error) : t(message.key)}</p>}
    </fieldset>
    <ApprovalDialog open={confirming} busy={saving} tone="warn"
      title={t("proxy.saveTitle")}
      description={t("proxy.pendingWarning", { count: pendingCount })}
      confirmLabel={t("settingsTable.saveAndRevoke")}
      onOpenChange={(open) => { if (!open && !saving) setConfirming(false); }}
      onConfirm={async () => { await save(); setConfirming(false); }} />
  </form>;
}
