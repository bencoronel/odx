import PanelTitle, { useCollapsed } from "./PanelTitle";
import SettingsTable from "./SettingsTable";
import { useEffect, useMemo, useState } from "react";
import { ipc, type Environment, type EnvironmentDraft, type EnvironmentProxyInput, type EnvironmentProxyView, type Pending, type ProxyMode, type WriteMode } from "./ipc";
import { ProxyFields, ProxyKindSelect, draftFromView, emptyProxy, proxyError, proxyInput, withKind, type ProxyDraft } from "./ProxyPanel";
import EnvironmentForm, { ProtocolSelect, connectionComplete, protocolLabels, urlError, type ConnectionValues, type Protocol } from "./EnvironmentForm";
import { ApprovalDialog } from "@/components/ui/approval-dialog";
import { Icon } from "./Icon";
import { EnvIcon } from "./EnvIcon";
import { ModeSelect, modeLabels, modeTone } from "./ModeSelect";
import Select from "./Select";
import { HealthChip, HealthDot, checkHealth, forgetHealth } from "./HealthChip";
import { formatRemaining, useNow } from "./time";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { formatDateTime } from "./i18n/format";
import { useWindowState, usePreventUnload } from "./windowState";

export default function EnvironmentsPage({ envs, pending = [], refresh, busy, creating, setCreating, save, update, remove, clone, openShell, writableOnly, setWritableOnly, changeMode }: {
  envs: Environment[];
  /** Confirm before saving context or headers, which revokes pending approvals. */
  pending?: Pending[];
  refresh: () => Promise<void>;
  busy: boolean;
  creating: boolean;
  setCreating: (creating: boolean) => void;
  save: (draft: EnvironmentDraft) => Promise<Environment>;
  update: (id: string, draft: EnvironmentDraft) => Promise<Environment>;
  remove: (id: string) => Promise<boolean>;
  clone: (id: string, name: string) => Promise<Environment>;
  openShell: (env: Environment) => void;
  writableOnly: boolean;
  setWritableOnly: (value: boolean) => void;
  changeMode: (env: Environment, mode: WriteMode) => void;
}) {
  const { t } = useTranslation();
  const [selectedId, setSelectedId] = useWindowState<string | null>("environments.selected", null);
  const [filter, setFilter] = useWindowState("environments.filter", "");
  const [rechecking, setRechecking] = useState(false);
  async function recheckAll() {
    setRechecking(true);
    try { await Promise.all(envs.map((env) => checkHealth(env))); } finally { setRechecking(false); }
  }
  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return envs
      .filter((env) => !writableOnly || env.write_mode === "temporary" || env.write_mode === "permanent")
      .filter((env) => !q || [env.name, env.url, env.database, env.login].some((v) => v.toLowerCase().includes(q)));
  }, [envs, filter, writableOnly]);
  // Keep the detail selection within the filtered list.
  const pool = visible.length ? visible : envs;
  const selected = pool.find((env) => env.id === selectedId) ?? pool[0];
  function select(id: string) { setCreating(false); setSelectedId(id); }

  if (envs.length === 0 && !creating) return <div className="empty">
    <span className="empty-icon"><Icon name="server" /></span>
    <h2>{t("environments.emptyTitle")}</h2>
    <p>{t("environments.emptyHint")}</p>
    <div className="row gap-2">
      <button className="btn primary" disabled={busy} onClick={() => setCreating(true)}><Icon name="plus" />{t("environmentForm.createAction")}</button>
    </div>
  </div>;

  return <div className="split">
    <aside className="list-pane" aria-label={t("environments.listLabel")}>
      <div className="list-toolbar">
        <div className="row gap-2">
          <label className="search grow"><Icon name="search" /><input type="search" placeholder={t("environments.filterPlaceholder")} aria-label={t("environments.filterLabel")} value={filter} onChange={(e) => setFilter(e.target.value)} /></label>
          <button type="button" className="btn sm icon-only" disabled={rechecking} onClick={() => void recheckAll()} aria-label={t("environments.checkAll")} data-tip={t("environments.checkAllHint")}><Icon name="refresh" className={rechecking ? "spin" : undefined} /></button>
          <button type="button" className="btn primary sm icon-only" disabled={busy || creating} onClick={() => setCreating(true)} aria-label={t("environments.newEnvironment")} data-tip={t("environments.newEnvironment")}><Icon name="plus" /></button>
        </div>
        {writableOnly && <button type="button" className="chip chip-tone tone-warn filter-chip" onClick={() => setWritableOnly(false)} aria-label={t("environments.removeWriteFilter")}><Icon name="pencil" />{t("environments.freeWrites")}<Icon name="x" /></button>}
      </div>
      <ul className="env-list" role="listbox" aria-label={t("environments.environments")}>
        {visible.map((env) => <li key={env.id} role="option" aria-selected={!creating && env.id === selected?.id}
          tabIndex={0}
          onClick={() => select(env.id)}
          onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); select(env.id); } }}
        >
          <span className="env-avatar"><EnvIcon env={env} /><span className={`status-dot tone-${modeTone[env.write_mode]}`} data-tip={t(modeLabels[env.write_mode] as "mode.readOnly")} /></span>
          <span className="env-list-text"><strong>{env.name}</strong><small>{env.database}</small></span>
          <HealthDot env={env} />
        </li>)}
        {visible.length === 0 && <li className="list-empty">{t("environments.noMatches")}</li>}
      </ul>
    </aside>
    <section className="detail-pane">
      {creating ? <EnvironmentForm busy={busy} cancel={() => setCreating(false)} save={async (draft) => { setSelectedId((await save(draft)).id); }} />
        : selected && <EnvironmentDetail key={selected.id} env={selected} busy={busy} pendingCount={pending.filter((p) => p.environment_id === selected.id).length} envNames={envs.map((e) => e.name)}
          refresh={refresh}
          openShell={() => openShell(selected)}
          clone={async (name) => { setSelectedId((await clone(selected.id, name)).id); }}
          update={async (draft) => { await update(selected.id, draft); forgetHealth(selected.id); }}
          remove={() => remove(selected.id).then((ok) => { if (ok) forgetHealth(selected.id); return ok; })}
          changeMode={(mode) => changeMode(selected, mode)} />}
    </section>
  </div>;
}

