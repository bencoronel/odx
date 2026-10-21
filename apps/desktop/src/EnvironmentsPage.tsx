import PanelTitle, { useCollapsed } from "./PanelTitle";
import SettingsTable from "./SettingsTable";
import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { ipc, type Environment, type EnvironmentDraft, type EnvironmentProxyInput, type EnvironmentProxyView, type Pending, type ProxyMode, type WriteMode } from "./ipc";
import { ProxyFields, ProxyKindSelect, draftFromView, emptyProxy, proxyError, proxyInput, withKind, type ProxyDraft } from "./ProxyPanel";
import EnvironmentForm, { ProtocolSelect, type FormDraft, connectionComplete, protocolLabels, urlError, type ConnectionValues, type Protocol } from "./EnvironmentForm";
import { ApprovalDialog } from "@/components/ui/approval-dialog";
import { Icon } from "./Icon";
import { ListResizer, useListWidth } from "./ListResizer";
import { EnvIcon } from "./EnvIcon";
import { ModeSelect, modeLabels, modeTone } from "./ModeSelect";
import Select from "./Select";
import { HealthChip, HealthDot, checkHealth, forgetHealth } from "./HealthChip";
import { formatRemaining, useNow } from "./time";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { formatDateTime } from "./i18n/format";
import { useWindowState, usePreventUnload } from "./windowState";
import TagEditor, { sameTag, uniqueTags } from "./TagEditor";
import EnvironmentFilters from "./EnvironmentFilters";

