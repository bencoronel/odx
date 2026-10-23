import PanelTitle, { useCollapsed } from "./PanelTitle";
import SettingsTable from "./SettingsTable";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { ipc, type Environment, type EnvironmentDraft, type EnvironmentProxyInput, type EnvironmentProxyView, type Pending, type ProxiesView, type WriteMode } from "./ipc";
import EnvironmentForm, { ProtocolSelect, type FormDraft, connectionComplete, protocolLabels, proxyChoice, proxyOptions, urlError, type ConnectionValues, type Protocol } from "./EnvironmentForm";
import { ApprovalDialog } from "@/components/ui/approval-dialog";
import { Icon, type IconName } from "./Icon";
import { ListResizer, useListWidth } from "./ListResizer";
import { EnvIcon } from "./EnvIcon";
import SearchField from "./SearchField";
import { ModeSelect, modeIcons, modeLabels, modeShortLabels, modeTone } from "./ModeSelect";
import Select from "./Select";
import { HealthChip, HealthDot, checkHealth, forgetHealth } from "./HealthChip";
import { formatRemaining, useNow } from "./time";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { formatDateTime } from "./i18n/format";
import { loadTagColors, tagColor, tagTone, useTagColors } from "./tagColors";
import { useWindowState, usePreventUnload } from "./windowState";
import TagEditor, { sameTag, uniqueTags } from "./TagEditor";
import EnvironmentFilters from "./EnvironmentFilters";
import { groupByHost, hostOf } from "./hosts";
import { useDismiss } from "./ContextMenu";