/** Build a saved environment draft; an empty credential preserves it. Context is edited separately and ignored on update. */
function draftFrom(env: Environment, changes: Partial<EnvironmentDraft>): EnvironmentDraft {
  return { name: env.name, url: env.url, database: env.database, login: env.login, secret: "", protocol: env.protocol as EnvironmentDraft["protocol"], context: env.context, ...changes };
}

function EnvironmentDetail({ env, pendingCount, refresh, busy, envNames, update, remove, clone, openShell, changeMode }: {
  env: Environment;
  pendingCount: number;
  refresh: () => Promise<void>;
  busy: boolean;
  envNames: string[];
  clone: (name: string) => Promise<void>;
  openShell: () => void;
  update: (draft: EnvironmentDraft) => Promise<void>;
  remove: () => Promise<boolean>;
  changeMode: (mode: WriteMode) => void;
}) {
  const { t } = useTranslation();
  const until = env.write_mode === "temporary" && env.write_until ? Date.parse(env.write_until) : NaN;
  const now = useNow(Number.isFinite(until));
  const [deleting, setDeleting] = useState(false);
  const [cloneName, setCloneName] = useState<string | null>(null);
  const suggestedName = () => {
    let name = t("environments.copyName", { name: env.name });
    for (let i = 2; envNames.includes(name); i++) name = t("environments.copyNameNumbered", { name: env.name, index: i });
    return name;
  };

  return <div className="detail">
    <header className="detail-header">
      <span className="tile"><EnvIcon env={env} /></span>
      <div className="grow min-w-0">
        <h2 className="row gap-2 min-w-0"><span className="truncate">{env.name}</span>
          <button type="button" className="icon-btn sm open-link" onClick={() => void ipc.openUrl(env.id).catch(() => undefined)} data-tip={t("environments.openBrowserTip", { url: env.url })} aria-label={t("environments.openBrowser")}><Icon name="external" /></button>
        </h2>
      </div>
      <HealthChip env={env} />
      <button className="btn sm icon-only" disabled={busy} onClick={openShell} data-tip={t("environments.openShell")} aria-label={t("environments.openShell")}><Icon name="terminal" /></button>
      <button className="btn sm icon-only" disabled={busy} onClick={() => setCloneName(suggestedName())} data-tip={t("environments.cloneHint")} aria-label={t("environments.cloneEnvironment")}><Icon name="copy" /></button>
      <button className="btn sm danger-ghost" disabled={busy} onClick={() => setDeleting(true)}><Icon name="trash" />{t("common.delete")}</button>
    </header>

    <section className="panel">
      <header className="panel-header"><h3>{t("environments.writeMode")}</h3>
        {Number.isFinite(until) && <span className="row gap-2"><span className="countdown tone-warn" data-tip={t("environments.readOnlyAt", { time: formatDateTime(new Date(until), { hour: "2-digit", minute: "2-digit" }) })}><Icon name="clock" /><time dateTime={env.write_until!}>{formatRemaining(until - now, { hour: t("common.hourShort"), minute: t("common.minuteShort") })}</time></span><button className="btn sm" disabled={busy} onClick={() => changeMode("temporary")}>{t("environments.changeDeadline")}</button></span>}
      </header>
      <div className="panel-body">
        <ModeSelect id={`mode-${env.id}`} value={env.write_mode} disabled={busy} onValueChange={changeMode} />
      </div>
    </section>

    <ConnectionPanel env={env} busy={busy} pendingCount={pendingCount} update={update} />
    <SettingsTable kind="context" environmentId={env.id} pendingCount={pendingCount} onSaved={refresh} />
    <SettingsTable kind="headers" environmentId={env.id} pendingCount={pendingCount} onSaved={() => forgetHealth(env.id)} />

    <ApprovalDialog
      open={cloneName !== null}
      busy={busy}
      title={t("environments.cloneTitle", { name: env.name })}
      description={t("environments.cloneDescription")}
      confirmLabel={t("environments.cloneEnvironment")}
      confirmDisabled={!cloneName?.trim() || envNames.includes(cloneName.trim())}
      onOpenChange={(open) => { if (!open && !busy) setCloneName(null); }}
      onConfirm={async () => { if (cloneName) await clone(cloneName.trim()); setCloneName(null); }}
    >
      <label className="field"><span>{t("environments.copyNameLabel")}</span>
        <input autoFocus value={cloneName ?? ""} maxLength={200} onChange={(e) => setCloneName(e.target.value)} />
        {cloneName && envNames.includes(cloneName.trim()) && <small className="hint text-danger">{t("environments.nameExists")}</small>}
      </label>
    </ApprovalDialog>
    <ApprovalDialog
      open={deleting}
      busy={busy}
      destructive
      title={t("environments.deleteTitle", { name: env.name })}
      description={t("environments.deleteDescription")}
      confirmLabel={t("environments.deleteEnvironment")}
      onOpenChange={(open) => { if (!open && !busy) setDeleting(false); }}
      onConfirm={async () => { if (!(await remove())) throw { code: "app.environment_delete_failed" }; }}
    >
      <dl className="props compact">
        <div><dt>{t("common.database")}</dt><dd className="mono">{env.database}</dd></div>
        <div><dt>URL</dt><dd className="mono">{env.url}</dd></div>
      </dl>
    </ApprovalDialog>
  </div>;
}

