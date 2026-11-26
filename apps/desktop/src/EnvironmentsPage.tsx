import PanelTitle, { chevron, panelToggle, useCollapsed } from "./PanelTitle";
import SettingsTable from "./SettingsTable";
import RulesPanel from "./RulesPanel";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { ipc, type Environment, type EnvironmentDraft, type EnvironmentProxyInput, type EnvironmentProxyView, type Pending, type ProxiesView, type Stage, type WriteMode } from "./ipc";
import EnvironmentForm, { ProtocolSelect, type EnvironmentOrganization, type FormDraft, connectionComplete, loadEnvironmentDefaults, protocolLabels, proxyChoice, proxyOptions, urlError, type ConnectionValues, type Protocol } from "./EnvironmentForm";
import { ConfirmDialog } from "./ConfirmDialog";
import { Icon } from "./Icon";
import { ListResizer, useListWidth } from "./ListResizer";
import { EnvIcon } from "./EnvIcon";
import SearchField from "./SearchField";
import HeaderInput from "./HeaderInput";
import { ModeSelect, modeIcons, modeLabels, modeShortLabels, modeTone } from "./ModeSelect";
import { StageBadge, StagePicker, stageShortLabel, stageTone } from "./StageBadge";
import Select from "./Select";
import { HealthChip, HealthDot, checkHealth, forgetHealth } from "./HealthChip";
import { formatRemaining, useNow } from "./time";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { formatDateTime } from "./i18n/format";
import { loadTagColors, tagChipStyle, tagDotProps, tagTone, useTagColors } from "./tagColors";
import { useWindowState, usePreventUnload } from "./windowState";
import TagEditor, { sameTag, uniqueTags } from "./TagEditor";
import EnvironmentFilters from "./EnvironmentFilters";
import { groupEnvironments, hostOf, type GroupBy } from "./hosts";
import { FloatingMenu, MenuItems, MoreMenu } from "./Menu";
import { modalOpen } from "./modal";
import { Banner, Button, IconButton, PanelHeader, Props, StatusDot, type Tone, chip, count as countPill, countdown, cx, detail, detailHeader, detailPane, empty, emptyIcon, envAvatar, envList, envListText, field, inlineInput, listEmpty, listEmptyIcon, listPane, listToolbar, moreMenu, panel, panelBody, panelSummary, secretMask, split, tile, titleWarn, tones } from "./ui";

const GROUP_BY_KEY = "odx.environments.groupBy";
const PINNED_KEY = "odx.environments.pinned";
function readStoredGroupBy(): GroupBy | null {
  try {
    const v = localStorage.getItem(GROUP_BY_KEY);
    return v === "project" || v === "host" || v === "mode" || v === "flat" ? v : null;
  } catch { return null; }
}
function readPinned(): string[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(PINNED_KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
  } catch { return []; }
}