export default function EnvironmentsPage({ envs, pending = [], focusId, refresh, busy, creating, setCreating, save, update, remove, clone, openShell, changeMode, draftRef }: {
  envs: Environment[];
  /** Confirm before saving context or headers, which revokes pending approvals. */
  pending?: Pending[];
  /** Environment to select on open, e.g. when coming from an approval. */
  focusId?: string | null;
  refresh: () => Promise<void>;
  busy: boolean;
  creating: boolean;
  setCreating: (creating: boolean) => void;
  save: (draft: EnvironmentDraft, tags: string[], proxy: EnvironmentProxyInput) => Promise<Environment>;
  update: (id: string, draft: EnvironmentDraft) => Promise<Environment>;
  remove: (id: string) => Promise<boolean>;
  clone: (id: string, name: string) => Promise<Environment>;
  openShell: (env: Environment) => void;
  changeMode: (env: Environment, mode: WriteMode) => void;
  draftRef?: { current: FormDraft | null };
}) {
  const { t } = useTranslation();
  const [listWidth, setListWidth] = useListWidth("odx.environments.listWidth");
  const tagColors = useTagColors();
  useEffect(() => { loadTagColors().catch(() => undefined); }, []);
  const [listMenu, setListMenu] = useState<{ env: Environment; x: number; y: number } | null>(null);
  const [detailAction, setDetailAction] = useState<DetailAction | null>(null);
  const [selectedId, setSelectedId] = useWindowState<string | null>("environments.selected", focusId ?? null);
  // Requests from elsewhere (an approval, the command palette) select even when the page is already open.
  // Closing the create form would drop its draft, so a request arriving meanwhile only selects behind it and says so.
  const [focusHint, setFocusHint] = useState<string | null>(null);
  useEffect(() => setFocusHint(null), [creating]);
  useEffect(() => { if (focusId) { setSelectedId(focusId); setFocusHint(focusId); } }, [focusId, setSelectedId]);
  const [filter, setFilter] = useWindowState("environments.filter", "");
  const [tagFilter, setTagFilter] = useWindowState<string[]>("environments.tags", []);
  const [modeFilter, setModeFilter] = useWindowState<WriteMode[]>("environments.modes", []);
  // Tags differing only in case are one tag, shown with the first spelling.
  const allTags = useMemo(() => uniqueTags(envs.flatMap((env) => env.tags)).sort((a, b) => a.localeCompare(b)), [envs]);
  // Drop selected tags that no environment uses anymore.
  const activeTags = useMemo(() => tagFilter.filter((tag) => allTags.some((other) => sameTag(other, tag))), [tagFilter, allTags]);
  const [rechecking, setRechecking] = useState(false);
  async function recheckAll() {
    setRechecking(true);
    try { await Promise.all(envs.map((env) => checkHealth(env))); } finally { setRechecking(false); }
  }
  // "#text" searches tags to add as filters instead of filtering the list.
  const tagQuery = filter.startsWith("#") ? filter.slice(1).trim().toLowerCase() : null;
  const tagMatches = tagQuery === null ? [] : allTags.filter((tag) => !activeTags.some((active) => sameTag(active, tag)) && tag.toLowerCase().includes(tagQuery));
  const [tagIndex, setTagIndex] = useState(0);
  const addTagFilter = (tag: string) => { setTagFilter([...activeTags, tag]); setFilter(""); setTagIndex(0); search.current?.focus(); };
  const visible = useMemo(() => {
    const q = filter.startsWith("#") ? "" : filter.trim().toLowerCase();
    return envs
      .filter((env) => !modeFilter.length || modeFilter.includes(env.write_mode))
      .filter((env) => activeTags.every((tag) => env.tags.some((other) => sameTag(other, tag))))
      .filter((env) => !q || [env.name, env.url, env.database, env.login, ...env.tags].some((v) => v.toLowerCase().includes(q)))
      .sort((a, b) => a.name.localeCompare(b.name));
  }, [envs, filter, modeFilter, activeTags]);
  const filtered = (!!filter.trim() && !filter.startsWith("#")) || activeTags.length > 0 || modeFilter.length > 0;
  function clearFilters() { setFilter(""); setTagFilter([]); setModeFilter([]); }
  // Filtering never replaces the chosen environment, which would discard its unsaved edits.
  const selected = envs.find((env) => env.id === selectedId) ?? visible[0];
  useEffect(() => { if (selected && selected.id !== selectedId) setSelectedId(selected.id); }, [selected, selectedId, setSelectedId]);
  function select(id: string) { setCreating(false); setSelectedId(id); }
  const search = useRef<HTMLInputElement>(null);

  // Environments on one host (an Odoo server with several databases) form a group; a lone one stays a plain row.
  const groups = useMemo(() => groupByHost(visible), [visible]);
  const [collapsed, setCollapsed] = useWindowState<string[]>("environments.collapsed", []);
  // Filtering shows every match, also inside collapsed groups.
  const shown = groups.flatMap(([host, list]) => list.length > 1 && !filtered && collapsed.includes(host) ? [] : list);
  // Another database on the same server: URL, protocol and proxy carry over. The login is usually the same but stays editable;
  // the API key belongs to the database, so it is never copied. Tags often differ per database (client, test), so they start empty.
  async function addDatabase(env: Environment) {
    if (creating) return;
    const proxy = await ipc.environmentProxy(env.id).then(proxyValue, () => "inherit");
    if (draftRef) draftRef.current = { values: { name: "", url: env.url, database: "", login: env.login, secret: "", protocol: env.protocol as Protocol }, tags: [], proxy };
    setCreating(true);
  }
  const row = (env: Environment, grouped: boolean) => <li key={env.id} role="option" data-id={env.id} aria-selected={!creating && env.id === selected?.id}
    // One tab stop; arrows move within the list.
    tabIndex={env.id === selected?.id || (env.id === shown[0]?.id && !shown.some((v) => v.id === selected?.id)) ? 0 : -1}
    onClick={() => select(env.id)}
    onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); select(env.id); } }}
    // Our own menu instead of the app-wide text menu.
    onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); select(env.id); setListMenu({ env, x: e.clientX, y: e.clientY }); }}
  >
    <span className="env-avatar"><EnvIcon env={env} /><span className={`status-dot tone-${modeTone[env.write_mode]}`} data-tip={t(modeLabels[env.write_mode] as "mode.readOnly")} /></span>
    {/* Tags share the name's line so every row has the same height. */}
    <span className="env-list-text"><span className="env-list-title"><strong>{env.name}</strong>
      {env.tags.length > 0 && <span className="env-list-tags">
        {env.tags.slice(0, LIST_TAGS).map((tag) => <span key={tag} className={`chip${tagTone(tagColors, tag)}${activeTags.some((active) => sameTag(active, tag)) ? " match" : ""}`}
          data-tip={t("environments.filterByTag")} onClick={(e) => { if (activeTags.some((active) => sameTag(active, tag))) return; e.stopPropagation(); setTagFilter([...activeTags, tag]); }}>{tag}</span>)}
        {env.tags.length > LIST_TAGS && <span className="chip" data-tip={env.tags.slice(LIST_TAGS).join(", ")}>+{env.tags.length - LIST_TAGS}</span>}
      </span>}</span>
      {/* In a group the header names the host; the login tells apart two users of one database. */}
      <small>{grouped ? `${env.database} · ${env.login}` : `${hostOf(env.url)} · ${env.database}`}</small>
    </span>
    <HealthDot env={env} />
  </li>;

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
    <div className="row gap-2">
      <button className="btn primary" disabled={busy} onClick={() => setCreating(true)}><Icon name="plus" />{t("environmentForm.createAction")}</button>
    </div>
  </div>;

  return <div className="split" style={{ "--list-w": `${listWidth}px` } as CSSProperties}>
    <aside className="list-pane" aria-label={t("environments.listLabel")}>
      <ListResizer width={listWidth} setWidth={setListWidth} label={t("environments.resizeList")} />
      <div className="list-toolbar">
        <div className="row gap-2">
          {/* Active filters are chips inside the field, so filtering never adds rows above the list. */}
          <SearchField className="grow" inputRef={search} value={filter} label={t("environments.filterLabel")} shortcut
            placeholder={activeTags.length || modeFilter.length ? "" : t("environments.filterPlaceholder")}
            filtered={filtered} onClear={clearFilters} clearLabel={t("environments.clearFilters")} onChange={(value) => { setFilter(value); setTagIndex(0); }}
            count={{ shown: visible.length, total: envs.length, text: t("environments.filterCount", { shown: visible.length, total: envs.length }) }}
            chips={activeTags.length > 0 || modeFilter.length > 0 ? <>
              {activeTags.map((tag) => <span key={tag} className={`chip token${tagTone(tagColors, tag)}`}>{tag}
                <button type="button" aria-label={t("environments.removeTagFilter", { tag })} onClick={() => setTagFilter(activeTags.filter((x) => x !== tag))}><Icon name="x" /></button></span>)}
              {modeFilter.map((mode) => <span key={mode} className={`chip token chip-tone tone-${modeTone[mode]}`}><Icon name={modeIcons[mode]} />{t(modeShortLabels[mode] as "mode.readOnlyShort")}
                <button type="button" aria-label={t("environments.removeModeFilter", { mode: t(modeLabels[mode] as "mode.readOnly") })} onClick={() => setModeFilter(modeFilter.filter((m) => m !== mode))}><Icon name="x" /></button></span>)}
            </> : undefined}
            inputProps={{ "aria-keyshortcuts": "Control+F /", role: "combobox", "aria-expanded": tagMatches.length > 0, "aria-controls": "tag-suggestions", "aria-autocomplete": "list", onKeyDown: (e) => {
              if (tagMatches.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) { e.preventDefault(); setTagIndex((i) => (i + (e.key === "ArrowDown" ? 1 : tagMatches.length - 1)) % tagMatches.length); }
              else if (tagMatches.length && e.key === "Enter") { e.preventDefault(); addTagFilter(tagMatches[Math.min(tagIndex, tagMatches.length - 1)]); }
              else if (e.key === "Backspace" && !filter && (modeFilter.length || activeTags.length)) { if (modeFilter.length) setModeFilter(modeFilter.slice(0, -1)); else setTagFilter(activeTags.slice(0, -1)); }
            } }} />
          <EnvironmentFilters envs={envs} tags={activeTags} setTags={setTagFilter} modes={modeFilter} setModes={setModeFilter} />
          <button type="button" className="btn sm icon-only" disabled={rechecking} onClick={() => void recheckAll()} aria-label={t("environments.checkAll")} data-tip={t("environments.checkAll")}><Icon name="refresh" className={rechecking ? "spin" : undefined} /></button>
          <button type="button" className="btn primary sm icon-only" disabled={busy || creating} onClick={() => setCreating(true)} aria-label={t("environments.newEnvironment")} aria-keyshortcuts="Control+N" data-tip={`${t("environments.newEnvironment")} · Ctrl+N`}><Icon name="plus" /></button>
        </div>
        {tagQuery !== null && <ul id="tag-suggestions" className="token-suggestions" role="listbox" aria-label={t("environments.tagFilter")}>
          {tagMatches.map((tag, i) => <li key={tag} role="option" aria-selected={i === Math.min(tagIndex, tagMatches.length - 1)} onMouseDown={(e) => { e.preventDefault(); addTagFilter(tag); }}>
            <span className={`tag-dot tone-${tagColor(tagColors, tag)}`} aria-hidden="true" />{tag}</li>)}
          {tagMatches.length === 0 && <li className="hint">{allTags.length ? t("environments.noMatches") : t("environments.noTagsYet")}</li>}
        </ul>}
      </div>
      <ul className="env-list" role="listbox" aria-label={t("environments.environments")} aria-keyshortcuts="Delete F2 Control+D" onKeyDown={(e) => {
        // Desktop list keys act on the focused environment: Delete, F2 to rename, Ctrl+D to duplicate.
        const command = e.ctrlKey || e.metaKey;
        const onOption = (e.target as HTMLElement).getAttribute("role") === "option";
        const action: DetailAction | null = !onOption || e.altKey || e.shiftKey || creating ? null
          : e.key === "Delete" && !command ? "delete" : e.key === "F2" && !command ? "rename" : command && e.key.toLowerCase() === "d" ? "clone" : null;
        if (action) { e.preventDefault(); setDetailAction(action); return; }
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        const options = [...e.currentTarget.querySelectorAll<HTMLElement>("[role=option]")];
        const next = options[options.indexOf(document.activeElement as HTMLElement) + (e.key === "ArrowDown" ? 1 : -1)];
        if (!next) return;
        e.preventDefault();
        next.focus();
        select(next.dataset.id!);
      }}>
        {groups.map(([host, list]) => list.length === 1 ? row(list[0], false) : <li key={host} role="group" aria-label={host} className="env-group">
          <div className="env-group-header">
            <button type="button" className="panel-toggle" aria-expanded={filtered || !collapsed.includes(host)} disabled={filtered}
              onClick={() => setCollapsed(collapsed.includes(host) ? collapsed.filter((h) => h !== host) : [...collapsed, host])}>
              <Icon name="chevron" /><EnvIcon env={list[0]} /><span className="truncate">{host}</span>
            </button>
            <span className="env-group-count">{list.length}</span>
            <button type="button" className="icon-btn sm" disabled={busy || creating} onClick={() => void addDatabase(list[0])}
              aria-label={t("environments.addDatabase")} data-tip={t("environments.addDatabase")}><Icon name="plus" /></button>
          </div>
          {(filtered || !collapsed.includes(host)) && <ul role="none">{list.map((env) => row(env, true))}</ul>}
        </li>)}
        {visible.length === 0 && envs.length > 0 && <li className="list-empty">
          <span className="list-empty-icon"><Icon name="search" /></span>
          <strong>{t("environments.noEnvironmentMatches")}</strong>
          {filtered && <button type="button" className="btn sm" onClick={clearFilters}><Icon name="x" />{t("environments.clearFilters")}</button>}</li>}
      </ul>
    </aside>
    {listMenu && <FloatingMenu x={listMenu.x} y={listMenu.y} label={listMenu.env.name} close={() => setListMenu(null)} items={[
      { label: t("environments.openShell"), icon: "terminal", run: () => openShell(listMenu.env) },
      { label: t("environments.cloneEnvironment"), icon: "copy", run: () => setDetailAction("clone") },
      ...creating ? [] : [{ label: t("environments.addDatabase"), icon: "database" as const, run: () => void addDatabase(listMenu.env) }],
      { label: t("environments.deleteEnvironment"), icon: "trash", danger: true, run: () => setDetailAction("delete") },
    ]} />}
    <section className="detail-pane">
      {creating && focusHint && <p className="banner info compact" role="status"><Icon name="alert" />{t("environments.focusWhileCreating", { name: envs.find((env) => env.id === focusHint)?.name ?? "" })}</p>}
      {creating ? <EnvironmentForm busy={busy} tagSuggestions={allTags} draftRef={draftRef} cancel={() => setCreating(false)} save={async (draft, tags, proxy) => { setSelectedId((await save(draft, tags, proxy)).id); }} />
        : selected && <EnvironmentDetail key={selected.id} env={selected} action={detailAction} actionHandled={() => setDetailAction(null)} busy={busy} pendingCount={pending.filter((p) => p.environment_id === selected.id).length} envNames={envs.map((e) => e.name)} allTags={allTags}
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

/** Build a saved environment draft; an empty credential preserves it. Context is edited separately and ignored on update. */
function draftFrom(env: Environment, changes: Partial<EnvironmentDraft>): EnvironmentDraft {
  return { name: env.name, url: env.url, database: env.database, login: env.login, secret: "", protocol: env.protocol as EnvironmentDraft["protocol"], context: env.context, ...changes };
}

function EnvironmentDetail({ env, action, actionHandled, pendingCount, refresh, busy, envNames, allTags, update, remove, clone, openShell, changeMode }: {
  env: Environment;
  /** Requested from the list's context menu or keys. */
  action?: DetailAction | null;
  actionHandled?: () => void;
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
  const [renaming, setRenaming] = useState(0);
  const [tagError, setTagError] = useState<ErrorState>("");
  // Edits build on the latest local tags, not on props that update only after the save round trip.
  const [tags, setTags] = useState(env.tags);
  useEffect(() => setTags(env.tags), [env.tags]);
  const saveTags = (next: string[]) => {
    setTags(next);
    setTagError("");
    ipc.setTags(env.id, next).then(refresh).catch((reason) => { setTags(env.tags); setTagError(normalizeAppError(reason)); });
  };
  useEffect(() => {
    if (!action) return;
    if (action === "clone") setCloneName(suggestedName());
    else if (action === "rename") setRenaming((n) => n + 1);
    else setDeleting(true);
    actionHandled?.();
  });
  const suggestedName = () => {
    let name = t("environments.copyName", { name: env.name });
    for (let i = 2; envNames.includes(name); i++) name = t("environments.copyNameNumbered", { name: env.name, index: i });
    return name;
  };

  return <div className="detail">
    <header className="detail-header">
      <span className="tile"><EnvIcon env={env} /></span>
      <div className="grow min-w-0 detail-title">
        <h2 className="row gap-2 min-w-0"><span className="truncate">{env.name}</span>
          <button type="button" className="icon-btn sm open-link" onClick={() => void ipc.openUrl(env.id).catch(() => undefined)} data-tip={t("environments.openBrowserTip", { url: env.url })} aria-label={t("environments.openBrowser")}><Icon name="external" /></button>
        </h2>
        <p className="hint mono truncate detail-subtitle">{hostOf(env.url)} · {env.database} · {env.login}</p>
        <TagEditor tags={tags} suggestions={allTags} disabled={busy} collapsed onChange={saveTags} />
        {tagError && <small className="hint text-danger" role="alert">{formatAppError(tagError)}</small>}
      </div>
      <HealthChip env={env} />
      <button className="btn sm" disabled={busy} onClick={openShell}><Icon name="terminal" />{t("environments.openShell")}</button>
      <MoreMenu disabled={busy} items={[
        { label: t("environments.cloneEnvironment"), icon: "copy", run: () => setCloneName(suggestedName()) },
        { label: t("environments.deleteEnvironment"), icon: "trash", danger: true, run: () => setDeleting(true) },
      ]} />
    </header>

    <section className="panel">
      <header className="panel-header"><h3>{t("environments.writeMode")}</h3>
        {Number.isFinite(until) && <span className="row gap-2"><span className="countdown tone-warn" data-tip={t("environments.readOnlyAt", { time: formatDateTime(new Date(until), { hour: "2-digit", minute: "2-digit" }) })}><Icon name="clock" /><time dateTime={env.write_until!}>{formatRemaining(until - now, { hour: t("common.hourShort"), minute: t("common.minuteShort") })}</time></span><button className="btn sm" disabled={busy} onClick={() => changeMode("temporary")}>{t("environments.changeDeadline")}</button></span>}
      </header>
      <div className="panel-body">
        <ModeSelect id={`mode-${env.id}`} value={env.write_mode} disabled={busy} onValueChange={changeMode} />
      </div>
    </section>

    <ConnectionPanel env={env} busy={busy} pendingCount={pendingCount} update={update} renaming={renaming} />
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

type DetailAction = "clone" | "delete" | "rename";
type MenuItem = { label: string; icon: IconName; danger?: boolean; run: () => void };
/** "More actions" menu for actions that are not used every day. */
function MoreMenu({ items, disabled }: { items: MenuItem[]; disabled: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  useDismiss(open, () => root.current, () => setOpen(false));
  return <span className="more-menu" ref={root} onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); setOpen(false); } }}>
    <button type="button" className="btn sm icon-only" disabled={disabled} aria-haspopup="menu" aria-expanded={open} aria-label={t("environments.moreActions")} data-tip={open ? undefined : t("environments.moreActions")} onClick={() => setOpen(!open)}>⋯</button>
    {open && <div className="more-menu-list" role="menu" aria-label={t("environments.moreActions")}>
      <MenuItems items={items} close={() => setOpen(false)} />
    </div>}
  </span>;
}