/** Show the editing notice beside the title without shifting content. */
function RevokeNotice() {
  const { t } = useTranslation();
  return <span className="title-warn tone-warn" tabIndex={0} aria-label={t("environments.saveWarning")}>
    <Icon name="alert" />
    <span className="tooltip start" role="tooltip"><strong>{t("settingsTable.onSave")}</strong>{t("environments.revokePending")}</span>
  </span>;
}

function PanelActions({ busy, editing, start, cancel, canSave }: { busy: boolean; editing: boolean; start: () => void; cancel: () => void; canSave: boolean }) {
  const { t } = useTranslation();
  return editing ? <span className="row gap-2">
    <button type="button" className="btn sm" disabled={busy} onClick={cancel}>{t("common.cancel")}</button>
    <button type="submit" className="btn sm primary" disabled={busy || !canSave}>{busy ? t("common.saving") : t("common.save")}</button>
  </span> : <button type="button" className="btn sm" disabled={busy} onClick={start}><Icon name="pencil" />{t("common.edit")}</button>;
}

type ProxyState = { mode: ProxyMode; draft: ProxyDraft };
const proxyState = (view: EnvironmentProxyView): ProxyState => ({ mode: view.mode, draft: view.proxy ? draftFromView(view.proxy) : emptyProxy() });
function proxyChoice(state: ProxyState): EnvironmentProxyInput {
  return state.mode === "custom" ? { mode: "custom", ...proxyInput(state.draft) } : { mode: state.mode };
}
function ConnectionPanel({ env, busy, pendingCount, update }: { env: Environment; busy: boolean; pendingCount: number; update: (draft: EnvironmentDraft) => Promise<void> }) {
  const { t } = useTranslation();
  const initial = (): ConnectionValues => ({ name: env.name, url: env.url, database: env.database, login: env.login, secret: "", protocol: env.protocol as Protocol });
  const [values, setValues] = useState<ConnectionValues | null>(null);
  const [proxyView, setProxyView] = useState<EnvironmentProxyView | null>(null);
  const [proxy, setProxy] = useState<ProxyState | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [collapsed, setCollapsed] = useCollapsed("connection");
  const [error, setError] = useState<ErrorState>("");
  usePreventUnload(values !== null || busy);
  useEffect(() => {
    let active = true;
    ipc.environmentProxy(env.id).then((view) => { if (active) setProxyView(view); }).catch((reason) => { if (active) setError(normalizeAppError(reason)); });
    return () => { active = false; };
  }, [env.id]);
  // Apply proxy settings when they arrive, even if editing started earlier.
  useEffect(() => { if (values && !proxy && proxyView) setProxy(proxyState(proxyView)); }, [values, proxy, proxyView]);
  const cancel = () => { setValues(null); setProxy(null); setError(""); };
  const start = () => { setValues(initial()); if (proxyView) setProxy(proxyState(proxyView)); };
  const invalidProxy = proxy?.mode === "custom" ? proxyError(proxy.draft) : null;
  const connectionChanged = values !== null && (
    (["name", "url", "database", "login"] as const).some((field) => values[field].trim() !== env[field])
    || values.protocol !== env.protocol || values.secret !== ""
  );
  const proxyChanged = proxy !== null && proxyView !== null
    && JSON.stringify(proxyChoice(proxy)) !== JSON.stringify(proxyChoice(proxyState(proxyView)));
  const changed = connectionChanged || proxyChanged;

  async function save() {
    if (!values || !changed || busy) return;
    setError("");
    try {
      await update(draftFrom(env, { name: values.name.trim(), url: values.url.trim(), database: values.database.trim(), login: values.login.trim(), secret: values.secret, protocol: values.protocol }));
      if (proxy && proxyChanged) {
        await ipc.saveEnvironmentProxy(env.id, proxyChoice(proxy));
        setProxyView(await ipc.environmentProxy(env.id));
      }
      cancel();
    } catch (reason) { setError(normalizeAppError(reason)); throw reason; }
  }

  return <form className="panel" autoComplete="off" aria-label={t("environmentForm.connection")} onSubmit={(event) => {
    event.preventDefault();
    if (!values || busy || !changed) return;
    const invalidUrl = urlError(values.url) ?? invalidProxy;
    if (invalidUrl) { setError(invalidUrl); return; }
    setError("");
    if (pendingCount > 0) setConfirming(true); else void save().catch(() => undefined);
  }}>
    <header className="panel-header"><PanelTitle title={t("environmentForm.connection")} collapsed={collapsed && !values} toggle={() => setCollapsed(!collapsed)}>{values && <RevokeNotice />}</PanelTitle>
      {(!collapsed || values !== null) && <PanelActions busy={busy} editing={values !== null} start={start} cancel={cancel} canSave={changed && values !== null && connectionComplete(values, false) && !invalidProxy} />}
    </header>
    <ApprovalDialog open={confirming} busy={busy} tone="warn"
      title={t("environments.saveConnection")}
      description={t("environments.saveConnectionWarning", { count: pendingCount })}
      confirmLabel={t("settingsTable.saveAndRevoke")}
      onOpenChange={(open) => { if (!open) setConfirming(false); }}
      onConfirm={save} />
    {/* Keep rows stable; editing changes only the value control. */}
    {(!collapsed || values) && <fieldset disabled={busy} className="props-fieldset">
      <dl className={`props ${values ? "editing" : ""}`}>
        <div><dt><Icon name="pencil" />{t("environmentForm.name")}</dt><dd>{values
          ? <input className="inline-input" aria-label={t("environmentForm.name")} autoFocus required maxLength={200} value={values.name} data-tip={t("environments.nameTip")} onChange={(e) => setValues({ ...values, name: e.target.value })} />
          : env.name}</dd></div>
        <div><dt><Icon name="globe" />{t("common.url")}</dt><dd className="mono">{values
          ? <input className="inline-input mono" aria-label={t("environmentForm.odooUrl")} type="url" required value={values.url} onChange={(e) => setValues({ ...values, url: e.target.value })} />
          : env.url}</dd></div>
        <div><dt><Icon name="database" />{t("common.database")}</dt><dd className="mono">{values
          ? <input className="inline-input mono" aria-label={t("common.database")} required value={values.database} onChange={(e) => setValues({ ...values, database: e.target.value })} />
          : env.database}</dd></div>
        <div><dt><Icon name="user" />{t("environmentForm.user")}</dt><dd>{values
          ? <input className="inline-input" aria-label={t("environmentForm.user")} required autoComplete="off" value={values.login} onChange={(e) => setValues({ ...values, login: e.target.value })} />
          : env.login}</dd></div>
        <div><dt><Icon name="plug" />{t("environmentForm.protocol")}</dt><dd>{values
          ? <ProtocolSelect className="inline-input" value={values.protocol} onChange={(protocol) => setValues({ ...values, protocol })} />
          : <>{protocolLabels[env.protocol] ? t(protocolLabels[env.protocol] as "environmentForm.protocols.legacy_jsonrpc") : env.protocol}{env.protocol === "auto" && env.detected_protocol && protocolLabels[env.detected_protocol] && <> · {t(protocolLabels[env.detected_protocol] as "environmentForm.protocols.legacy_jsonrpc")}</>}</>}</dd></div>
        <div><dt><Icon name="key" />{t("environmentForm.credential")}</dt><dd>{values
          ? <input className="inline-input" aria-label={t("environmentForm.apiKey")} type="password" autoComplete="new-password" placeholder={t("environmentForm.keepSecretPlaceholder")} value={values.secret} onChange={(e) => setValues({ ...values, secret: e.target.value })} />
          : env.has_secret ? <span className="secret-mask" aria-label={t("environmentForm.savedCredential")} data-tip={t("environmentForm.savedCredentialHint")}>••••••••••••</span> : <span className="text-warn">{t("environmentForm.noCredential")}</span>}</dd></div>
        <div><dt><Icon name="shield" />{t("proxy.title")}</dt><dd className="mono">{values && proxy && proxyView
          ? <span className="row gap-2">
            <Select className="inline-input" label={t("environments.environmentProxy")} value={proxy.mode} options={[
              { value: "inherit", label: t("environments.inheritProxy", { proxy: proxyView.inherited ?? t("environments.noGlobalProxy") }) },
              { value: "custom", label: t("environments.ownProxy") },
              { value: "direct", label: t("environments.directProxy") },
            ]} onChange={(mode) => setProxy({ ...proxy, mode })} />
            {proxy.mode === "custom" && <ProxyKindSelect id={`proxy-kind-${env.id}`} className="inline-input" value={proxy.draft.kind} change={(kind) => { if (kind !== "none") setProxy({ ...proxy, draft: withKind(proxy.draft, kind) }); }} />}
          </span>
          : proxyView ? <span data-tip={proxyView.mode === "direct" ? t("environments.directProxyHint") : undefined}>{proxyView.mode === "direct" ? t("environments.proxyDirect") : proxyView.mode === "custom" && proxyView.proxy ? t("environments.proxyOwn", { url: proxyView.proxy.url }) : t("environments.proxyInherit", { proxy: proxyView.inherited ?? t("environments.noGlobalProxy") })}</span> : "…"}</dd></div>
      </dl>
      {values && proxy?.mode === "custom" && <div className="panel-body"><ProxyFields idPrefix={`env-proxy-${env.id}`} value={proxy.draft} change={(draft) => setProxy({ ...proxy, draft })} />
        {proxy.draft.kind === "socks5h" && <p className="hint">{t("proxy.socksHint")}</p>}</div>}
      {values && error && <div className="panel-body panel-foot"><p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p></div>}
    </fieldset>}
  </form>;
}
