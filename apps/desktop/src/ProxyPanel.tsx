import { useEffect, useRef, useState } from "react";
import { ConfirmDialog } from "./ConfirmDialog";
import { Icon } from "./Icon";
import Select from "./Select";
import { ipc, type Environment, type ProxiesView, type ProxyInput, type ProxyTest, type ProxyView } from "./ipc";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type AppError, type ErrorState } from "./appError";
import { usePreventUnload } from "./windowState";

/** Returning the backend placeholder preserves saved credentials. */
export const PROXY_MASK = "••••••••••••";

export type ProxyKind = "http" | "https" | "socks5h";
export type ProxyDraft = { kind: ProxyKind; host: string; port: string; username: string; password: string };
const proxyKindLabels: Record<ProxyKind, string> = { http: "HTTP", https: "HTTPS", socks5h: "SOCKS5" };
const proxyKindOptions = (Object.keys(proxyKindLabels) as ProxyKind[]).map((kind) => ({ value: kind, label: proxyKindLabels[kind] }));
const defaultPorts: Record<ProxyKind, string> = { http: "3128", https: "3128", socks5h: "1080" };

const emptyProxy = (kind: ProxyKind = "http"): ProxyDraft => ({ kind, host: "", port: defaultPorts[kind], username: "", password: "" });

/** The backend normalizes URLs to `scheme://host:port`. */
export function draftFromView(view: Omit<ProxyInput, "name">): ProxyDraft {
  const match = /^(http|https|socks5h):\/\/(\[[^\]]+\]|[^:/]+):(\d+)$/.exec(view.url);
  const kind = (match?.[1] ?? "http") as ProxyKind;
  return { kind, host: match?.[2] ?? "", port: match?.[3] ?? defaultPorts[kind], username: view.username, password: view.password };
}