/** Menu at the pointer, kept inside the window; closes on outside click, Escape, scroll or blur. */
function FloatingMenu({ x, y, label, items, close }: { x: number; y: number; label: string; items: MenuItem[]; close: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.left = `${Math.max(4, Math.min(x, window.innerWidth - el.offsetWidth - 4))}px`;
    el.style.top = `${Math.max(4, Math.min(y, window.innerHeight - el.offsetHeight - 4))}px`;
    el.querySelector<HTMLElement>("[role=menuitem]")?.focus();
  }, [x, y]);
  useEffect(() => {
    const outside = (event: Event) => { if (!ref.current?.contains(event.target as Node)) close(); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("keydown", escape);
    window.addEventListener("scroll", close, true);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("keydown", escape);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("blur", close);
    };
  }, [close]);
  return <div ref={ref} className="more-menu-list floating" role="menu" aria-label={label} style={{ left: x, top: y }}>
    <MenuItems items={items} close={close} />
  </div>;
}

function MenuItems({ items, close }: { items: MenuItem[]; close: () => void }) {
  return <>{items.map((item) => <button key={item.label} type="button" role="menuitem" className={item.danger ? "text-danger" : undefined} onClick={() => { close(); item.run(); }}><Icon name={item.icon} />{item.label}</button>)}</>;
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

/** Select value: "inherit", "direct" or the id of a named proxy prefixed with "p:". */
const proxyValue = (view: EnvironmentProxyView) => view.mode === "profile" && view.id ? `p:${view.id}` : view.mode === "direct" ? "direct" : "inherit";
/** `renaming` changes to start editing with the name focused (F2 in the list). */
function ConnectionPanel({ env, busy, pendingCount, update, renaming = 0 }: { env: Environment; busy: boolean; pendingCount: number; update: (draft: EnvironmentDraft) => Promise<void>; renaming?: number }) {
  const { t } = useTranslation();
  const initial = (): ConnectionValues => ({ name: env.name, url: env.url, database: env.database, login: env.login, secret: "", protocol: env.protocol as Protocol });
  const [values, setValues] = useState<ConnectionValues | null>(null);
  const [proxyView, setProxyView] = useState<EnvironmentProxyView | null>(null);
  const [proxies, setProxies] = useState<ProxiesView | null>(null);
  const [proxy, setProxy] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [collapsed, setCollapsed] = useCollapsed("connection");
  const [error, setError] = useState<ErrorState>("");
  usePreventUnload(values !== null || busy);
  useEffect(() => {
    let active = true;
    Promise.all([ipc.environmentProxy(env.id), ipc.proxies()]).then(([view, list]) => { if (active) { setProxyView(view); setProxies(list); } }).catch((reason) => { if (active) setError(normalizeAppError(reason)); });
    return () => { active = false; };
  }, [env.id]);
  // Apply proxy settings when they arrive, even if editing started earlier.
  useEffect(() => { if (values && !proxy && proxyView) setProxy(proxyValue(proxyView)); }, [values, proxy, proxyView]);
  const cancel = () => { setValues(null); setProxy(null); setError(""); };
  const start = () => { setValues(initial()); if (proxyView) setProxy(proxyValue(proxyView)); ipc.proxies().then(setProxies, () => undefined); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { if (renaming && !busy) start(); }, [renaming]);
  const connectionChanged = values !== null && (
    (["name", "url", "database", "login"] as const).some((field) => values[field].trim() !== env[field])
    || values.protocol !== env.protocol || values.secret !== ""
  );
  // The backend refuses to send the saved key to a different server, database or user.
  const retarget = values !== null && (["url", "database", "login"] as const).some((field) => values[field].trim() !== env[field]);
  const proxyChanged = proxy !== null && proxyView !== null && proxy !== proxyValue(proxyView);
  const changed = connectionChanged || proxyChanged;
  const protocolName = (p: string) => protocolLabels[p] ? t(protocolLabels[p] as "environmentForm.protocols.legacy_jsonrpc") : p;
  const protocolText = env.protocol === "auto" && env.detected_protocol && protocolLabels[env.detected_protocol]
    ? `${protocolName(env.protocol)} · ${protocolName(env.detected_protocol)}` : protocolName(env.protocol);
  const proxyLabel = proxyView && proxies ? proxyOptions(proxies).find((o) => o.value === proxyValue(proxyView))?.label ?? t("environments.missingProxy") : "";

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
    const invalidUrl = urlError(values.url);
    if (invalidUrl) { setError(invalidUrl); return; }
    setError("");
    if (pendingCount > 0) setConfirming(true); else void save().catch(() => undefined);
  }}>
    <header className="panel-header"><div><PanelTitle title={t("environmentForm.connection")} collapsed={collapsed && !values} toggle={() => setCollapsed(!collapsed)}>{values && <RevokeNotice />}</PanelTitle>