export default function EnvironmentsPage({ envs, pending = [], focusId, refresh, busy, creating, setCreating, save, update, remove, clone, openShell, writableOnly, setWritableOnly, changeMode, draftRef }: {
  envs: Environment[];
  /** Confirm before saving context or headers, which revokes pending approvals. */
  pending?: Pending[];
  /** Environment to select on open, e.g. when coming from an approval. */
  focusId?: string | null;
  refresh: () => Promise<void>;
  busy: boolean;
  creating: boolean;
  setCreating: (creating: boolean) => void;
  save: (draft: EnvironmentDraft, tags: string[]) => Promise<Environment>;
  update: (id: string, draft: EnvironmentDraft) => Promise<Environment>;
  remove: (id: string) => Promise<boolean>;
  clone: (id: string, name: string) => Promise<Environment>;
  openShell: (env: Environment) => void;
  writableOnly: boolean;
  setWritableOnly: (value: boolean) => void;
  changeMode: (env: Environment, mode: WriteMode) => void;
  draftRef?: { current: FormDraft | null };
}) {
  const { t } = useTranslation();
  const [listWidth, setListWidth] = useListWidth("odx.environments.listWidth");
  const [selectedId, setSelectedId] = useWindowState<string | null>("environments.selected", focusId ?? null);
  // Requests from elsewhere (an approval, the command palette) select even when the page is already open.
  // Closing the create form would drop its draft, so a request arriving meanwhile only selects behind it and says so.
  const [focusHint, setFocusHint] = useState<string | null>(null);
  useEffect(() => setFocusHint(null), [creating]);
  useEffect(() => { if (focusId) { setSelectedId(focusId); setFocusHint(focusId); } }, [focusId, setSelectedId]);
  const [filter, setFilter] = useWindowState("environments.filter", "");
  const [tagFilter, setTagFilter] = useWindowState<string[]>("environments.tags", []);
  // Tags differing only in case are one tag, shown with the first spelling.
  const allTags = useMemo(() => uniqueTags(envs.flatMap((env) => env.tags)).sort((a, b) => a.localeCompare(b)), [envs]);
  // Drop selected tags that no environment uses anymore.
  const activeTags = useMemo(() => tagFilter.filter((tag) => allTags.some((other) => sameTag(other, tag))), [tagFilter, allTags]);
  const [rechecking, setRechecking] = useState(false);
  async function recheckAll() {
    setRechecking(true);
    try { await Promise.all(envs.map((env) => checkHealth(env))); } finally { setRechecking(false); }
  }
  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return envs
      .filter((env) => !writableOnly || env.write_mode === "temporary" || env.write_mode === "permanent")
      .filter((env) => activeTags.every((tag) => env.tags.some((other) => sameTag(other, tag))))
      .filter((env) => !q || [env.name, env.url, env.database, env.login, ...env.tags].some((v) => v.toLowerCase().includes(q)))
      .sort((a, b) => a.name.localeCompare(b.name));
  }, [envs, filter, writableOnly, activeTags]);
  const filtered = !!filter.trim() || activeTags.length > 0 || writableOnly;
  function clearFilters() { setFilter(""); setTagFilter([]); setWritableOnly(false); }
  // Filtering never replaces the chosen environment, which would discard its unsaved edits.
  const selected = envs.find((env) => env.id === selectedId) ?? visible[0];
  useEffect(() => { if (selected && selected.id !== selectedId) setSelectedId(selected.id); }, [selected, selectedId, setSelectedId]);
  function select(id: string) { setCreating(false); setSelectedId(id); }
  const search = useRef<HTMLInputElement>(null);

  // Ctrl+F or / focuses the search; Ctrl+N starts a new environment.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.altKey || document.querySelector('[role="dialog"],[role="alertdialog"]')) return;
      // Shift is allowed alone: "/" needs it on layouts such as Spanish (Shift+7).
      const command = event.ctrlKey || event.metaKey;
      if (command && event.shiftKey) return;
      const key = event.key.toLowerCase();
      const typing = event.target instanceof Element && event.target.closest("input, textarea, select, [contenteditable]");
      if ((command && key === "f") || (!command && key === "/" && !typing)) {
        if (!search.current) return;
        event.preventDefault();
        search.current.focus();
        search.current.select();
      } else if (command && key === "n") {
        event.preventDefault();
        if (!busy && !creating) setCreating(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, creating, setCreating]);

  if (envs.length === 0 && !creating) return <div className="empty">
    <span className="empty-icon"><Icon name="server" /></span>
    <h2>{t("environments.emptyTitle")}</h2>
    <p>{t("environments.emptyHint")}</p>
    <div className="row gap-2">
      <button className="btn primary" disabled={busy} onClick={() => setCreating(true)}><Icon name="plus" />{t("environmentForm.createAction")}</button>
    </div>
  </div>;

  return <div className="split" style={{ "--list-w": `${listWidth}px` } as CSSProperties}>
    <aside className="list-pane" aria-label={t("environments.listLabel")}>
      <ListResizer width={listWidth} setWidth={setListWidth} label={t("environments.resizeList")} />
      <div className="list-toolbar">
        <div className="row gap-2">
          <label className="search grow"><Icon name="search" /><input ref={search} type="search" aria-keyshortcuts="Control+F /" placeholder={t("environments.filterPlaceholder")} aria-label={t("environments.filterLabel")} value={filter} onChange={(e) => setFilter(e.target.value)} onKeyDown={(e) => { if (e.key === "Escape" && filter) { e.preventDefault(); setFilter(""); } }} />{!filter && <kbd className="search-kbd" aria-hidden="true">/</kbd>}</label>
          <EnvironmentFilters envs={envs} tags={activeTags} setTags={setTagFilter} writableOnly={writableOnly} setWritableOnly={setWritableOnly} />
          <button type="button" className="btn sm icon-only" disabled={rechecking} onClick={() => void recheckAll()} aria-label={t("environments.checkAll")} data-tip={t("environments.checkAllHint")}><Icon name="refresh" className={rechecking ? "spin" : undefined} /></button>
          <button type="button" className="btn primary sm icon-only" disabled={busy || creating} onClick={() => setCreating(true)} aria-label={t("environments.newEnvironment")} aria-keyshortcuts="Control+N" data-tip={`${t("environments.newEnvironment")} · Ctrl+N`}><Icon name="plus" /></button>
        </div>
        {filtered && <div className="active-filters">
          {activeTags.map((tag) => <button key={tag} type="button" className="chip filter-chip" aria-label={t("environments.removeTagFilter", { tag })} onClick={() => setTagFilter(activeTags.filter((x) => x !== tag))}>{tag}<Icon name="x" /></button>)}
          {writableOnly && <button type="button" className="chip chip-tone tone-warn filter-chip" onClick={() => setWritableOnly(false)} aria-label={t("environments.removeWriteFilter")}><Icon name="pencil" />{t("environments.freeWrites")}<Icon name="x" /></button>}
          <div className="filter-summary"><span role="status">{t("environments.filterCount", { shown: visible.length, total: envs.length })}</span>
            <button type="button" className="btn sm ghost" onClick={clearFilters}>{t("environments.clearFilters")}</button></div>
        </div>}
      </div>
      <ul className="env-list" role="listbox" aria-label={t("environments.environments")} onKeyDown={(e) => {
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        const options = [...e.currentTarget.querySelectorAll<HTMLElement>("[role=option]")];
        const next = options[options.indexOf(document.activeElement as HTMLElement) + (e.key === "ArrowDown" ? 1 : -1)];
        if (!next) return;
        e.preventDefault();
        next.focus();
        select(next.dataset.id!);
      }}>
        {visible.map((env, index) => <li key={env.id} role="option" data-id={env.id} aria-selected={!creating && env.id === selected?.id}
          // One tab stop; arrows move within the list.
          tabIndex={env.id === selected?.id || (index === 0 && !visible.some((v) => v.id === selected?.id)) ? 0 : -1}
          onClick={() => select(env.id)}
          onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); select(env.id); } }}
        >
          <span className="env-avatar"><EnvIcon env={env} /><span className={`status-dot tone-${modeTone[env.write_mode]}`} data-tip={t(modeLabels[env.write_mode] as "mode.readOnly")} /></span>
          <span className="env-list-text"><strong>{env.name}</strong><small>{hostOf(env.url)} · {env.database}</small>
            {env.tags.length > 0 && <span className="env-list-tags">
              {env.tags.slice(0, LIST_TAGS).map((tag) => <span key={tag} className={`chip${activeTags.some((active) => sameTag(active, tag)) ? " match" : ""}`}>{tag}</span>)}
              {env.tags.length > LIST_TAGS && <span className="chip" data-tip={env.tags.slice(LIST_TAGS).join(", ")}>+{env.tags.length - LIST_TAGS}</span>}
            </span>}
          </span>
          <HealthDot env={env} />
        </li>)}
        {visible.length === 0 && <li className="list-empty">{t("environments.noMatches")}
          {filtered && <button type="button" className="btn sm" onClick={clearFilters}>{t("environments.clearFilters")}</button>}</li>}
      </ul>
    </aside>
    <section className="detail-pane">
      {creating && focusHint && <p className="banner info compact" role="status"><Icon name="alert" />{t("environments.focusWhileCreating", { name: envs.find((env) => env.id === focusHint)?.name ?? "" })}</p>}
      {creating ? <EnvironmentForm busy={busy} tagSuggestions={allTags} draftRef={draftRef} cancel={() => setCreating(false)} save={async (draft, tags) => { setSelectedId((await save(draft, tags)).id); }} />
        : selected && <EnvironmentDetail key={selected.id} env={selected} busy={busy} pendingCount={pending.filter((p) => p.environment_id === selected.id).length} envNames={envs.map((e) => e.name)} allTags={allTags}
          refresh={refresh}
          openShell={() => openShell(selected)}
          clone={async (name) => { setSelectedId((await clone(selected.id, name)).id); }}
          update={async (draft) => { await update(selected.id, draft); forgetHealth(selected.id); }}
          remove={() => remove(selected.id).then((ok) => { if (ok) forgetHealth(selected.id); return ok; })}
          changeMode={(mode) => changeMode(selected, mode)} />}
    </section>
  </div>;
}