export function proxyInput(draft: ProxyDraft): Omit<ProxyInput, "name"> {
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
function ProxyFields({ idPrefix, value, change, error }: { idPrefix: string; value: ProxyDraft; change: (draft: ProxyDraft) => void;
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

function ProxyKindSelect({ id, value, change, className }: { id: string; value: ProxyKind; change: (kind: ProxyKind) => void; className?: string }) {
  const { t } = useTranslation();
  return <Select id={id} className={className} label={t("proxy.kind")} value={value} options={proxyKindOptions} onChange={change} />;
}

/** Preserve host and credentials; update the port only if it was the default. */
function withKind(draft: ProxyDraft, kind: ProxyKind): ProxyDraft {
  return { ...draft, kind, port: draft.port === defaultPorts[draft.kind] ? defaultPorts[kind] : draft.port };
}

type Editing = { id: string | null; name: string; draft: ProxyDraft };
type TestState = { running: true } | { result: ProxyTest } | { error: AppError };

/** Reach one environment's server through the proxy being edited, before saving it. */
function ProxyTester({ id, draft, invalid }: { id: string | null; draft: ProxyDraft; invalid: boolean }) {
  const { t } = useTranslation();
  const [envs, setEnvs] = useState<Environment[] | null>(null);
  const [target, setTarget] = useState("");
  const [state, setState] = useState<TestState | null>(null);
  useEffect(() => { ipc.environments().then((list) => { setEnvs(list); setTarget((current) => current || list[0]?.id || ""); }, () => setEnvs([])); }, []);
  // Any edit makes the last result stale.
  useEffect(() => setState(null), [draft, target]);
  const latest = useRef("");
  if (!envs?.length) return envs ? <p className="hint">{t("proxy.testNeedsEnvironment")}</p> : null;
  const url = envs.find((env) => env.id === target)?.url ?? envs[0].url;
  latest.current = JSON.stringify([draft, url]);
  const run = () => {
    setState({ running: true });
    // A result for a proxy or target edited since is dropped (the edit already cleared the state).
    const asked = JSON.stringify([draft, url]);
    const current = () => latest.current === asked;
    ipc.testProxy(id, { name: "", ...proxyInput(draft) }, url).then((result) => { if (current()) setState({ result }); }, (e) => { if (current()) setState({ error: normalizeAppError(e) }); });
  };
  return <div className="proxy-test">
    <span className="row gap-2">
      <Select className="proxy-test-target" label={t("proxy.testTarget")} value={target} options={envs.map((env) => ({ value: env.id, label: env.name }))} onChange={setTarget} />
      <button type="button" className="btn sm" disabled={invalid || (state !== null && "running" in state)} onClick={run}>
        <Icon name={state && "running" in state ? "refresh" : "plug"} className={state && "running" in state ? "spin" : undefined} />{t("proxy.test")}</button>
    </span>
    {state && "result" in state && <p className="text-ok proxy-test-result" role="status"><Icon name="check" />{state.result.server_version
      ? t("proxy.testOkVersion", { latency: state.result.latency_ms, version: state.result.server_version })
      : t("proxy.testOk", { latency: state.result.latency_ms })}</p>}
    {state && "error" in state && <p className="text-danger proxy-test-result" role="alert"><Icon name="alert" />{formatAppError(state.error)}</p>}
  </div>;
}

/** Named proxies and the default one. Changes that move an environment to another proxy revoke its pending approvals. */
export default function ProxyPanel({ pendingCount = 0, onSaved }: { pendingCount?: number; onSaved?: () => void | Promise<void> }) {
  const { t } = useTranslation();
  const [view, setView] = useState<ProxiesView | null>(null);
  const [editing, setEditing] = useState<Editing | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirm, setConfirm] = useState<(() => Promise<void>) | null>(null);
  const [deleting, setDeleting] = useState<ProxyView | null>(null);
  usePreventUnload(saving || confirm !== null || editing !== null);
  useEffect(() => { ipc.proxies().then(setView).catch((e) => setError(normalizeAppError(e))); }, []);
  if (!view) return <section className="panel" aria-busy={!error}><header className="panel-header"><h3>{t("proxy.list")}</h3></header>
    {error && <div className="panel-body"><p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p></div>}</section>;

  /** Run a change, asking first when it may revoke pending approvals. */
  const run = (action: () => Promise<void>, revokes = true) => {
    const go = async () => {
      setSaving(true);
      setError(null);
      try { await action(); setView(await ipc.proxies()); await onSaved?.(); }
      catch (e) { setError(normalizeAppError(e)); throw e; }
      finally { setSaving(false); }
    };
    if (revokes && pendingCount > 0) setConfirm(() => go); else void go().catch(() => undefined);
  };
  const invalid = editing ? proxyError(editing.draft) ?? (editing.name.trim() ? null : { code: "validation.proxy.name_invalid" }) : null;
  const save = () => {
    if (!editing || invalid) return;
    const { id, name, draft } = editing;
    run(async () => { await ipc.saveProxy(id, { name: name.trim(), ...proxyInput(draft) }); setEditing(null); }, id !== null);
  };
  const add = () => { setError(null); setEditing({ id: null, name: "", draft: emptyProxy() }); };
  const busy = saving || editing !== null;
  const proxies = [...view.proxies].sort((a, b) => a.name.localeCompare(b.name));
  const title = editing?.id ? t("proxy.editTitle", { name: editing.name }) : t("proxy.newTitle");
  const editor = editing && <form className="proxy-editor" aria-label={title} autoComplete="off" onSubmit={(e) => { e.preventDefault(); save(); }}>
    <fieldset disabled={saving} className="proxy-body">
      <strong>{title}</strong>
      <div className="form-grid">
        <label className="field"><span>{t("proxy.name")}</span><input autoFocus required maxLength={100} value={editing.name} placeholder={t("proxy.namePlaceholder")} onChange={(e) => setEditing({ ...editing, name: e.target.value })} /></label>
        <label className="field" htmlFor="proxy-kind"><span>{t("proxy.type")}</span>
          <ProxyKindSelect id="proxy-kind" value={editing.draft.kind} change={(kind) => setEditing({ ...editing, draft: withKind(editing.draft, kind) })} /></label>
      </div>
      <ProxyFields idPrefix="proxy" value={editing.draft} error={invalid} change={(draft) => setEditing({ ...editing, draft })} />
      {invalid && <p id="proxy-error" className="hint text-danger">{formatAppError(invalid)}</p>}
      <div className="proxy-editor-foot">
        <ProxyTester id={editing.id} draft={editing.draft} invalid={Boolean(proxyError(editing.draft))} />
        <span className="grow" />
        <button type="button" className="btn sm" onClick={() => { setEditing(null); setError(null); }}>{t("common.cancel")}</button>
        <button type="submit" className="btn sm primary" disabled={saving || Boolean(invalid)}>{saving ? t("common.saving") : editing.id ? t("common.save") : t("proxy.create")}</button>
      </div>
    </fieldset>
  </form>;

  return <section className="panel" aria-label={t("proxy.list")}>
    <header className="panel-header"><h3>{t("proxy.list")}</h3>
      {proxies.length > 0 && <button type="button" className="btn sm" disabled={busy} onClick={add}><Icon name="plus" />{t("proxy.add")}</button>}
    </header>
    {proxies.length === 0 && !editing && <div className="empty proxy-empty">
      <span className="empty-icon"><Icon name="shield" /></span>
      <p>{t("proxy.empty")}</p>
      <div className="row"><button type="button" className="btn sm primary" onClick={add}><Icon name="plus" />{t("proxy.add")}</button></div>
    </div>}
    {proxies.length > 0 && <ul className="proxy-list" aria-label={t("proxy.list")}>{proxies.map((p) => {
      if (editing?.id === p.id) return <li key={p.id}>{editor}</li>;
      const { kind, host, port } = draftFromView(p);
      const isDefault = view.default === p.id;
      return <li key={p.id} className="setting-row">
        <span className="tile"><Icon name={kind === "socks5h" ? "shield" : "globe"} /></span>
        <div>
          <strong className="row gap-2">{p.name}{isDefault && <span className="chip chip-tone tone-info">{t("proxy.isDefault")}</span>}</strong>
          <p className="hint proxy-meta"><span className="mono">{proxyKindLabels[kind]} · {host}:{port}</span>
            {p.username && <span><Icon name="key" />{t("proxy.withCredentials")}</span>}
            <span>{p.environments ? t("proxy.usedBy", { count: p.environments }) : t("proxy.unused")}</span></p>
        </div>
        <span className="row gap-2">
          <button type="button" className="btn sm ghost" disabled={busy} onClick={() => run(() => ipc.setDefaultProxy(isDefault ? null : p.id))}>{isDefault ? t("proxy.clearDefault") : t("proxy.makeDefault")}</button>
          <button type="button" className="icon-btn sm" disabled={busy} aria-label={t("proxy.edit", { name: p.name })} data-tip={t("common.edit")} onClick={() => { setError(null); setEditing({ id: p.id, name: p.name, draft: draftFromView(p) }); }}><Icon name="pencil" /></button>
          <button type="button" className="icon-btn sm" disabled={busy || p.environments > 0} aria-label={t("proxy.delete", { name: p.name })} data-tip={p.environments ? t("proxy.inUseTip", { count: p.environments }) : t("common.delete")} onClick={() => setDeleting(p)}><Icon name="trash" /></button>
        </span>
      </li>;
    })}</ul>}
    {editing?.id === null && editor}
    {error && <div className="panel-body"><p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p></div>}
    <ConfirmDialog open={confirm !== null} busy={saving} tone="warn"
      title={t("proxy.saveTitle")}
      description={t("proxy.pendingWarning", { count: pendingCount })}
      confirmLabel={t("settingsTable.saveAndRevoke")}
      onOpenChange={(open) => { if (!open && !saving) setConfirm(null); }}
      onConfirm={async () => { if (confirm) await confirm(); setConfirm(null); }} />
    <ConfirmDialog open={deleting !== null} busy={saving} destructive
      title={t("proxy.deleteTitle")}
      description={deleting ? `${t("proxy.deleteWarning", { name: deleting.name })}${view.default === deleting.id ? ` ${t("proxy.deleteDefaultWarning")}` : ""}` : ""}
      confirmLabel={t("common.delete")}
      onOpenChange={(open) => { if (!open && !saving) setDeleting(null); }}
      onConfirm={async () => { const target = deleting; setDeleting(null); if (target) run(() => ipc.deleteProxy(target.id), view.default === target.id); }} />
  </section>;
}