export default function EnvironmentsPage({ envs, pending = [], focusId, openWritesSignal = 0, refresh, busy, creating, setCreating, save, update, remove, clone, openShell, closeOpenWrites, changeMode, draftRef }: {
  envs: Environment[];
  /** Confirm before saving context or headers, which revokes pending approvals. */
  pending?: Pending[];
  /** Environment to select on open, e.g. when coming from an approval. */
  focusId?: string | null;
  /** Bumped when the status bar's "open writes" indicator is clicked. */
  openWritesSignal?: number;
  refresh: () => Promise<void>;
  busy: boolean;
  creating: boolean;
  setCreating: (creating: boolean) => void;
  save: (draft: EnvironmentDraft, tags: string[], proxy: EnvironmentProxyInput, org?: EnvironmentOrganization) => Promise<Environment>;
  update: (id: string, draft: EnvironmentDraft) => Promise<Environment>;
  remove: (id: string) => Promise<boolean>;
  clone: (id: string, name: string) => Promise<Environment>;
  openShell: (env: Environment) => void;
  closeOpenWrites?: () => Promise<void>;
  changeMode: (env: Environment, mode: WriteMode) => void;
  draftRef?: { current: FormDraft | null };
}) {
  const { t } = useTranslation();
  const [listWidth, setListWidth] = useListWidth("odx.environments.listWidth");
  const tagColors = useTagColors();
  useEffect(() => { loadTagColors().catch(() => undefined); }, []);
  const [listMenu, setListMenu] = useState<{ env: Environment; x: number; y: number } | null>(null);
  const closeListMenu = useCallback(() => setListMenu(null), []);
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
  const [stageFilter, setStageFilter] = useWindowState<Stage[]>("environments.stages", []);
  const [groupByChoice, setGroupByChoice] = useState<GroupBy | null>(readStoredGroupBy);
  const hasProjects = useMemo(() => envs.some((env) => !!env.project?.trim()), [envs]);
  const groupBy: GroupBy = groupByChoice ?? (hasProjects ? "project" : "host");
  const changeGroupBy = (next: GroupBy) => {
    setGroupByChoice(next);
    try { localStorage.setItem(GROUP_BY_KEY, next); } catch { /* Optional preference. */ }
  };
  const [pinnedIds, setPinnedIds] = useState<string[]>(readPinned);
  const togglePin = (id: string) => {
    setPinnedIds((current) => {
      const next = current.includes(id) ? current.filter((x) => x !== id) : [id, ...current];
      try { localStorage.setItem(PINNED_KEY, JSON.stringify(next)); } catch { /* Optional preference. */ }
      return next;
    });
  };
  const lastSignal = useRef(openWritesSignal);
  useEffect(() => {
    if (openWritesSignal !== lastSignal.current) {
      lastSignal.current = openWritesSignal;
      setFilter("");
      setTagFilter([]);
      setStageFilter([]);
      setModeFilter(["temporary", "permanent"]);
      setCreating(false);
    }
  }, [openWritesSignal, setFilter, setTagFilter, setStageFilter, setModeFilter, setCreating]);
  // Tags differing only in case are one tag, shown with the first spelling.
  const allTags = useMemo(() => uniqueTags(envs.flatMap((env) => env.tags)).sort((a, b) => a.localeCompare(b)), [envs]);
  const allProjects = useMemo(() => uniqueTags(envs.map((env) => env.project?.trim() ?? "").filter(Boolean)).sort((a, b) => a.localeCompare(b)), [envs]);
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
      .filter((env) => !stageFilter.length || (!!env.stage && stageFilter.includes(env.stage)))
      .filter((env) => activeTags.every((tag) => env.tags.some((other) => sameTag(other, tag))))
      .filter((env) => !q || [env.name, env.url, env.database, env.login, env.project ?? "", env.stage ?? "", ...env.tags].some((v) => v.toLowerCase().includes(q)))
      .sort((a, b) => a.name.localeCompare(b.name));
  }, [envs, filter, modeFilter, stageFilter, activeTags]);
  const filtered = (!!filter.trim() && !filter.startsWith("#")) || activeTags.length > 0 || modeFilter.length > 0 || stageFilter.length > 0;
  function clearFilters() { setFilter(""); setTagFilter([]); setModeFilter([]); setStageFilter([]); }
  // Filtering never replaces the chosen environment, which would discard its unsaved edits.
  const selected = envs.find((env) => env.id === selectedId) ?? visible[0];
  useEffect(() => { if (selected && selected.id !== selectedId) setSelectedId(selected.id); }, [selected, selectedId, setSelectedId]);
  function select(id: string) { setCreating(false); setSelectedId(id); }
  const search = useRef<HTMLInputElement>(null);

  const pinnedEnvs = useMemo(() => filtered ? [] : pinnedIds.map((id) => visible.find((env) => env.id === id)).filter((env): env is Environment => !!env), [filtered, pinnedIds, visible]);
  const unpinnedEnvs = useMemo(() => pinnedEnvs.length ? visible.filter((env) => !pinnedIds.includes(env.id)) : visible, [pinnedEnvs.length, visible, pinnedIds]);
  const groups = useMemo(() => groupEnvironments(unpinnedEnvs, groupBy, {
    noProject: t("environments.noProject"),
    mode: (m) => t(modeLabels[m] as "mode.readOnly"),
  }), [unpinnedEnvs, groupBy, t]);
  const [collapsed, setCollapsed] = useWindowState<string[]>("environments.collapsed", []);
  // Filtering shows every match, also inside collapsed groups.
  const shown = [
    ...pinnedEnvs,
    ...groups.flatMap((group) => group.kind !== "plain" && !filtered && collapsed.includes(group.key) ? [] : group.items),
  ];
  // Another database on the same server: URL, protocol and proxy carry over. The login is usually the same but stays editable;
  // the API key belongs to the database, so it is never copied. Tags often differ per database (client, test), so they start empty.
  async function addDatabase(env: Environment) {
    if (creating) return;
    const proxy = await ipc.environmentProxy(env.id).then(proxyValue, () => "inherit");
    if (draftRef) draftRef.current = {
      values: { name: "", url: env.url, database: "", login: env.login, secret: "", protocol: env.protocol as Protocol },
      tags: [], proxy,
      ...(env.project ? { project: env.project } : {}),
      ...(env.stage ? { stage: env.stage } : {}),
    };
    setCreating(true);
  }
  const [confirmCloseWrites, setConfirmCloseWrites] = useState(false);
  const openWritesCount = useMemo(() => envs.filter((e) => e.write_mode === "temporary" || e.write_mode === "permanent").length, [envs]);
  function addToProject(project: string) {
    if (creating) return;
    if (draftRef) draftRef.current = {
      values: { name: "", url: "", secret: "", protocol: "auto", ...loadEnvironmentDefaults() },
      tags: [], proxy: "inherit", project, stage: null,
    };
    setCreating(true);
  }
  // Another user on the exact same database: URL, database, protocol, proxy, project and stage carry over.
  async function addUser(env: Environment) {
    if (creating) return;
    const proxy = await ipc.environmentProxy(env.id).then(proxyValue, () => "inherit");
    if (draftRef) draftRef.current = {
      values: { name: "", url: env.url, database: env.database, login: "", secret: "", protocol: env.protocol as Protocol },
      tags: [...env.tags], proxy,
      ...(env.project ? { project: env.project } : {}),
      ...(env.stage ? { stage: env.stage } : {}),
    };
    setCreating(true);
  }
  const row = (env: Environment, groupedByHost: boolean) => <li key={env.id} role="option" data-id={env.id} aria-selected={!creating && env.id === selected?.id}
    // One tab stop; arrows move within the list.
    tabIndex={env.id === selected?.id || (env.id === shown[0]?.id && !shown.some((v) => v.id === selected?.id)) ? 0 : -1}
    onClick={() => select(env.id)}
    onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); select(env.id); } }}
    // Our own menu instead of the app-wide text menu.
    onContextMenu={(e) => { e.preventDefault(); e.stopPropagation(); select(env.id); setListMenu({ env, x: e.clientX, y: e.clientY }); }}
  >
    <span className={envAvatar()}><EnvIcon env={env} /><StatusDot kind="badge" tone={modeTone[env.write_mode]} data-tip={t(modeLabels[env.write_mode] as "mode.readOnly")} /></span>
    {/* Stage badge and tags share the name's line so every row has the same height. */}
    <span className={envListText}><span className="flex items-center gap-1.5 min-w-0">
      <StageBadge stage={env.stage} compact />
      <strong className="flex-initial min-w-0">{env.name}</strong>
      {env.tags.length > 0 && <span className="flex flex-[0_1000_auto] gap-0.75 min-w-0 overflow-hidden">
        {env.tags.slice(0, LIST_TAGS).map((tag) => <span key={tag} className={listChip(tagTone(tagColors, tag), activeTags.some((active) => sameTag(active, tag)))} style={tagChipStyle(tagColors, tag)}
          data-tip={t("environments.filterByTag")} onClick={(e) => { if (activeTags.some((active) => sameTag(active, tag))) return; e.stopPropagation(); setTagFilter([...activeTags, tag]); }}>{tag}</span>)}
        {env.tags.length > LIST_TAGS && <span className={listChip()} data-tip={env.tags.slice(LIST_TAGS).join(", ")}>+{env.tags.length - LIST_TAGS}</span>}
      </span>}</span>
      {/* In a host group the header names the host; the login tells apart two users of one database. */}
      <small>{groupedByHost ? `${env.database} · ${env.login}` : `${hostOf(env.url)} · ${env.database}`}</small>
    </span>
    <HealthDot env={env} />
  </li>;

  // Ctrl+F or / focuses the search; Ctrl+N starts a new environment.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.altKey || modalOpen()) return;
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

  if (envs.length === 0 && !creating) return <div className={empty}>
    <span className={emptyIcon()}><Icon name="server" /></span>
    <h2>{t("environments.emptyTitle")}</h2>
    <div className="flex items-center gap-2 mt-2">
      <Button variant="primary" disabled={busy} onClick={() => setCreating(true)}><Icon name="plus" />{t("environmentForm.createAction")}</Button>
    </div>
  </div>;

  return <div className={split} style={{ "--list-w": `${listWidth}px` } as CSSProperties}>
    <aside className={listPane} aria-label={t("environments.listLabel")}>
      <ListResizer width={listWidth} setWidth={setListWidth} label={t("environments.resizeList")} />
      <div className={listToolbar}>
        <div className="flex items-center gap-2">
          {/* Active filters are chips inside the field, so filtering never adds rows above the list. */}
          <SearchField className="grow min-w-0" inputRef={search} value={filter} label={t("environments.filterLabel")} shortcut
            placeholder={activeTags.length || modeFilter.length || stageFilter.length ? "" : t("environments.filterPlaceholder")}
            filtered={filtered} onClear={clearFilters} clearLabel={t("environments.clearFilters")} onChange={(value) => { setFilter(value); setTagIndex(0); }}
            count={{ shown: visible.length, total: envs.length, text: t("environments.filterCount", { shown: visible.length, total: envs.length }) }}
            chips={activeTags.length > 0 || modeFilter.length > 0 || stageFilter.length > 0 ? <>
              {activeTags.map((tag) => <span key={tag} className={chip({ token: true, tone: tagTone(tagColors, tag) })} style={tagChipStyle(tagColors, tag)}>{tag}
                <button type="button" aria-label={t("environments.removeTagFilter", { tag })} onClick={() => setTagFilter(activeTags.filter((x) => x !== tag))}><Icon name="x" /></button></span>)}
              {stageFilter.map((stage) => <span key={stage} className={chip({ token: true, tone: stageTone[stage] })}>{stageShortLabel[stage]}
                <button type="button" aria-label={t("environments.removeStageFilter", { stage: stageShortLabel[stage] })} onClick={() => setStageFilter(stageFilter.filter((s) => s !== stage))}><Icon name="x" /></button></span>)}
              {modeFilter.map((mode) => <span key={mode} className={chip({ token: true, tone: modeTone[mode] })}><Icon name={modeIcons[mode]} />{t(modeShortLabels[mode] as "mode.readOnlyShort")}
                <button type="button" aria-label={t("environments.removeModeFilter", { mode: t(modeLabels[mode] as "mode.readOnly") })} onClick={() => setModeFilter(modeFilter.filter((m) => m !== mode))}><Icon name="x" /></button></span>)}
            </> : undefined}
            inputProps={{ "aria-keyshortcuts": "Control+F /", role: "combobox", "aria-expanded": tagMatches.length > 0, "aria-controls": "tag-suggestions", "aria-autocomplete": "list", onKeyDown: (e) => {
              if (tagMatches.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) { e.preventDefault(); setTagIndex((i) => (i + (e.key === "ArrowDown" ? 1 : tagMatches.length - 1)) % tagMatches.length); }
              else if (tagMatches.length && e.key === "Enter") { e.preventDefault(); addTagFilter(tagMatches[Math.min(tagIndex, tagMatches.length - 1)]); }
              else if (e.key === "Backspace" && !filter && (modeFilter.length || stageFilter.length || activeTags.length)) { if (modeFilter.length) setModeFilter(modeFilter.slice(0, -1)); else if (stageFilter.length) setStageFilter(stageFilter.slice(0, -1)); else setTagFilter(activeTags.slice(0, -1)); }
            } }} />
          <EnvironmentFilters envs={envs} tags={activeTags} setTags={setTagFilter} modes={modeFilter} setModes={setModeFilter} stages={stageFilter} setStages={setStageFilter} />
          {envs.length > 1 && <GroupByButton value={groupBy} onChange={changeGroupBy} />}
          <Button size="sm" iconOnly className="flex-none" disabled={rechecking} onClick={() => void recheckAll()} aria-label={t("environments.checkAll")} data-tip={t("environments.checkAll")}><Icon name="refresh" className={rechecking ? "animate-spin" : undefined} /></Button>
          <Button variant="primary" size="sm" iconOnly className="flex-none" disabled={busy || creating} onClick={() => setCreating(true)} aria-label={t("environments.newEnvironment")} aria-keyshortcuts="Control+N" data-tip={`${t("environments.newEnvironment")} · Ctrl+N`}><Icon name="plus" /></Button>
        </div>
        {closeOpenWrites && openWritesCount > 0 && <div className="flex items-center justify-between gap-2 px-2 py-1 rounded-md border border-warn/35 bg-warn-soft text-warn text-xs">
          <span className="inline-flex items-center gap-1.5 truncate"><Icon name="pencil" className="size-3 flex-none" />{t("service.openWrites", { count: openWritesCount })}</span>
          <Button size="sm" variant="ghost" className="h-5! px-1.5! text-warn! hover:bg-warn/15!" disabled={busy} onClick={() => setConfirmCloseWrites(true)}>{t("environments.closeOpenWrites")}</Button>
        </div>}
        {tagQuery !== null && <ul id="tag-suggestions" className="absolute z-20 top-[calc(100%-4px)] inset-x-2 max-h-[220px] overflow-auto p-0.75 border border-line-strong rounded-md bg-surface shadow-pop" role="listbox" aria-label={t("environments.tagFilter")}>
          {tagMatches.map((tag, i) => { const dot = tagDotProps(tagColors, tag); return <li key={tag} role="option" className={`${SUGGESTION} cursor-pointer hover:bg-accent-soft hover:text-fg aria-selected:bg-accent-soft aria-selected:text-fg`} aria-selected={i === Math.min(tagIndex, tagMatches.length - 1)} onMouseDown={(e) => { e.preventDefault(); addTagFilter(tag); }}>
            <span className={dot.className} style={dot.style} aria-hidden="true" />{tag}</li>; })}
          {tagMatches.length === 0 && <li className={`${SUGGESTION} cursor-default`}>{allTags.length ? t("environments.noMatches") : t("environments.noTagsYet")}</li>}
        </ul>}
      </div>
      <ul className={envList} role="listbox" aria-label={t("environments.environments")} aria-keyshortcuts="Delete F2 Control+D" onKeyDown={(e) => {
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
        {pinnedEnvs.length > 0 && <li key="__pinned__" role="group" aria-label={t("environments.pinnedSection")} className="block p-0 cursor-default hover:bg-transparent mb-1 pb-1 border-b border-line">
          <div className="flex items-center gap-1.5 pt-1 pr-1 pb-0.5 pl-2 text-fg-3 text-2xs font-semibold uppercase tracking-[.05em]">
            <Icon name="pin" className="size-3" /><span>{t("environments.pinnedSection")}</span>
            <span className="ml-auto px-1.25 rounded-[7px] bg-neutral-soft tabular-nums font-normal">{pinnedEnvs.length}</span>
          </div>
          <ul role="none">{pinnedEnvs.map((env) => row(env, false))}</ul>
        </li>}
        {groups.map((group) => group.kind === "plain" ? group.items.map((env) => row(env, false)) : <li key={group.key} role="group" aria-label={group.label} className="block p-0 cursor-default hover:bg-transparent [li+&]:mt-1 [&+li]:mt-1">
          <div className="flex items-center gap-1 pt-1 pr-1 pb-0.5 pl-2 text-fg-3 text-xs [&_.favicon]:size-3.5">
            <button type="button" className={`${panelToggle} flex-1 min-w-0 ${group.kind === "host" ? "font-mono" : "font-medium"} hover:text-fg`} aria-expanded={filtered || !collapsed.includes(group.key)} disabled={filtered}
              onClick={() => setCollapsed(collapsed.includes(group.key) ? collapsed.filter((h) => h !== group.key) : [...collapsed, group.key])}>
              <Icon name="chevron" className={chevron} />{group.kind === "host" && <EnvIcon env={group.items[0]} />}<span className="truncate">{group.label}</span>
            </button>
            <span className="px-1.25 rounded-[7px] bg-neutral-soft tabular-nums">{group.items.length}</span>
            {group.kind === "host" && <IconButton size="sm" disabled={busy || creating} onClick={() => void addDatabase(group.items[0])}
              aria-label={t("environments.addDatabase")} data-tip={t("environments.addDatabase")}><Icon name="plus" /></IconButton>}
            {group.kind === "project" && group.key !== "project:__none__" && <IconButton size="sm" disabled={busy || creating} onClick={() => addToProject(group.label)}
              aria-label={t("environments.addToProject", { project: group.label })} data-tip={t("environments.addToProject", { project: group.label })}><Icon name="plus" /></IconButton>}
          </div>
          {(filtered || !collapsed.includes(group.key)) && <ul role="none" className="pl-2.5">{group.items.map((env) => row(env, group.kind === "host"))}</ul>}
        </li>)}
        {visible.length === 0 && envs.length > 0 && <li className={listEmpty}>
          <span className={listEmptyIcon}><Icon name="search" /></span>
          <strong>{t("environments.noEnvironmentMatches")}</strong>
          {filtered && <Button size="sm" className="mt-2" onClick={clearFilters}><Icon name="x" />{t("environments.clearFilters")}</Button>}</li>}
      </ul>
    </aside>
    {listMenu && <FloatingMenu at={{ left: listMenu.x, top: listMenu.y }} label={listMenu.env.name} className={moreMenu} close={closeListMenu}><MenuItems close={closeListMenu} items={[
      { label: t("environments.openShell"), icon: "terminal", run: () => openShell(listMenu.env) },
      { label: t(pinnedIds.includes(listMenu.env.id) ? "environments.unpinEnvironment" : "environments.pinEnvironment"), icon: "pin", run: () => togglePin(listMenu.env.id) },
      { label: t("environments.cloneEnvironment"), icon: "copy", run: () => setDetailAction("clone") },
      ...creating ? [] : [
        { label: t("environments.addDatabase"), icon: "database" as const, run: () => void addDatabase(listMenu.env) },
        { label: t("environments.addUser"), icon: "user" as const, run: () => void addUser(listMenu.env) },
      ],
      { label: t("environments.deleteEnvironment"), icon: "trash", danger: true, run: () => setDetailAction("delete") },
    ]} /></FloatingMenu>}
    <section className={detailPane}>
      {creating && focusHint && <Banner tone="info" compact role="status"><Icon name="alert" />{t("environments.focusWhileCreating", { name: envs.find((env) => env.id === focusHint)?.name ?? "" })}</Banner>}
      {creating ? <EnvironmentForm busy={busy} tagSuggestions={allTags} projectSuggestions={allProjects} draftRef={draftRef} cancel={() => setCreating(false)} save={async (draft, tags, proxy, org) => { setSelectedId((await save(draft, tags, proxy, org)).id); }} />
        : selected && <EnvironmentDetail key={selected.id} env={selected} pinned={pinnedIds.includes(selected.id)} togglePin={() => togglePin(selected.id)} addUser={() => void addUser(selected)} action={detailAction} actionHandled={() => setDetailAction(null)} busy={busy} pendingCount={pending.filter((p) => p.environment_id === selected.id).length} envNames={envs.map((e) => e.name)} allTags={allTags} allProjects={allProjects}
          refresh={refresh}
          openShell={() => openShell(selected)}
          clone={async (name) => { setSelectedId((await clone(selected.id, name)).id); }}
          update={async (draft) => { await update(selected.id, draft); forgetHealth(selected.id); }}
          remove={() => remove(selected.id).then((ok) => { if (ok) forgetHealth(selected.id); return ok; })}
          changeMode={(mode) => changeMode(selected, mode)} />}
    </section>
    {closeOpenWrites && <ConfirmDialog
      open={confirmCloseWrites}
      busy={busy}
      tone="warn"
      title={t("environments.closeOpenWritesTitle")}
      description={t("environments.closeOpenWritesDescription", { count: openWritesCount })}
      confirmLabel={t("environments.closeOpenWritesConfirm")}
      onOpenChange={(open) => { if (!open && !busy) setConfirmCloseWrites(false); }}
      onConfirm={async () => { await closeOpenWrites(); setConfirmCloseWrites(false); }}
    />}
  </div>;
}

const LIST_TAGS = 3;
/** Smaller than chip(): its height, padding, size and colors all differ. */
const listChip = (tone?: Tone, selected?: boolean) => cx("inline-flex items-center gap-[5px] h-4 px-1.25 max-w-[110px] overflow-hidden text-ellipsis whitespace-nowrap flex-none border rounded-sm text-2xs font-medium cursor-pointer",
  tone ? cx(tones[tone], "text-tone bg-tone-soft") : "text-fg-3 bg-surface-2", selected ? "border-accent" : tone ? "border-transparent" : "border-line");
const SUGGESTION = "flex items-center gap-2 px-1.5 py-1.25 rounded-sm text-sm text-fg-2";

/** Build a saved environment draft; an empty credential preserves it. Context is edited separately and ignored on update. */
function draftFrom(env: Environment, changes: Partial<EnvironmentDraft>): EnvironmentDraft {
  return { name: env.name, url: env.url, database: env.database, login: env.login, secret: "", protocol: env.protocol as EnvironmentDraft["protocol"], context: env.context, ...changes };
}

type DetailTab = "general" | "contextHeaders" | "rules";

function EnvironmentDetail({ env, pinned, togglePin, addUser, action, actionHandled, pendingCount, refresh, busy, envNames, allTags, allProjects, update, remove, clone, openShell, changeMode }: {
  env: Environment;
  pinned: boolean;
  togglePin: () => void;
  addUser: () => void;
  /** Requested from the list's context menu or keys. */
  action?: DetailAction | null;
  actionHandled?: () => void;
  pendingCount: number;
  refresh: () => Promise<void>;
  busy: boolean;
  envNames: string[];
  allTags: string[];
  allProjects: string[];
  clone: (name: string) => Promise<void>;
  openShell: () => void;
  update: (draft: EnvironmentDraft) => Promise<void>;
  remove: () => Promise<boolean>;
  changeMode: (mode: WriteMode) => void;
}) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<DetailTab>("general");
  const [contextCount, setContextCount] = useState(0);
  const [headersCount, setHeadersCount] = useState(0);
  const [rulesCount, setRulesCount] = useState(0);
  const contextHeadersCount = contextCount + headersCount;
  const until = env.write_mode === "temporary" && env.write_until ? Date.parse(env.write_until) : NaN;
  const now = useNow(Number.isFinite(until));
  const [deleting, setDeleting] = useState(false);
  const [cloneName, setCloneName] = useState<string | null>(null);
  const [renaming, setRenaming] = useState(0);
  const [tagError, setTagError] = useState<ErrorState>("");
  // Edits build on the latest local tags, not on props that update only after the save round trip.
  const [tags, setTags] = useState(env.tags);
  useEffect(() => setTags(env.tags), [env.tags]);
  // One tag save at a time, in order: an earlier save landing last would undo a later edit. Only the latest reports.
  const tagSaves = useRef(Promise.resolve());
  const tagVersion = useRef(0);
  const saveTags = (next: string[]) => {
    setTags(next);
    setTagError("");
    const version = ++tagVersion.current;
    const latest = () => version === tagVersion.current;
    tagSaves.current = tagSaves.current.then(() => ipc.setTags(env.id, next)).then(
      () => { if (latest()) void refresh(); },
      (reason) => { if (latest()) { setTags(env.tags); setTagError(normalizeAppError(reason)); } });
  };
  useEffect(() => {
    if (!action) return;
    if (action === "clone") setCloneName(suggestedName());
    else if (action === "rename") { setTab("general"); setRenaming((n) => n + 1); }
    else setDeleting(true);
    actionHandled?.();
  });
  const suggestedName = () => {
    let name = t("environments.copyName", { name: env.name });
    for (let i = 2; envNames.includes(name); i++) name = t("environments.copyNameNumbered", { name: env.name, index: i });
    return name;
  };

  return <div className={detail}>
    <header className={detailHeader}>
      <span className={tile()}><EnvIcon env={env} /></span>
      <div className="min-w-0 flex-[1_1_220px]">
        <h2 className="flex items-center gap-2 min-w-0">
          <StageBadge stage={env.stage} />
          <span className="truncate">{env.name}</span>
          <IconButton size="sm" className="flex-none hover:text-accent-text!" onClick={() => void ipc.openUrl(env.id).catch(() => undefined)} data-tip={t("environments.openBrowserTip", { url: env.url })} aria-label={t("environments.openBrowser")}><Icon name="external" /></IconButton>
        </h2>
        <div className="mt-1">
          <TagEditor tags={tags} suggestions={allTags} disabled={busy} collapsed onChange={saveTags} />
          {tagError && <small className="text-danger text-xs mt-px" role="alert">{formatAppError(tagError)}</small>}
        </div>
      </div>
      <HealthChip env={env} />
      <Button size="sm" disabled={busy} onClick={openShell}><Icon name="terminal" />{t("environments.openShell")}</Button>
      <MoreMenu label={t("environments.moreActions")} disabled={busy} items={[
        { label: t(pinned ? "environments.unpinEnvironment" : "environments.pinEnvironment"), icon: "pin", run: togglePin },
        { label: t("environments.addUser"), icon: "user", run: addUser },
        { label: t("environments.cloneEnvironment"), icon: "copy", run: () => setCloneName(suggestedName()) },
        { label: t("environments.deleteEnvironment"), icon: "trash", danger: true, run: () => setDeleting(true) },
      ]} />
    </header>

    <nav className="flex items-center gap-1 border-b border-line pb-2" aria-label={env.name}>
      {([
        ["general", "plug", 0],
        ["contextHeaders", "settings", contextHeadersCount],
        ["rules", "shield", rulesCount],
      ] as const).map(([id, icon, badge]) =>
        <button key={id} type="button"
          className="inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md text-fg-2 text-sm whitespace-nowrap cursor-pointer hover:bg-hover hover:text-fg aria-[current=page]:bg-active aria-[current=page]:text-fg aria-[current=page]:font-medium aria-[current=page]:[&_.icon]:text-accent-text"
          aria-current={tab === id ? "page" : undefined} onClick={() => setTab(id)}>
          <Icon name={icon} className="size-3.5" /><span>{t(`environments.tabs.${id}`)}</span>
          {badge > 0 && <span className={countPill()} aria-hidden="true">{badge}</span>}
        </button>)}
    </nav>

    <div className="grid gap-3" hidden={tab !== "general"}>
      <section className={panel}>
        <PanelHeader><h3>{t("environments.writeMode")}</h3>
          {Number.isFinite(until) && <span className="flex items-center gap-2 flex-none"><span className={countdown("warn")} data-tip={t("environments.readOnlyAt", { time: formatDateTime(new Date(until), { hour: "2-digit", minute: "2-digit" }) })}><Icon name="clock" /><time dateTime={env.write_until!}>{formatRemaining(until - now, { hour: t("common.hourShort"), minute: t("common.minuteShort") })}</time></span><Button size="sm" disabled={busy} onClick={() => changeMode("temporary")}>{t("environments.changeDeadline")}</Button></span>}
        </PanelHeader>
        <div className={panelBody}>
          <ModeSelect id={`mode-${env.id}`} value={env.write_mode} disabled={busy} onValueChange={changeMode} />
        </div>
      </section>
      <OrganizationPanel env={env} allProjects={allProjects} refresh={refresh} busy={busy} update={update} renaming={renaming} />
      <ConnectionPanel env={env} busy={busy} pendingCount={pendingCount} update={update} />
    </div>
    <div className="grid gap-3" hidden={tab !== "contextHeaders"}>
      <SettingsTable kind="context" environmentId={env.id} pendingCount={pendingCount} onSaved={refresh} onCount={setContextCount} />
      <SettingsTable kind="headers" environmentId={env.id} pendingCount={pendingCount} onSaved={() => forgetHealth(env.id)} onCount={setHeadersCount} />
    </div>
    <div className="grid gap-3" hidden={tab !== "rules"}>
      <RulesPanel env={env} onCount={setRulesCount} />
    </div>

    <ConfirmDialog
      open={cloneName !== null}
      busy={busy}
      title={t("environments.cloneTitle", { name: env.name })}
      description={t("environments.cloneDescription")}
      confirmLabel={t("environments.cloneEnvironment")}
      confirmDisabled={!cloneName?.trim() || envNames.includes(cloneName.trim())}
      onOpenChange={(open) => { if (!open && !busy) setCloneName(null); }}
      onConfirm={async () => { if (cloneName) await clone(cloneName.trim()); setCloneName(null); }}
    >
      <label className={field}><span>{t("environments.copyNameLabel")}</span>
        <input autoFocus value={cloneName ?? ""} maxLength={200} onChange={(e) => setCloneName(e.target.value)} />
        {cloneName && envNames.includes(cloneName.trim()) && <small className="text-danger text-xs">{t("environments.nameExists")}</small>}
      </label>
    </ConfirmDialog>
    <ConfirmDialog
      open={deleting}
      busy={busy}
      destructive
      title={t("environments.deleteTitle", { name: env.name })}
      description={t("environments.deleteDescription")}
      confirmLabel={t("environments.deleteEnvironment")}
      onOpenChange={(open) => { if (!open && !busy) setDeleting(false); }}
      onConfirm={async () => { if (!(await remove())) throw { code: "app.environment_delete_failed" }; }}
    >
      <Props compact>
        <div><dt>{t("common.database")}</dt><dd className="mono">{env.database}</dd></div>
        <div><dt>URL</dt><dd className="mono">{env.url}</dd></div>
      </Props>
    </ConfirmDialog>
  </div>;
}