const LIST_TAGS = 3;
const hostOf = (url: string) => { try { return new URL(url).host; } catch { return url; } };

/** Build a saved environment draft; an empty credential preserves it. Context is edited separately and ignored on update. */
function draftFrom(env: Environment, changes: Partial<EnvironmentDraft>): EnvironmentDraft {
  return { name: env.name, url: env.url, database: env.database, login: env.login, secret: "", protocol: env.protocol as EnvironmentDraft["protocol"], context: env.context, ...changes };
}

function EnvironmentDetail({ env, pendingCount, refresh, busy, envNames, allTags, update, remove, clone, openShell, changeMode }: {
  env: Environment;
  pendingCount: number;
  refresh: () => Promise<void>;
  busy: boolean;
  envNames: string[];
  allTags: string[];
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
  const [tagError, setTagError] = useState<ErrorState>("");
  // Edits build on the latest local tags, not on props that update only after the save round trip.
  const [tags, setTags] = useState(env.tags);
  useEffect(() => setTags(env.tags), [env.tags]);
  const saveTags = (next: string[]) => {
    setTags(next);
    setTagError("");
    ipc.setTags(env.id, next).then(refresh).catch((reason) => { setTags(env.tags); setTagError(normalizeAppError(reason)); });
  };
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
        <TagEditor tags={tags} suggestions={allTags} disabled={busy} collapsed onChange={saveTags} />
        {tagError && <small className="hint text-danger" role="alert">{formatAppError(tagError)}</small>}
      </div>
      <HealthChip env={env} />
      <button className="btn sm icon-only" disabled={busy} onClick={openShell} data-tip={t("environments.openShell")} aria-label={t("environments.openShell")}><Icon name="terminal" /></button>
      <button className="btn sm icon-only" disabled={busy} onClick={() => setCloneName(suggestedName())} data-tip={t("environments.cloneHint")} aria-label={t("environments.cloneEnvironment")}><Icon name="copy" /></button>
      <button className="btn sm icon-only danger-ghost" disabled={busy} onClick={() => setDeleting(true)} data-tip={t("environments.deleteEnvironment")} aria-label={t("common.delete")}><Icon name="trash" /></button>
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
  // The backend refuses to send the saved key to a different server, database or user.
  const retarget = values !== null && (["url", "database", "login"] as const).some((field) => values[field].trim() !== env[field]);
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
      {(!collapsed || values !== null) && <PanelActions busy={busy} editing={values !== null} start={start} cancel={cancel} canSave={changed && values !== null && connectionComplete(values, retarget) && !invalidProxy} />}
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
          ? <input className="inline-input" aria-label={t("environmentForm.apiKey")} type="password" autoComplete="new-password" required={retarget} placeholder={t(retarget ? "environmentForm.newTargetSecretPlaceholder" : "environmentForm.keepSecretPlaceholder")} value={values.secret} onChange={(e) => setValues({ ...values, secret: e.target.value })} />
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