</div>
      {collapsed && !values && <span className="panel-summary">{protocolText}{proxyView && proxies && <> · {t("environments.proxySummary", { proxy: proxyLabel })}</>}</span>}
      {(!collapsed || values !== null) && <PanelActions busy={busy} editing={values !== null} start={start} cancel={cancel} canSave={changed && values !== null && connectionComplete(values, retarget)} />}
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
        {/* The header already shows the name; the row only appears to edit it. */}
        {values && <div><dt><Icon name="pencil" />{t("environmentForm.name")}</dt><dd>
          <input className="inline-input" aria-label={t("environmentForm.name")} autoFocus required maxLength={200} value={values.name} onChange={(e) => setValues({ ...values, name: e.target.value })} /></dd></div>}
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
          : protocolText}</dd></div>
        <div><dt><Icon name="key" />{t("environmentForm.credential")}</dt><dd>{values
          ? <input className="inline-input" aria-label={t("environmentForm.apiKey")} type="password" autoComplete="new-password" required={retarget} placeholder={t(retarget ? "environmentForm.newTargetSecretPlaceholder" : "environmentForm.keepSecretPlaceholder")} value={values.secret} onChange={(e) => setValues({ ...values, secret: e.target.value })} />
          : env.has_secret ? <span className="secret-mask" aria-label={t("environmentForm.savedCredential")} data-tip={t("environmentForm.savedCredentialHint")}>••••••••••••</span> : <span className="text-warn">{t("environmentForm.noCredential")}</span>}</dd></div>
        <div><dt><Icon name="shield" />{t("proxy.title")}</dt><dd>{values && proxy && proxyView && proxies
          ? <Select className="inline-input" label={t("environments.environmentProxy")} value={proxy} options={proxyOptions(proxies)} onChange={setProxy} />
          : proxyView && proxies ? <span>{proxyLabel}</span> : "…"}</dd></div>
      </dl>
      {values && error && <div className="panel-body panel-foot"><p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p></div>}
    </fieldset>}
  </form>;
}