type DetailAction = "clone" | "delete" | "rename";
/** Show the editing notice beside the title without shifting content. */
function RevokeNotice() {
  const { t } = useTranslation();
  return <span className={titleWarn} tabIndex={0} aria-label={t("environments.saveWarning")}>
    <Icon name="alert" />
    <span className="tooltip start" role="tooltip"><strong>{t("settingsTable.onSave")}</strong>{t("environments.revokePending")}</span>
  </span>;
}

function PanelActions({ busy, editing, start, cancel, canSave }: { busy: boolean; editing: boolean; start: () => void; cancel: () => void; canSave: boolean }) {
  const { t } = useTranslation();
  return editing ? <span className="flex items-center gap-2 flex-none">
    <Button size="sm" disabled={busy} onClick={cancel}>{t("common.cancel")}</Button>
    <Button type="submit" size="sm" variant="primary" disabled={busy || !canSave}>{busy ? t("common.saving") : t("common.save")}</Button>
  </span> : <Button size="sm" disabled={busy} onClick={start}><Icon name="pencil" />{t("common.edit")}</Button>;
}

/** Select value: "inherit", "direct" or the id of a named proxy prefixed with "p:". */
const proxyValue = (view: EnvironmentProxyView) => view.mode === "profile" && view.id ? `p:${view.id}` : view.mode === "direct" ? "direct" : "inherit";
const GROUP_BY_OPTIONS: GroupBy[] = ["project", "host", "mode", "flat"];
function GroupByButton({ value, onChange }: { value: GroupBy; onChange: (next: GroupBy) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState<{ left: number; top: number } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const close = useCallback(() => setOpen(null), []);
  return <>
    <Button ref={trigger} size="sm" iconOnly className="flex-none" aria-haspopup="menu" aria-expanded={!!open}
      aria-label={t("environments.groupBy")}
      data-tip={open ? undefined : `${t("environments.groupBy")}: ${t(`environments.groupByOptions.${value}`)}`}
      onClick={() => {
        if (open) return setOpen(null);
        const rect = trigger.current!.getBoundingClientRect();
        setOpen({ left: rect.right, top: rect.bottom + 4 });
      }}>
      <Icon name="rows" />
    </Button>
    {open && <FloatingMenu at={open} alignEnd label={t("environments.groupBy")} className={moreMenu} close={close}>
      {GROUP_BY_OPTIONS.map((option) => <button key={option} type="button" role="menuitemradio" aria-checked={value === option}
        className={value === option ? "text-fg! font-medium" : undefined}
        onClick={() => { close(); onChange(option); }}>
        <Icon name="check" className={value === option ? "text-accent-text" : "opacity-0"} />
        <span>{t(`environments.groupByOptions.${option}`)}</span>
      </button>)}
    </FloatingMenu>}
  </>;
}

/** `renaming` changes to start editing with the name focused (F2 in the list). */
function OrganizationPanel({ env, allProjects, refresh, busy, update, renaming = 0 }: { env: Environment; allProjects: string[]; refresh: () => Promise<void>; busy: boolean; update: (draft: EnvironmentDraft) => Promise<void>; renaming?: number }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(env.name);
  const [project, setProject] = useState(env.project ?? "");
  const [stage, setStage] = useState<Stage | null>(env.stage ?? null);
  const [collapsed, setCollapsed] = useCollapsed("organization");
  const [error, setError] = useState<ErrorState>("");
  usePreventUnload(editing || busy);
  const cancel = () => { setEditing(false); setName(env.name); setProject(env.project ?? ""); setStage(env.stage ?? null); setError(""); };
  const start = () => { setName(env.name); setProject(env.project ?? ""); setStage(env.stage ?? null); setEditing(true); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { if (renaming && !busy) start(); }, [renaming]);
  const nameChanged = editing && name.trim() !== env.name;
  const orgChanged = editing && ((project.trim() || null) !== (env.project ?? null) || stage !== (env.stage ?? null));
  const changed = nameChanged || orgChanged;
  const summary = collapsed && !editing;

  async function save() {
    if (!editing || !changed || !name.trim() || busy) return;
    setError("");
    try {
      if (nameChanged) {
        await update(draftFrom(env, { name: name.trim(), url: env.url, database: env.database, login: env.login, secret: "", protocol: env.protocol as Protocol }));
      }
      if (orgChanged) {
        await ipc.setOrganization(env.id, project.trim() || null, stage);
        await refresh();
      }
      setEditing(false);
    } catch (reason) { setError(normalizeAppError(reason)); }
  }

  return <form className={panel} autoComplete="off" aria-label={t("environmentForm.identityTitle")} onSubmit={(event) => {
    event.preventDefault();
    void save();
  }}>
    <PanelHeader className={summary ? "flex-wrap gap-y-1" : undefined}>
      <div className={cx("flex-auto min-w-0", summary && "basis-[220px]")}><PanelTitle title={t("environmentForm.identityTitle")} collapsed={collapsed && !editing} toggle={() => setCollapsed(!collapsed)} /></div>
      {summary && <span className={panelSummary}>{[env.stage ? stageShortLabel[env.stage] : null, env.project].filter(Boolean).join(" · ") || env.name}</span>}
      {(!collapsed || editing) && <PanelActions busy={busy} editing={editing} start={start} cancel={cancel} canSave={changed && !!name.trim()} />}
    </PanelHeader>
    {(!collapsed || editing) && <fieldset disabled={busy}>
      <Props editing={editing}>
        <div><dt><Icon name="pencil" />{t("environmentForm.name")}</dt><dd>{editing
          ? <input className={inlineInput()} aria-label={t("environmentForm.name")} autoFocus required maxLength={200} value={name} onChange={(e) => setName(e.target.value)} />
          : <span>{env.name}</span>}</dd></div>
        <div><dt><Icon name="server" />{t("environments.project")}</dt><dd>{editing
          ? <HeaderInput className={inlineInput()} aria-label={t("environments.project")} maxLength={80} placeholder={t("environments.projectPlaceholder")} value={project} suggestions={allProjects} showAll onChange={setProject} onBlur={() => undefined} />
          : env.project ? <span>{env.project}</span> : <span className="text-fg-3">—</span>}</dd></div>
        <div><dt><Icon name="alert" />{t("environments.stage")}</dt><dd>{editing
          ? <div className="-ml-[9px]"><StagePicker value={stage} disabled={busy} onChange={setStage} /></div>
          : env.stage ? <StageBadge stage={env.stage} /> : <span className="text-fg-3">—</span>}</dd></div>
      </Props>
      {editing && error && <div className={cx(panelBody, "border-t")}><Banner tone="danger" role="alert"><Icon name="alert" />{formatAppError(error)}</Banner></div>}
    </fieldset>}
  </form>;
}

function ConnectionPanel({ env, busy, pendingCount, update }: { env: Environment; busy: boolean; pendingCount: number; update: (draft: EnvironmentDraft) => Promise<void> }) {
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
  const connectionChanged = values !== null && (
    (["url", "database", "login"] as const).some((field) => values[field].trim() !== env[field])
    || values.protocol !== env.protocol || values.secret !== ""
  );
  // The backend refuses to send the saved key to a different server, database or user.
  const retarget = values !== null && (["url", "database", "login"] as const).some((field) => values[field].trim() !== env[field]);
  const proxyChanged = proxy !== null && proxyView !== null && proxy !== proxyValue(proxyView);
  const changed = connectionChanged || proxyChanged;
  const protocolName = (p: string) => protocolLabels[p] ? t(protocolLabels[p] as "environmentForm.protocols.legacy_jsonrpc") : p;
  const protocolText = env.protocol === "auto" && env.detected_protocol && protocolLabels[env.detected_protocol]
    ? `${protocolName(env.protocol)} · ${protocolName(env.detected_protocol)}` : protocolName(env.protocol);
  const summary = collapsed && !values;
  const proxyLabel = proxyView && proxies ? proxyOptions(proxies).find((o) => o.value === proxyValue(proxyView))?.label ?? t("environments.missingProxy") : "";

  async function save() {
    if (!values || !changed || busy) return;
    setError("");
    try {
      await update(draftFrom(env, { name: env.name, url: values.url.trim(), database: values.database.trim(), login: values.login.trim(), secret: values.secret, protocol: values.protocol }));
      if (proxy && proxyChanged) {
        await ipc.saveEnvironmentProxy(env.id, proxyChoice(proxy));
        setProxyView(await ipc.environmentProxy(env.id));
      }
      cancel();
    } catch (reason) { setError(normalizeAppError(reason)); throw reason; }
  }

  return <form className={panel} autoComplete="off" aria-label={t("environmentForm.connection")} onSubmit={(event) => {
    event.preventDefault();
    if (!values || busy || !changed) return;
    const invalidUrl = urlError(values.url);
    if (invalidUrl) { setError(invalidUrl); return; }
    setError("");
    if (pendingCount > 0) setConfirming(true); else void save().catch(() => undefined);
  }}>
    <PanelHeader className={summary ? "flex-wrap gap-y-1" : undefined}><div className={cx("flex-auto min-w-0", summary && "basis-[260px]")}><PanelTitle title={t("environmentForm.connection")} collapsed={collapsed && !values} toggle={() => setCollapsed(!collapsed)}>{values && <RevokeNotice />}</PanelTitle>
</div>
      {summary && <span className={panelSummary}>{protocolText}{proxyView && proxies && <> · {t("environments.proxySummary", { proxy: proxyLabel })}</>}</span>}
      {(!collapsed || values !== null) && <PanelActions busy={busy} editing={values !== null} start={start} cancel={cancel} canSave={changed && values !== null && connectionComplete(values, retarget)} />}
    </PanelHeader>
    <ConfirmDialog open={confirming} busy={busy} tone="warn"
      title={t("environments.saveConnection")}
      description={t("environments.saveConnectionWarning", { count: pendingCount })}
      confirmLabel={t("settingsTable.saveAndRevoke")}
      onOpenChange={(open) => { if (!open) setConfirming(false); }}
      onConfirm={save} />
    {/* Keep rows stable; editing changes only the value control. */}
    {(!collapsed || values) && <fieldset disabled={busy}>
      <Props editing={!!values}>
        <div><dt><Icon name="globe" />{t("common.url")}</dt><dd className="mono">{values
          ? <input className={inlineInput(true)} aria-label={t("environmentForm.odooUrl")} autoFocus type="url" required value={values.url} onChange={(e) => setValues({ ...values, url: e.target.value })} />
          : <button type="button" className="inline-flex items-center gap-1.5 text-left text-accent-text hover:underline cursor-pointer bg-transparent border-0 p-0 font-mono text-sm max-w-full" onClick={() => void ipc.openUrl(env.id).catch(() => undefined)} data-tip={t("environments.openBrowserTip", { url: env.url })}>
              <span className="truncate">{env.url}</span><Icon name="external" className="size-3 shrink-0 opacity-75" />
            </button>}</dd></div>
        <div><dt><Icon name="database" />{t("common.database")}</dt><dd className="mono">{values
          ? <input className={inlineInput(true)} aria-label={t("common.database")} required value={values.database} onChange={(e) => setValues({ ...values, database: e.target.value })} />
          : env.database}</dd></div>
        <div><dt><Icon name="user" />{t("environmentForm.user")}</dt><dd>{values
          ? <input className={inlineInput()} aria-label={t("environmentForm.user")} required autoComplete="off" value={values.login} onChange={(e) => setValues({ ...values, login: e.target.value })} />
          : env.login}</dd></div>
        <div><dt><Icon name="plug" />{t("environmentForm.protocol")}</dt><dd>{values
          ? <ProtocolSelect className={inlineInput()} value={values.protocol} onChange={(protocol) => setValues({ ...values, protocol })} />
          : protocolText}</dd></div>
        <div><dt><Icon name="key" />{t("environmentForm.credential")}</dt><dd>{values
          ? <input className={inlineInput()} aria-label={t("environmentForm.apiKey")} type="password" autoComplete="new-password" required={retarget} placeholder={t(retarget ? "environmentForm.newTargetSecretPlaceholder" : "environmentForm.keepSecretPlaceholder")} value={values.secret} onChange={(e) => setValues({ ...values, secret: e.target.value })} />
          : env.has_secret ? <span className={secretMask} aria-label={t("environmentForm.savedCredential")} data-tip={t("environmentForm.savedCredentialHint")}>••••••••••••</span> : <span className="text-warn">{t("environmentForm.noCredential")}</span>}</dd></div>
        <div><dt><Icon name="shield" />{t("proxy.title")}</dt><dd>{values && proxy && proxyView && proxies
          ? <Select className={inlineInput()} label={t("environments.environmentProxy")} value={proxy} options={proxyOptions(proxies)} onChange={setProxy} />
          : proxyView && proxies ? <span>{proxyLabel}</span> : "…"}</dd></div>
      </Props>
      {values && error && <div className={cx(panelBody, "border-t")}><Banner tone="danger" role="alert"><Icon name="alert" />{formatAppError(error)}</Banner></div>}
    </fieldset>}
  </form>;
}
