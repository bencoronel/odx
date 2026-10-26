import { createContext, memo, useCallback, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ContextType, type ClipboardEvent, type KeyboardEvent, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { currentShellFlush, registerShellFlush } from "./shellFlush";
import { ipc, type AgentRequest, type Environment, type HistoryEntry, type ShellCall } from "./ipc";
import { highlight } from "./shell/highlight";
import { exportName, isIds, isM2o, toCsv, toJson, type Link, type Table } from "./table";
import { CellActions, TableBlock } from "./TableBlock";
import { CommandError, META, VERBS, expandAlias, needsOdoo, parse, searchCall, substitute, unfinished, type Command, type OdooCall } from "./shell/commands";
import { completions, type Completion } from "./shell/complete";
import { connectionProblem, outcomeOf, plainEntry, pythonCall, reshapeTable, resultEntries } from "./shell/output";
import { MAX_LINES, newTab, nextEntryId, peekEntryId, pristine, restore, savedAliases, seedEntryIds, snapshot, tabLabel, type Confirmation, type ConnectionErrorPresentation, type Entry, type NewEntry, type QueuedRun, type AgentRun, type Session, type ShellTab } from "./shell/sessions";
import { modeIcons, modeLabels, modeShortLabels, modeTone } from "./ModeSelect";
import { CallDetails, ValuesTable, methodTone, recordIds, writtenValues } from "./ApprovalDetails";
import { EnvIcon } from "./EnvIcon";
import { hostOf } from "./hosts";
import { Icon } from "./Icon";
import CopyButton from "./CopyButton";
import { useTranslation } from "react-i18next";
import { formatDateTime, formatNumber } from "./i18n/format";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { hasWindowState, useWindowState, usePreventUnload } from "./windowState";
import { FloatingMenu, MenuItems } from "./Menu";
import { modalOpen } from "./modal";

/** Commands kept out of the history, with their aliases (\s for \history). */
const UNRECALLED = new Set(["\\?", "help", "\\history", "\\s", "\\clear"]);
/** Longer logins (often e-mail addresses) are cut with an ellipsis in the prompt. */
const PROMPT_LOGIN = 20;
const FONT_KEY = "odx.shell.fontSize";
const FONT_RANGE = [10, 24] as const;
const DEFAULT_FONT = 12;
function storedFont(): number {
  try { const n = Number(localStorage.getItem(FONT_KEY)); return n >= FONT_RANGE[0] && n <= FONT_RANGE[1] ? n : DEFAULT_FONT; } catch { return DEFAULT_FONT; }
}
/** Quiet time before changed sessions are saved to the vault, so a burst of output is one save. */
export const SAVE_DELAY_MS = 800;
/** Typing pause before the input is checked for a syntax hint. */
export const HINT_DELAY_MS = 700;
const GROUP_FIELDS = ["all_group_ids", "groups_id", "group_ids"];

type Menu = Completion & { index: number };



const size = (bytes: number) => bytes < 1024 ? `${formatNumber(bytes)} B` : `${formatNumber(bytes / 1024, { maximumFractionDigits: 1 })} KB`;

export default function ShellPage({ envs, connectTo, command, onConnectHandled, visible = true, agentRequests = [], onAgentHandled }: {
  /** `command` runs once connected to `connectTo`. */
  envs: Environment[]; connectTo?: string | null; command?: string | null; onConnectHandled?: () => void; visible?: boolean;
  /** Requests from MCP agents (terminal_run, show); each is answered through ipc.agentReply and then reported handled. */
  agentRequests?: AgentRequest[]; onAgentHandled?: (id: number) => void;
}) {
  const { t } = useTranslation();
  const [tabs, setTabs] = useWindowState<ShellTab[]>("shell.tabs", () => [], (tabs) => tabs.map((tab) => ({
    ...tab, busy: false, runs: undefined, session: { ...tab.session, history: null, models: {}, fields: {} },
  })));
  // Restored entries keep their ids; new entries must not reuse them.
  const seeded = useRef(false);
  if (!seeded.current) {
    seeded.current = true;
    for (const tab of tabs) seedEntryIds(tab.session.entries);
  }
  const [activeId, setActiveId] = useWindowState("shell.activeId", tabs[0]?.id ?? "");
  const [aliases, setAliases] = useWindowState<Record<string, string>>("shell.aliases", {});

  // After a restart (no in-memory window state), load the tabs saved in the vault. Saving waits
  // for this load so an empty start never overwrites the saved sessions.
  const [loaded, setLoaded] = useState(() => hasWindowState("shell.tabs"));
  useEffect(() => {
    if (loaded) return;
    let cancelled = false;
    void Promise.resolve(ipc.shellSessions()).then((data) => {
      if (cancelled || !data) return;
      setAliases((current) => ({ ...savedAliases(data), ...current }));
      const restored = restore(data, t("shell.sessionRestored", { date: formatDateTime(new Date(), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }) }));
      if (!restored) return;
      // Keep anything typed while the vault was being read.
      setTabs((current) => [...restored.tabs, ...current.filter((tab) => !pristine(tab))]);
      setActiveId(restored.activeId);
    }).catch(() => undefined).finally(() => { if (!cancelled) setLoaded(true); });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const state = useRef({ tabs, activeId, aliases });
  state.current = { tabs, activeId, aliases };
  const saveTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const saving = useRef<Promise<void>>(Promise.resolve());
  const [saveFailed, setSaveFailed] = useState(false);
  const save = () => {
    clearTimeout(saveTimer.current);
    saveTimer.current = undefined;
    return saving.current = Promise.resolve().then(() => ipc.saveShellSessions(snapshot(state.current.tabs, state.current.activeId, state.current.aliases)))
      .then(() => setSaveFailed(false), () => setSaveFailed(true));
  };
  const persist = useCallback(() => {
    if (!loaded) return;
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(save, SAVE_DELAY_MS);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded]);
  // Switching tabs alone is not worth a save of every tab's output; the next save records it.
  useEffect(persist, [tabs, aliases, persist]);
  // Flush on quit, lock or when the window goes away.
  useEffect(() => {
    const flush = () => saveTimer.current !== undefined ? save() : saving.current;
    const onHide = () => void flush();
    registerShellFlush(flush);
    window.addEventListener("pagehide", onHide);
    return () => { window.removeEventListener("pagehide", onHide); if (currentShellFlush() === flush) registerShellFlush(undefined); void flush(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [renaming, setRenaming] = useState<{ id: string; value: string; width: number } | null>(null);
  const [fontSize, setFontSizeState] = useState(storedFont);
  const setFontSize = useCallback((size: number) => {
    const next = Math.min(FONT_RANGE[1], Math.max(FONT_RANGE[0], size));
    setFontSizeState(next);
    try { localStorage.setItem(FONT_KEY, String(next)); } catch { /* per-window only */ }
  }, []);
  usePreventUnload(tabs.some((tab) => tab.busy || tab.runs?.length) || renaming !== null);
  const refreshTabs = useCallback(() => setTabs((current) => [...current]), [setTabs]);
  // Stable callbacks let hidden sessions skip re-rendering when the app updates.
  const connectHandled = useRef(onConnectHandled);
  connectHandled.current = onConnectHandled;
  const add = (env: Pick<Environment, "id" | "name">) => {
    const tab = newTab(env);
    setTabs((current) => [...current, tab]);
    setActiveId(tab.id);
    return tab;
  };
  // "+" and Ctrl+T ask for the environment first, so a new session opens already connected.
  const [picking, setPicking] = useState<{ left: number; top: number } | null>(null);
  const plus = useRef<HTMLButtonElement>(null);
  const closePicker = useCallback(() => { setPicking(null); plus.current?.focus(); }, []);
  /** `from`: the button the menu hangs from; fixed position, since the tab strip scrolls and would clip it. */
  const pick = (from: HTMLElement | null = plus.current) => {
    if (!envs.length) return;
    const rect = from?.getBoundingClientRect();
    setPicking({ left: rect?.left ?? 8, top: (rect?.bottom ?? 40) + 4 });
  };
  const openWith = (env: Environment) => { setPicking(null); add(env); };
  // A requested environment reuses a tab of that environment (the active one first), else opens one. The command
  // queues in that tab until it is connected. Waits for saved tabs, since a restore changes the active tab.
  useEffect(() => {
    if (!connectTo || !loaded) return;
    const env = envs.find((e) => e.name.toLowerCase() === connectTo.toLowerCase());
    connectHandled.current?.();
    if (!env) return;
    const own = tabs.filter((tab) => tab.envId === env.id);
    const tab = own.find((item) => item.id === activeId) ?? own[0] ?? add(env);
    setActiveId(tab.id);
    if (command) { tab.runs = [...tab.runs ?? [], { command }]; refreshTabs(); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connectTo, loaded]);
  /** Busy tabs stay open. When the active tab goes, `keep` (the tab whose menu closed the others) takes its place, else its neighbour. */
  const close = (targets: ShellTab[], keep?: ShellTab) => {
    const gone = targets.filter((tab) => !tab.busy);
    if (!gone.length) return;
    const remaining = tabs.filter((item) => !gone.includes(item));
    setTabs(remaining);
    if (gone.some((tab) => tab.id === activeId)) {
      const index = tabs.findIndex((tab) => tab.id === activeId);
      setActiveId(keep && remaining.includes(keep) ? keep.id : remaining[Math.min(index, remaining.length - 1)]?.id ?? "");
    }
    if (renaming && gone.some((tab) => tab.id === renaming.id)) setRenaming(null);
  };
  // Pinning moves the tab to the front; unpinning, to just after the pinned ones.
  const pin = (tab: ShellTab) => {
    tab.pinned = !tab.pinned;
    const rest = tabs.filter((item) => item !== tab);
    const at = tab.pinned ? 0 : rest.filter((item) => item.pinned).length;
    setTabs([...rest.slice(0, at), tab, ...rest.slice(at)]);
  };
  const [tabMenu, setTabMenu] = useState<{ id: string; left: number; top: number } | null>(null);
  // Dragging a tab carries it under the pointer and marks with a line where it will land; the order changes on release.
  // It lands within its group (pinned or not). Mouse events, since native drag is off app-wide.
  const [dragging, setDragging] = useState<{ id: string; dx: number; drop?: { id: string; side: "before" | "after" } } | null>(null);
  const drag = (event: ReactMouseEvent, tab: ShellTab) => {
    if (event.button !== 0) return;
    const startX = event.clientX;
    let started = false;
    let next: ShellTab[] | undefined;
    const move = (e: MouseEvent) => {
      if (!started && Math.abs(e.clientX - startX) < 5) return;
      started = true;
      const current = state.current.tabs;
      const rest = current.filter((item) => item !== tab);
      const pinned = rest.filter((item) => item.pinned).length;
      // The slot among the other tabs: past every middle the pointer has crossed, kept inside the tab's group.
      const middles = rest.map((item) => {
        const rect = document.getElementById(`shell-tab-${item.id}`)?.parentElement?.getBoundingClientRect();
        return rect ? rect.left + rect.width / 2 : 0;
      });
      const crossed = middles.filter((middle) => e.clientX > middle).length;
      const slot = tab.pinned ? Math.min(crossed, pinned) : Math.max(crossed, pinned);
      const drop = slot === current.indexOf(tab) ? undefined
        : slot < rest.length ? { id: rest[slot].id, side: "before" as const } : { id: rest[rest.length - 1].id, side: "after" as const };
      next = drop && [...rest.slice(0, slot), tab, ...rest.slice(slot)];
      setDragging({ id: tab.id, dx: e.clientX - startX, drop });
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      setDragging(null);
      if (next) setTabs(next);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
  // Agent requests: a run goes to its tab's queue (the session runs it); show brings that tab to the front.
  useEffect(() => {
    if (!loaded) return;
    // Tabs opened earlier in this batch, so two runs for one environment share a session.
    const all = [...tabs];
    for (const request of agentRequests) {
      onAgentHandled?.(request.id);
      if (request.kind === "show") {
        if (all.some((tab) => tab.id === request.terminal)) { setActiveId(request.terminal); void ipc.agentReply(request.id); }
        else void ipc.agentReply(request.id, undefined, `no terminal ${request.terminal}`);
        continue;
      }
      const env = envs.find((e) => e.id === request.envId);
      const own = all.filter((tab) => tab.envId === request.envId);
      let tab = request.terminal ? all.find((item) => item.id === request.terminal) : own.find((item) => item.id === activeId) ?? own[0];
      if (!tab && !request.terminal && env) {
        // A new session for the agent opens behind the current tab; only show brings it forward.
        tab = newTab(env);
        all.push(tab);
        setTabs((current) => [...current, tab!]);
        if (all.length === 1) setActiveId(tab.id);
      }
      if (!tab) { void ipc.agentReply(request.id, undefined, request.terminal ? `no terminal ${request.terminal}` : "environment unavailable"); continue; }
      tab.runs = [...tab.runs ?? [], { command: request.command, agent: request }];
      if (tab.id !== activeId) tab.unseen = "agent";
    }
    if (agentRequests.length) refreshTabs();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentRequests, loaded]);
  // Terminal tab keys: Ctrl+T new, Ctrl+W close, Ctrl+Tab / Ctrl+PageDown next, with Shift / PageUp previous.
  useEffect(() => {
    if (!visible) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey) || event.altKey || modalOpen()) return;
      const key = event.key.toLowerCase();
      const index = tabs.findIndex((tab) => tab.id === activeId);
      if (key === "t" && !event.shiftKey) { event.preventDefault(); pick(plus.current ?? document.querySelector<HTMLElement>(".shell-empty .btn")); }
      else if (key === "w" && !event.shiftKey) { event.preventDefault(); if (tabs[index]) close([tabs[index]]); }
      else if (key === "tab" || key === "pagedown" || key === "pageup") {
        event.preventDefault();
        const step = key === "pageup" || (key === "tab" && event.shiftKey) ? -1 : 1;
        if (tabs.length) setActiveId(tabs[(index + step + tabs.length) % tabs.length].id);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, tabs, activeId, renaming]);
  const rename = (tab: ShellTab) => {
    const width = document.getElementById(`shell-tab-${tab.id}`)?.getBoundingClientRect().width;
    if (width === undefined) return;
    setActiveId(tab.id);
    setRenaming({ id: tab.id, value: tabLabel(tab), width });
  };
  const finishRename = (tab: ShellTab) => {
    if (renaming?.id !== tab.id) return;
    const name = renaming.value.trim();
    // Clearing the name, or typing the environment's, goes back to following the environment.
    tab.defaultName = !name || name === tab.envName;
    tab.name = tab.defaultName ? "" : name;
    setRenaming(null);
    refreshTabs();
  };
  return <div className="shell-sessions">
    <div className="shell-tabs" role="tablist" aria-label={t("shell.sessions")} aria-orientation="horizontal">
      {tabs.map((tab, index) => <div className="shell-tab" key={tab.id} role="presentation" data-active={activeId === tab.id} data-dragging={dragging?.id === tab.id || undefined}
        data-drop={dragging?.drop?.id === tab.id ? dragging.drop.side : undefined} style={dragging?.id === tab.id ? { transform: `translateX(${dragging.dx}px)` } : undefined}
        onMouseDown={(event) => { if (event.button === 1) event.preventDefault(); }}
        onAuxClick={(event) => { if (event.button === 1) { event.preventDefault(); close([tab]); } }}>
        {renaming?.id === tab.id ? <div className="shell-tab-editor" style={{ width: renaming.width }}>
          <Icon name={tab.busy ? "spinner" : "terminal"} className={tab.busy ? "spin" : undefined} />
          <input className="shell-tab-name" aria-label={t("shell.sessionName")} autoFocus maxLength={80} spellCheck={false} autoCorrect="off" autoComplete="off"
          value={renaming.value} onFocus={(event) => event.currentTarget.select()}
          onChange={(event) => setRenaming({ ...renaming, value: event.target.value })}
          onBlur={() => finishRename(tab)} onKeyDown={(event) => {
            if (event.key === "Enter") { event.preventDefault(); finishRename(tab); }
            if (event.key === "Escape") { event.preventDefault(); setRenaming(null); }
          }} />
        </div> : <button type="button" role="tab" id={`shell-tab-${tab.id}`} aria-controls={`shell-panel-${tab.id}`}
          aria-selected={activeId === tab.id} tabIndex={activeId === tab.id ? 0 : -1}
          onMouseDown={(event) => drag(event, tab)} onClick={() => setActiveId(tab.id)} onDoubleClick={(event) => { event.preventDefault(); rename(tab); }}
          onContextMenu={(event) => {
            event.preventDefault();
            event.stopPropagation();
            // The menu key reports no pointer position: hang the menu from the tab.
            const rect = event.currentTarget.getBoundingClientRect();
            setTabMenu({ id: tab.id, ...event.clientX || event.clientY ? { left: event.clientX, top: event.clientY } : { left: rect.left, top: rect.bottom + 4 } });
          }} onKeyDown={(event) => {
            if (event.key === "F2") { event.preventDefault(); rename(tab); return; }
            let target: ShellTab | undefined;
            if (event.key === "ArrowRight") target = tabs[(index + 1) % tabs.length];
            if (event.key === "ArrowLeft") target = tabs[(index + tabs.length - 1) % tabs.length];
            if (event.key === "Home") target = tabs[0];
            if (event.key === "End") target = tabs[tabs.length - 1];
            if (target) { event.preventDefault(); setActiveId(target.id); document.getElementById(`shell-tab-${target.id}`)?.focus(); }
          }}>
          <Icon name={tab.busy ? "spinner" : "terminal"} className={tab.busy ? "spin" : undefined} />
          <span>{tabLabel(tab)}</span>{!tab.defaultName && <>{" "}<span className="shell-tab-env">{tab.envName}</span></>}
          {tab.unseen && <span className="shell-tab-unseen" role="status" aria-label={t(tab.unseen === "agent" ? "shell.agentActivity" : "shell.finishedAway")} data-tip={tab.unseen === "agent" ? t("shell.agentActivity") : undefined} />}
        </button>}
        {tab.pinned ? <button type="button" className="icon-btn sm shell-tab-pin" aria-label={t("shell.unpinSession")} data-tip={t("shell.unpinSession")}
          onClick={() => pin(tab)}><Icon name="pin" /></button>
        : <button type="button" className="icon-btn sm shell-tab-close" aria-label={t("shell.closeSession", { name: tabLabel(tab) })}
          disabled={tab.busy} data-tip={tab.busy ? t("shell.callInProgress") : `${t("shell.closeSessionLabel")} · Ctrl+W`} onClick={() => close([tab])}><Icon name="x" /></button>}
      </div>)}
      <button type="button" ref={plus} className="icon-btn sm" aria-label={t("shell.newSession")} data-tip={picking ? undefined : envs.length ? `${t("shell.newSessionLabel")} · Ctrl+T` : t("shell.noEnvironments")} aria-keyshortcuts="Control+T"
        disabled={!envs.length} aria-haspopup="menu" aria-expanded={!!picking} onClick={() => picking ? setPicking(null) : pick()}><Icon name="plus" /></button>
    </div>
    {loaded && !tabs.length && <div className="empty shell-empty">
      <span className="empty-icon"><Icon name="terminal" /></span>
      <h2>{t("shell.noSessions")}</h2>
      {envs.length ? <button type="button" className="btn primary" aria-haspopup="menu" aria-expanded={!!picking} onClick={(event) => picking ? setPicking(null) : pick(event.currentTarget)}><Icon name="plus" />{t("shell.newSession")}</button>
        : <p>{t("shell.noEnvironments")}</p>}
    </div>}
    {picking && <EnvironmentMenu envs={envs} at={picking} choose={openWith} close={closePicker} />}
    {tabMenu && (() => {
      const tab = tabs.find((item) => item.id === tabMenu.id);
      if (!tab) return null;
      const index = tabs.indexOf(tab);
      const closable = (list: ShellTab[]) => list.filter((item) => !item.pinned && !item.busy);
      const others = closable(tabs.filter((item) => item !== tab));
      const left = closable(tabs.slice(0, index));
      const right = closable(tabs.slice(index + 1));
      const done = () => { setTabMenu(null); document.getElementById(`shell-tab-${tab.id}`)?.focus(); };
      return <FloatingMenu at={tabMenu} label={tabLabel(tab)} close={done}><MenuItems close={done} items={[
        { label: t("shell.renameSession"), keys: "F2", run: () => rename(tab) },
        { label: t(tab.pinned ? "shell.unpinSession" : "shell.pinSession"), run: () => pin(tab) },
        null,
        { label: t("shell.closeTab"), keys: "Ctrl+W", disabled: tab.busy, run: () => close([tab]) },
        { label: t("shell.closeOthers"), disabled: !others.length, run: () => close(others, tab) },
        { label: t("shell.closeLeft"), disabled: !left.length, run: () => close(left, tab) },
        { label: t("shell.closeRight"), disabled: !right.length, run: () => close(right, tab) },
      ]} /></FloatingMenu>;
    })()}
    {tabs.map((tab) => <div className="shell-panel" key={tab.id} role="tabpanel" id={`shell-panel-${tab.id}`}
      aria-labelledby={`shell-tab-${tab.id}`} hidden={activeId !== tab.id}>
      <ShellSession tab={tab} envs={envs} active={visible && activeId === tab.id} onChange={refreshTabs} onPersist={persist} saveFailed={saveFailed} fontSize={fontSize} setFontSize={setFontSize}
        aliases={aliases} setAliases={setAliases}
        queued={tab.runs?.length ?? 0} />
    </div>)}
  </div>;
}

// `tab` keeps its identity while the session mutates it; the session re-renders from its own state.
const ShellSession = memo(function ShellSession({ tab, envs, active, onChange, onPersist, saveFailed, fontSize, setFontSize, aliases, setAliases, queued }: {
  tab: ShellTab; envs: Environment[]; active: boolean; onChange: () => void; onPersist: () => void; saveFailed: boolean;
  /** How many commands wait in `tab.runs`; a change wakes the session to run them. */
  queued: number;
  fontSize: number; setFontSize: (size: number) => void;
  aliases: Record<string, string>; setAliases: (update: (aliases: Record<string, string>) => Record<string, string>) => void;
}) {
  const { t } = useTranslation();
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [session, setSession] = useState<Session>(tab.session);
  const [input, setInput] = useWindowState(`shell.${tab.id}.input`, "");
  const [cursor, setCursor] = useState<number | null>(null);
  const [busy, setBusyState] = useState(false);
  // A command that ends while the tab is not in front (another tab or page) leaves a mark on the tab until it is shown.
  // Connecting does not: it is not something the user waits to read.
  const shown = useRef(active);
  shown.current = active;
  const setBusy = (value: boolean) => {
    if (!mounted.current) return;
    if (!value && tab.busy && !shown.current && !quiet.current) tab.unseen ??= "done";
    tab.busy = value; setBusyState(value); onChange();
  };
  useEffect(() => { if (active && tab.unseen) { tab.unseen = undefined; onChange(); } }, [active, tab, onChange]);
  const [started, setStarted] = useState(0);
  // Ctrl+C bumps the ticket: the call keeps running in Odoo, but its stale result is ignored.
  const ticket = useRef(0);
  const writing = useRef(false);
  const quiet = useRef(false);
  const begin = (write = false, connect = false) => {
    const id = ++ticket.current;
    writing.current = write;
    quiet.current = connect;
    setStarted(Date.now());
    setBusy(true);
    return () => ticket.current === id;
  };
  const finish = (current: () => boolean) => { if (current()) setBusy(false); };
  const discard = () => {
    ticket.current++;
    setBusy(false);
    print({ kind: "input", prompt: prompt(), text: "^C" });
    text("warn", t(writing.current ? "shell.discardedWrite" : "shell.discarded"));
    stopBatch();
  };
  const [confirming, setConfirming] = useWindowState<Confirmation | null>(`shell.${tab.id}.confirming`, null, (p) => p?.agent ? null : p);
  // Mirrors for the agent flow, which waits across renders.
  const confirmingNow = useRef(confirming);
  confirmingNow.current = confirming;
  /** The agent a running command belongs to: its calls carry its id, for the core's checks and the audit. */
  const acting = useRef<AgentRun | null>(null);
  const shell = (call: ShellCall) => ipc.shell(acting.current ? { ...call, client_id: acting.current.clientId, client_revision: acting.current.clientRevision } : call);
  // An agent's confirmation and run end with its reply; closing the window would drop both.
  usePreventUnload(busy || !!confirming?.agent);
  const [menu, setMenu] = useState<Menu | null>(null);
  const menuList = useRef<HTMLUListElement>(null);
  const out = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLTextAreaElement>(null);

  // New output, a call or a prompt brings the end into view; folding or toggling an earlier table keeps the reader's place.
  const lastEntry = session.entries[session.entries.length - 1]?.id;
  useLayoutEffect(() => { if (active) out.current?.scrollTo({ top: out.current.scrollHeight }); }, [lastEntry, busy, confirming, active]);
  useEffect(() => {
    if (active && !busy && !document.activeElement?.closest(".shell-tabs")) field.current?.focus();
  }, [busy, confirming, active]);

  const update = (f: (s: Session) => Session) => {
    if (!mounted.current) return;
    const previous = tab.session.env?.name;
    tab.session = f(tab.session);
    setSession(tab.session);
    if (previous !== tab.session.env?.name) onChange();
    onPersist();
  };

  // Connect on mount when opened from Environments.
  useEffect(() => {
    if (tab.session.history !== null) return;
    void ipc.shellHistory().then((history) => update((s) => ({ ...s, history })))
      .catch(() => update((s) => ({ ...s, history: [] })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  /** Ctrl+R as in bash: `query` typed in place of the line, `skip` older matches passed with Ctrl+R, `original` the line before. */
  const [search, setSearch] = useState<{ query: string; skip: number; original: string } | null>(null);
  const [help, setHelp] = useState(false);
  /** Why the last connection attempt failed; formatted when shown, so it follows the language. */
  const [failure, setFailure] = useState<ConnectionErrorPresentation | AppError | null>(null);
  const failureNow = useRef(failure);
  failureNow.current = failure;
  /** What the last successful check reported, for the mode chip's tooltip. */
  const [server, setServer] = useState<{ version?: string; latency: number } | null>(null);
  /** Lines pasted together, each a command, waiting for "run in order" or "paste as text". */
  const [pasted, setPasted] = useState<string[] | null>(null);
  /** Pasted commands running one by one; `mark` is the first entry id of the current step, to spot its errors. */
  const [batch, setBatch] = useState<{ lines: string[]; total: number; mark: number } | null>(null);
  /** A cancelled confirmation or a discarded call stops a pasted batch, whichever way it was cancelled. */
  const stopBatch = () => { if (batch) { setBatch(null); text("warn", t("shell.batchStopped", { index: batch.total - batch.lines.length, total: batch.total })); } };
  const stepping = useRef(false);
  /** A line starts with a command or an alias; indented or option lines continue a multi-line command instead. */
  const isCommand = (line: string) => { const word = line.split(/\s+/)[0]; return VERBS.includes(word) || META.includes(word) || word in aliases; };
  const onPaste = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    if (confirming || batch || input.trim()) return;
    const lines = event.clipboardData.getData("text").split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    if (lines.length < 2 || !lines.every(isCommand)) return;
    event.preventDefault();
    setPasted(lines);
  };
  const runPasted = () => {
    if (!pasted) return;
    if (!tab.session.env && pasted.some(odooCommand)) { setHint(t(connecting ? "shell.waitConnection" : "shell.needsConnection")); setNudge((n) => n + 1); return; }
    setBatch({ lines: pasted, total: pasted.length, mark: peekEntryId() });
    setPasted(null);
  };
  // One step at a time: wait for the previous call and any write confirmation, stop at the first error.
  useEffect(() => {
    if (!batch || busy || confirming || stepping.current) return;
    const failed = tab.session.entries.findIndex((entry) => entry.id >= batch.mark && entry.kind === "error") >= 0;
    const index = batch.total - batch.lines.length;
    if (failed) { setBatch(null); return text("warn", t("shell.batchStopped", { index, total: batch.total })); }
    if (!batch.lines.length) return setBatch(null);
    const [line, ...rest] = batch.lines;
    stepping.current = true;
    setBatch({ ...batch, lines: rest, mark: peekEntryId() });
    void submit(line).finally(() => { stepping.current = false; setBatch((current) => current && { ...current }); });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [batch, busy, confirming]);
  // Queued commands, one at a time: after the user's own call, connected, and through any write confirmation.
  const serving = useRef(false);
  useEffect(() => {
    const run = tab.runs?.[0];
    if (!run || serving.current) return;
    serving.current = true;
    void serveRun(run).finally(() => {
      tab.runs = tab.runs?.filter((item) => item !== run);
      serving.current = false;
      onChange();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queued]);
  /** A command from another page replaces a pending confirmation, as the user asked for it; an agent waits its turn. */
  async function serveRun({ command, agent }: QueuedRun) {
    const until = async (done: () => boolean) => { while (mounted.current && !done()) await new Promise((resolve) => setTimeout(resolve, 50)); };
    const idle = () => !tab.busy && !confirmingNow.current;
    const reply = (result?: unknown, error?: string) => { if (agent) void ipc.agentReply(agent.id, result, error).catch(() => undefined); };
    if (!agent && confirmingNow.current) { setConfirming(null); confirmingNow.current = null; text("info", t("shell.cancelled")); }
    await until(idle);
    if (!tab.session.env) { reconnect(); await until(() => !tab.busy); }
    if (!tab.session.env) return reply(undefined, failureNow.current ? connectionProblem(failureNow.current) : t("shell.noConnection"));
    acting.current = agent ?? null;
    const mark = peekEntryId();
    try {
      await submit(command, agent?.clientLabel);
      await until(() => !tab.busy);
      // A write the client may not request is withdrawn before the user is asked.
      if (agent && confirmingNow.current && !agent.allowWrites) {
        setConfirming(null);
        confirmingNow.current = null;
        text("error", t("shell.agentWriteDenied", { name: agent.clientLabel }));
      }
      await until(idle);
    } finally { acting.current = null; }
    const output = tab.session.entries.filter((entry) => entry.id >= mark).map(plainEntry);
    reply({ terminal: tab.id, env: tab.envName, command, output });
  }
  /** Bumped to shake the status chip when a command needs the connection it lacks. */
  const [nudge, setNudge] = useState(0);
  const odooCommand = (line: string) => needsOdoo(expandAlias(line, aliases));
  /** A Retry in flight keeps the chip in place, spinning, instead of hiding it while busy. */
  const [retrying, setRetrying] = useState(false);
  // A check that answers at once would only flash "Retrying…"; keep it long enough to be seen.
  const retryStarted = useRef(0);
  const RETRY_MIN_MS = 600;
  const editorHighlight = useRef<HTMLDivElement>(null);
  const syncScroll = () => {
    if (!editorHighlight.current || !field.current) return;
    editorHighlight.current.scrollLeft = field.current.scrollLeft;
    editorHighlight.current.scrollTop = field.current.scrollTop;
  };
  // Show parse errors once typing pauses; while typing, every prefix is incomplete.
  const [hint, setHint] = useState<string | null>(null);
  useEffect(() => {
    setHint(null);
    if (!input.trim() || confirming || search || /^!(!|\d+)$/.test(input.trim())) return;
    const timer = setTimeout(() => {
      // While the first word is being typed, the highlighter already marks unknown commands.
      // An open string or bracket means the user is mid-value; Enter still reports it.
      if (!/\s/.test(input.trim()) || unfinished(input)) return;
      try { parse(expand(input)); } catch (e) { if (e instanceof CommandError) setHint(e.message); }
    }, HINT_DELAY_MS);
    return () => clearTimeout(timer);
  }, [input, confirming, search]);

  /** Check the tab's environment and connect; the status chip shows how it goes. */
  const reconnect = () => {
    const env = envs.find((e) => e.id === tab.envId);
    // Disconnected until the health check passes; \next must not reuse the old connection.
    update((s) => ({ ...s, env: undefined, last: undefined }));
    if (!env) return print({ kind: "error", text: t("shell.restoredEnvironmentGone", { name: tab.envName }), transient: true });
    // The environment may have been renamed since the tab was saved.
    if (tab.envName !== env.name) { tab.envName = env.name; onChange(); }
    void open(env);
  };
  // New and restored tabs connect the first time they are shown.
  useEffect(() => {
    if (!active || !tab.reconnect || busy || !envs.length) return;
    tab.reconnect = undefined;
    reconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, envs, busy]);
  const print = (...entries: NewEntry[]) => update((s) => ({ ...s, entries: [...s.entries, ...entries.map((e) => ({ ...e, id: nextEntryId() }) as Entry)].slice(-MAX_LINES) }));
  const text = (kind: "text" | "info" | "error" | "warn" | "ok", t: string) => print({ kind, text: t });
  /** An error as Odoo or the core reported it, in the user's language. */
  const fail = (reason: unknown) => print({ kind: "error", text: "", appError: normalizeAppError(reason) });
  /** `login>`: who the calls run as; the database shows at the end of the input line. */
  const prompt = () => {
    const env = envs.find((e) => e.id === tab.envId);
    if (!env) return "odoo>";
    return `${env.login.length > PROMPT_LOGIN ? `${env.login.slice(0, PROMPT_LOGIN - 1)}…` : env.login}>`;
  };
  /** Alias, then `$_` and `$name`; \\alias keeps its command as typed. */
  const expand = (line: string) => /^\s*\\alias(\s|$)/.test(line) ? line : substitute(expandAlias(line, aliases), tab.session.previous?.ids, tab.session.vars);
  /** The latest table's record ids become `$_`. A table without ids (\cols name) clears it: `$_` must never mean rows no longer shown. */
  const remember = (table: Table) => {
    const idColumn = table.columns.indexOf("id");
    update((s) => ({ ...s, previous: idColumn < 0 ? undefined : { model: table.model, ids: table.rows.map((row) => row[idColumn]).filter((id): id is number => typeof id === "number") } }));
  };

  /** Why the latest environment state refuses writes, mirroring policy::authorize; empty when allowed. */
  const writeBlocked = (id: string) => {
    const env = envs.find((e) => e.id === id);
    if (env?.write_mode === "read_only") return t("shell.readOnlyWrite", { name: env.name });
    if (env?.write_mode === "temporary" && !(env.write_until && Date.parse(env.write_until) > Date.now())) return t("shell.writeWindowExpired", { name: env.name });
    return "";
  };

  async function run(pending: Confirmation, confirmed: boolean) {
    const env = pending.env ?? tab.session.env;
    if (!env) return text("error", t("shell.noConnection"));
    const current = begin(confirmed);
    try {
      const result = await shell({ environment_id: env.id, ...pending.call, context: tab.session.context, confirmed });
      if (!current()) {
        // A discarded read is dropped; a confirmed write still reports what Odoo did.
        if (!confirmed) return;
        text("warn", t("shell.lateResult", { call: `${pending.call.method} ${pending.call.model}` }));
      }
      render(pending, result.data, tab.session.timing ? `${result.upstream_ms} ms · ${size(result.upstream_bytes)}` : "");
      // \next repeats search_read pages only; name_search has no offset.
      if (pending.page && pending.call.method === "search_read") update((s) => ({ ...s, last: pending }));
    } catch (reason) {
      if (!current() && !confirmed) return;
      const e = reason as AppError;
      // The core classifies writes and requires confirmation before sending them.
      if (e?.code === "confirmation_required" && !confirmed) {
        const c = pending.call;
        print({ kind: "warn", text: `${c.method} ${c.model} ${JSON.stringify(c.args)}${Object.keys(c.kwargs).length ? ` ${JSON.stringify(c.kwargs)}` : ""}` });
        // Don't ask for a confirmation the core will refuse anyway; the core still rejects with `forbidden`.
        const blocked = writeBlocked(env.id);
        if (blocked) text("error", blocked);
        // The mirror updates now: an agent run checks it right after this call, before the next render.
        else { confirmingNow.current = { ...pending, env, agent: acting.current?.clientId }; setConfirming(confirmingNow.current); }
      } else if (e?.code === "uncertain_result") print({ kind: "warn", text: "", appError: normalizeAppError(reason) });
      else fail(reason);
    } finally { finish(current); }
  }

  function render(p: Confirmation, data: unknown, meta: string) {
    describe(p.call.model);
    const { entries, previous } = resultEntries(p, data, meta, tab.session.expanded);
    if (previous) update((s) => ({ ...s, previous }));
    print(...entries);
  }

  /** Run several read calls as one command; timing covers all of them. */
  async function sequence(work: (rpc: (call: OdooCall) => Promise<unknown>, env: Environment) => Promise<{ table: Table; footer?: string }>) {
    const env = envs.find((e) => e.id === tab.session.env?.id);
    if (!env) return text("error", t("shell.connectHint"));
    const current = begin();
    let ms = 0;
    let bytes = 0;
    const rpc = async (call: OdooCall) => {
      const r = await shell({ environment_id: env.id, ...call, context: tab.session.context, confirmed: false });
      ms += r.upstream_ms;
      bytes += r.upstream_bytes;
      return r.data;
    };
    try {
      const { table, footer } = await work(rpc, env);
      if (!current()) return;
      remember(table);
      describe(table.model);
      const meta = tab.session.timing ? `${ms} ms · ${size(bytes)}` : "";
      print({ kind: "table", expanded: tab.session.expanded, table, footer: [footer, meta].filter(Boolean).join(" · ") || undefined });
    } catch (reason) {
      if (!current()) return;
      if (reason instanceof CommandError) text("error", reason.message);
      else fail(reason);
    } finally { finish(current); }
  }

  /** The vault stores the login, not the uid; logins are unique per database. */
  async function currentUser(rpc: (call: OdooCall) => Promise<unknown>, env: Environment, fields: string[]) {
    const users = await rpc({ model: "res.users", method: "search_read", args: [], kwargs: { domain: [["login", "=", env.login]], fields, limit: 1 } }) as Record<string, unknown>[];
    if (!users.length) throw new CommandError(t("shell.userNotFound", { login: env.login }));
    return users[0];
  }

  /** Connection state lives on the status chip, not in the output: failures in its tooltip, the server on the mode's. */
  async function open(env: Environment) {
    const current = begin(false, true);
    setFailure(null);
    try {
      const health = await ipc.check(env.id);
      if (!current()) return;
      if (health.status !== "ok") return setFailure({
        name: env.name,
        reason: health.status === "auth_failed" ? "credential" : "offline",
        detail: health.error_code ? { code: health.error_code, params: health.error_params ?? undefined, detail: health.error ?? undefined } : undefined,
      });
      update((s) => ({ ...s, env: { id: env.id, name: env.name }, last: undefined }));
      setServer({ version: health.server_version ?? undefined, latency: health.latency_ms });
      if (!tab.session.models[env.id]) {
        void loadModels(env.id).catch(() => undefined);
      }
    } catch (reason) { if (current()) setFailure(normalizeAppError(reason)); } finally {
      finish(current);
      const left = RETRY_MIN_MS - (Date.now() - retryStarted.current);
      if (left > 0) setTimeout(() => { if (mounted.current) setRetrying(false); }, left); else setRetrying(false);
    }
  }

  /** Relation of a field, cached per environment. */
  const relations = useRef(new Map<string, string | undefined>());
  async function relationOf(envId: string, model: string, field: string) {
    const key = `${envId}:${model}.${field}`;
    if (!relations.current.has(key)) {
      const r = await shell({ environment_id: envId, model, method: "fields_get", args: [], kwargs: { allfields: [field], attributes: ["relation"] } });
      const relation = (r.data as Record<string, { relation?: string | false }>)[field]?.relation;
      relations.current.set(key, relation || undefined);
    }
    return relations.current.get(key);
  }

  /** A click runs a visible command, so it lands in history like a typed one; Ctrl+click opens the record in Odoo. */
  async function follow({ model, column, value, row }: Link, inOdoo = false) {
    const env = tab.session.env;
    if (!env || (!inOdoo && (busy || confirming))) return;
    let target: string | undefined;
    let ids: number[] = [];
    if (column === "id") { target = model; ids = [value as number]; }
    else if (column === "res_id" && model === "ir.model.data") { target = String(row.model); ids = [value as number]; }
    else {
      try { target = await relationOf(env.id, model, column); } catch (reason) { return fail(reason); }
      if (!target) return text("error", t("shell.noRelation", { field: `${model}.${column}` }));
      ids = isM2o(value) ? [value[0]] : isIds(value) ? value : [];
    }
    if (inOdoo) {
      if (ids.length !== 1) return text("error", t("shell.openOneRecord"));
      return ipc.openUrl(env.id, { model: target, id: ids[0] }).catch((reason) => fail(reason));
    }
    if (isIds(value) && column !== "id" && column !== "res_id") return submit(`search ${target} -d [('id','in',[${ids.join(",")}])]`);
    return submit(`read ${target} ${ids[0]} -f *`);
  }

  async function exportTable(table: Table, format: "csv" | "json") {
    try {
      const path = await ipc.shellExport(exportName(table, format), format === "csv" ? toCsv(table) : toJson(table));
      text("ok", t("shell.exported", { path }));
    } catch (reason) { fail(reason); }
  }

  const fold = (id: number) => update((s) => ({ ...s, entries: s.entries.map((e) => e.id === id && e.kind === "input" ? { ...e, folded: !e.folded } : e) }));
  const rerun = (command: string) => { if (!busy && !confirming) void submit(command); };

  const [find, setFind] = useState<{ query: string; index: number } | null>(null);
  const [matches, setMatches] = useState<Range[]>([]);
  const findField = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const root = out.current;
    if (!root || !find?.query) { setMatches([]); return; }
    const q = find.query.toLowerCase();
    const found: Range[] = [];
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node && found.length < 1000; node = walker.nextNode()) {
      const value = node.nodeValue?.toLowerCase() ?? "";
      for (let at = value.indexOf(q); at >= 0 && found.length < 1000; at = value.indexOf(q, at + q.length)) {
        const range = document.createRange();
        range.setStart(node, at);
        range.setEnd(node, at + q.length);
        found.push(range);
      }
    }
    setMatches(found);
  }, [find?.query, session.entries]);
  const findIndex = matches.length && find ? ((find.index % matches.length) + matches.length) % matches.length : 0;
  useEffect(() => {
    // The CSS Custom Highlight API paints matches without touching React's DOM.
    const registry = (globalThis.CSS as unknown as { highlights?: Map<string, unknown> } | undefined)?.highlights;
    const Highlight = (globalThis as unknown as { Highlight?: new (...ranges: Range[]) => unknown }).Highlight;
    if (registry && Highlight) {
      registry.delete("term-find");
      registry.delete("term-find-current");
      if (matches.length) {
        registry.set("term-find", new Highlight(...matches));
        registry.set("term-find-current", new Highlight(matches[findIndex]));
      }
    }
    matches[findIndex]?.startContainer.parentElement?.scrollIntoView?.({ block: "nearest" });
  }, [matches, findIndex]);
  const openFind = () => {
    setFind((current) => current ?? { query: "", index: 0 });
    requestAnimationFrame(() => { findField.current?.focus(); findField.current?.select(); });
  };
  const closeFind = () => { setFind(null); field.current?.focus(); };

  /** Ctrl+F and font zoom work from the editor and the find bar alike. */
  function onTermKey(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "F1") { event.preventDefault(); setHelp((open) => !open); return; }
    if (event.key === "Escape" && help) { event.preventDefault(); setHelp(false); field.current?.focus(); return; }
    const inTable = (event.target as HTMLElement).classList.contains("term-scroll");
    if (inTable && event.key === "Escape") { event.preventDefault(); field.current?.focus(); return; }
    // Typing in a table goes back to the editor; the key lands there.
    if (inTable && event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey && event.key.toLowerCase() !== "c") { field.current?.focus(); return; }
    if ((event.ctrlKey || event.metaKey) && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      // Ctrl+↑ from the editor enters the newest table; inside, Ctrl+↑/↓ moves between tables.
      const tables = [...(out.current?.querySelectorAll<HTMLElement>(".term-scroll[tabindex]") ?? [])];
      const at = tables.indexOf(event.target as HTMLElement);
      const next = at < 0 ? (event.key === "ArrowUp" ? tables.length - 1 : -1) : at + (event.key === "ArrowUp" ? -1 : 1);
      event.preventDefault();
      if (next >= 0 && next < tables.length) tables[next].focus();
      else if (at >= 0 && next >= tables.length) field.current?.focus();
      return;
    }
    if (!event.ctrlKey && !event.metaKey) return;
    const key = event.key.toLowerCase();
    if (key === "f") { event.preventDefault(); openFind(); }
    else if (key === "+" || key === "=") { event.preventDefault(); setFontSize(fontSize + 1); }
    else if (key === "-") { event.preventDefault(); setFontSize(fontSize - 1); }
    else if (key === "0") { event.preventDefault(); setFontSize(DEFAULT_FONT); }
  }

  const [inspecting, setInspecting] = useState<{ title: string; text: string } | null>(null);
  function inspect(title: string, value: unknown) {
    let text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
    // Pretty-print text fields that hold JSON.
    if (typeof value === "string" && /^\s*[[{]/.test(value)) { try { text = JSON.stringify(JSON.parse(value), null, 2); } catch { /* plain text */ } }
    setInspecting({ title, text });
  }

  /** Enter on a line typed or found with Ctrl+R. */
  function send(line: string) {
    // An empty line does nothing; only a confirmation reads it, as its default "no".
    if (!line.trim() && !confirming) return setInput(line);
    // Without a connection, a command for Odoo is not sent nor kept in history: it stays typed for when it connects.
    if (!confirming && !tab.session.env && odooCommand(line)) {
      setInput(line);
      setHint(t(connecting ? "shell.waitConnection" : "shell.needsConnection"));
      setNudge((n) => n + 1);
      return;
    }
    setInput(""); setCursor(null); void submit(line);
  }
  async function submit(line: string, agent?: string) {
    // The live prompt: a queued run calls the submit of the render that queued it.
    const p = confirmingNow.current;
    if (p) {
      const yes = /^(y|s|si|sí|yes)$/i.test(line.trim());
      print({ kind: "input", prompt: t("shell.confirmPrompt", { name: p.env?.name }), text: line });
      confirmingNow.current = null;
      setConfirming(null);
      // The temporary window may have closed while the prompt was open.
      const blocked = yes && p.env ? writeBlocked(p.env.id) : "";
      if (blocked) text("error", blocked);
      else if (yes) await run(p, true);
      else { text("info", t("shell.cancelled")); stopBatch(); }
      return;
    }
    // Load !n / !! into the input for review before execution.
    const bang = /^!(!|\d+)$/.exec(line.trim());
    if (bang) {
      const all = tab.session.history ?? [];
      const entry = bang[1] === "!" ? all[all.length - 1] : all[Number(bang[1]) - 1];
      if (!entry) return text("error", t("shell.missingHistoryCommand", { entry: bang[1] === "!" ? t("shell.previous") : t("shell.numbered", { number: bang[1] }) }));
      setInput(entry.command);
      return;
    }
    // The greeting and restore notes only matter until the first command actually runs.
    update((s) => ({ ...s, entries: s.entries.filter((entry) => !(entry.kind === "info" && (entry.translationKey === "shell.initial" || entry.transient))) }));
    print({ kind: "input", prompt: prompt(), text: line, command: !!line.trim(), ...agent ? { agent } : {} });
    // Looking at help or history, or clearing the screen, is not work worth recalling.
    if (line.trim() && !UNRECALLED.has(line.trim().split(/\s+/)[0])) {
      const entry = { at: new Date().toISOString(), environment: tab.session.env?.name ?? null, command: line.trim() };
      // Like shell_history_add, skip only consecutive duplicates in the same environment.
      update((s) => {
        const history = s.history ?? [];
        const last = history[history.length - 1];
        if (last?.command === entry.command && last.environment === entry.environment) return s;
        return { ...s, history: [...history, entry].slice(-500) };
      });
      void ipc.shellHistoryAdd(entry.command, entry.environment ?? undefined).catch(() => undefined);
    }
    let command: Command | null;
    const previous = tab.session.previous;
    let expanded: string;
    try { expanded = expand(line); command = parse(expanded); } catch (e) { return text("error", e instanceof CommandError ? e.message : String(e)); }
    if (!command) return;
    // Ids from another model are usually a mistake (ref leaves ir.model.data ids), so say where they come from.
    const target = command.kind === "call" ? command.call.model : command.kind === "metadata" ? command.model : undefined;
    if (target && command.kind !== "alias") for (const name of new Set(Array.from(expandAlias(line, aliases).matchAll(/\$([A-Za-z_]\w*)/g), (m) => m[1]))) {
      const source = name === "_" ? previous : tab.session.vars?.[name];
      if (source?.model && source.model !== target) text("warn", t("shell.previousFromModel", { name: `$${name}`, model: source.model, target }));
    }
    switch (command.kind) {
      case "help": return setHelp(true);
      case "clear": return update((s) => ({ ...s, entries: [] }));
      case "history": {
        if (command.clear) {
          await ipc.shellHistoryClear().catch((e) => fail(e));
          update((s) => ({ ...s, history: [] }));
          return text("info", t("shell.historyCleared"));
        }
        const q = command.q?.toLowerCase();
        const all = tab.session.history ?? [];
        // The number !n recalls, when and what; a click on a command puts it in the input. The search still matches the environment.
        const rows = all.map((h, i) => ({ h, n: i + 1 })).filter(({ h }) => !q || h.command.toLowerCase().includes(q) || (h.environment ?? "").toLowerCase().includes(q))
          .map(({ h, n }) => [n, formatDateTime(new Date(h.at), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" }), h.command] as unknown[])
          .slice(-50);
        if (!rows.length) return text("info", q ? t("shell.noCommandMatches") : t("shell.emptyHistory"));
        return print({ kind: "table", expanded: false, table: { columns: ["#", t("shell.columns.date"), t("shell.columns.command")], rows, code: [2] }, footer: t("shell.historyFooter", { shown: rows.length, total: all.length }) });
      }
      case "timing": {
        const timing = !tab.session.timing;
        update((s) => ({ ...s, timing }));
        return text("info", t("shell.timing", { state: timing ? t("common.visible") : t("common.hidden") }));
      }
      case "expanded": {
        const expanded = !tab.session.expanded;
        update((s) => ({ ...s, expanded }));
        return text("info", t("shell.expanded", { state: expanded ? t("common.enabled") : t("common.disabled") }));
      }
      case "list":
        if (!envs.length) return text("info", t("shell.noEnvironments"));
        return print({ kind: "table", expanded: false, table: { columns: [t("common.environment"), t("common.database"), t("common.url"), t("shell.writeMode")], rows: envs.map((e) => [e.name, e.database, e.url, t(modeLabels[e.write_mode] as "mode.readOnly")]) } });
      case "context":
        if (command.value === undefined) return text("text", JSON.stringify(tab.session.context));
        update((s) => ({ ...s, context: command.value ?? {} }));
        return text("info", command.value ? t("shell.contextSet") : t("shell.contextCleared"));
      case "next": case "prev": {
        const last = tab.session.last;
        if (!last?.page) return text("error", t("shell.noPreviousSearch"));
        if (command.kind === "prev" && !last.page.offset) return text("info", t("shell.firstPage"));
        const offset = command.kind === "next" ? last.page.offset + last.page.limit : Math.max(0, last.page.offset - last.page.limit);
        const kw = last.call.kwargs as { domain: unknown[]; fields?: string[]; order?: string };
        return run({ ...last, page: { ...last.page, offset }, call: searchCall(last.call.model, kw.domain, kw.fields, last.page.limit, offset, kw.order) }, false);
      }
      case "set": {
        const vars = tab.session.vars ?? {};
        const { name } = command;
        if (!name) {
          const rows = Object.entries(vars).map(([key, v]) => [`$${key}`, v.model ?? "", v.ids.length, v.ids.join(",")]);
          if (!rows.length) return text("info", t("shell.noVariables"));
          return print({ kind: "table", expanded: false, table: { columns: [t("shell.variable"), t("shell.model"), t("shell.records"), "ids"], rows } });
        }
        if (command.remove) {
          update((s) => ({ ...s, vars: Object.fromEntries(Object.entries(vars).filter(([key]) => key !== name)) }));
          return text("info", t("shell.variableRemoved", { name: `$${name}` }));
        }
        if (!previous?.ids.length) return text("error", t("shell.errors.noPreviousIds"));
        update((s) => ({ ...s, vars: { ...vars, [name]: previous } }));
        return text("ok", t("shell.variableSet", { name: `$${name}`, count: previous.ids.length, model: previous.model ?? "" }));
      }
      case "alias": {
        const { name } = command;
        if (!name) {
          const rows = Object.entries(aliases).sort(([a], [b]) => a.localeCompare(b));
          if (!rows.length) return text("info", t("shell.noAliases"));
          return print({ kind: "table", expanded: false, table: { columns: ["alias", t("shell.columns.command")], rows, code: [1] } });
        }
        if (command.command === null) {
          setAliases((current) => Object.fromEntries(Object.entries(current).filter(([key]) => key !== name)));
          return text("info", t("shell.aliasRemoved", { name }));
        }
        const body = command.command ?? "";
        setAliases((current) => ({ ...current, [name]: body }));
        return text("ok", t("shell.aliasSet", { name }));
      }
      case "grep": case "cols": {
        const source = [...tab.session.entries].reverse().find((e): e is Extract<Entry, { kind: "table" }> => e.kind === "table");
        if (!source) return text("error", t("shell.noTable"));
        const { table } = source;
        const next = reshapeTable(table, command);
        if ("missing" in next) return text("error", t("shell.unknownColumns", { columns: next.missing.join(", "), available: table.columns.join(", ") }));
        remember(next);
        return print({ kind: "table", expanded: source.expanded, table: next, footer: t("shell.filtered", { shown: next.rows.length, count: table.rows.length }) });
      }
      case "whoami": return sequence(async (rpc, env) => {
        const user = await currentUser(rpc, env, ["login", "name", "company_id", "company_ids"]);
        return { table: { columns: ["uid", "login", t("shell.name"), "company_id", "company_ids"], rows: [[user.id, user.login, user.name, user.company_id, user.company_ids]], model: "res.users" } };
      });
      case "metadata": {
        const { model, ids } = command;
        return sequence(async (rpc) => {
          const records = await rpc({ model, method: "read", args: [ids], kwargs: { fields: ["create_uid", "create_date", "write_uid", "write_date"] } }) as Record<string, unknown>[];
          const xmlids = await rpc({ model, method: "get_external_id", args: [ids], kwargs: {} }) as Record<string, string>;
          const rows = records.map((r) => [r.id, xmlids[String(r.id)] || false, r.create_uid, r.create_date, r.write_uid, r.write_date]);
          return { table: { columns: ["id", "xmlid", "create_uid", "create_date", "write_uid", "write_date"], rows, model }, footer: t("shell.row", { count: rows.length }) };
        });
      }
      case "access": {
        const { model } = command;
        return sequence(async (rpc) => {
          const rows: unknown[][] = [];
          for (const operation of ["read", "write", "create", "unlink"]) {
            const allowed = await rpc({ model, method: "check_access_rights", args: [], kwargs: { operation, raise_exception: false } });
            rows.push([operation, allowed === true ? t("shell.yes") : t("shell.no")]);
          }
          return { table: { columns: [t("shell.operation"), t("shell.allowed")], rows } };
        });
      }
      case "has_group": {
        const { group } = command;
        return sequence(async (rpc, env) => {
          const [module, name] = [group.slice(0, group.indexOf(".")), group.slice(group.indexOf(".") + 1)];
          const refs = await rpc({ model: "ir.model.data", method: "search_read", args: [], kwargs: { domain: [["module", "=", module], ["name", "=", name]], fields: ["model", "res_id"], limit: 2 } }) as { model: string; res_id: number }[];
          if (refs.length !== 1 || refs[0].model !== "res.groups") throw new CommandError(t("shell.groupNotFound", { group }));
          // Odoo 19 renamed groups_id; all_group_ids includes implied groups, group_ids does not.
          const described = await rpc({ model: "res.users", method: "fields_get", args: [], kwargs: { allfields: GROUP_FIELDS, attributes: ["type"] } }) as Record<string, unknown>;
          const field = GROUP_FIELDS.find((f) => f in described);
          if (!field) throw new CommandError(t("shell.noGroupField"));
          const member = await rpc({ model: "res.users", method: "search_count", args: [[["login", "=", env.login], [field, "in", [refs[0].res_id]]]], kwargs: {} });
          return { table: { columns: [t("shell.group"), "id", "login", t("shell.member")], rows: [[group, refs[0].res_id, env.login, member ? t("shell.yes") : t("shell.no")]] }, footer: field === "group_ids" ? t("shell.directGroupsOnly") : undefined };
        });
      }
      case "call": {
        if (!tab.session.env) return text("error", t("shell.connectHint"));
        const pending: Confirmation = { call: command.call, display: command.display, filter: command.filter, page: command.page, env: tab.session.env };
        return run(pending, false);
      }
    }
  }

  const context = (sess: Session = tab.session) => ({
    models: sess.env ? sess.models[sess.env.id] : undefined,
    fields: (model: string) => (sess.env ? sess.fields[`${sess.env.id}:${model}`] : undefined),
    connected: !!sess.env,
    aliases,
  });

  // Shared with autocompletion: once loaded for a model, both use the same fields.
  const fieldLoads = useRef(new Set<string>());
  const tableFields = (model: string) => tab.session.env ? session.fields[`${tab.session.env.id}:${model}`] : undefined;
  /** After a fresh result only: restoring old output never calls Odoo. */
  const describe = (model?: string) => {
    const env = tab.session.env;
    if (!env || !model) return;
    const key = `${env.id}:${model}`;
    if (tab.session.fields[key] || fieldLoads.current.has(key)) return;
    fieldLoads.current.add(key);
    void load({ fields: model }).catch(() => fieldLoads.current.delete(key));
  };
  /** Model names for completion, once per environment. */
  async function loadModels(envId: string) {
    const r = await shell({ environment_id: envId, model: "ir.model", method: "search_read", args: [], kwargs: { fields: ["model"], limit: 5000 } });
    update((s) => ({ ...s, models: { ...s.models, [envId]: (r.data as { model: string }[]).map((m) => m.model).sort() } }));
  }
  /** Load missing models or fields; report whether loading was needed. */
  async function load(needs: NonNullable<Completion["needs"]>) {
    const env = tab.session.env;
    if (!env) return;
    if ("models" in needs) await loadModels(env.id);
    else {
      const r = await shell({ environment_id: env.id, model: needs.fields, method: "fields_get", args: [], kwargs: { attributes: ["string", "type", "selection", "relation"] } });
      const fields = Object.entries(r.data as Record<string, { string: string; type: string; relation?: string | false; selection?: [string, string][] }>)
        .map(([name, f]) => ({ name, label: f.string, type: f.type, ...(f.relation ? { relation: f.relation } : {}), ...(Array.isArray(f.selection) ? { selection: f.selection } : {}) }))
        .sort((a, b) => a.name.localeCompare(b.name));
      update((s) => ({ ...s, fields: { ...s.fields, [`${env.id}:${needs.fields}`]: fields } }));
    }
  }

  function accept(m: Menu, value = input) {
    const item = m.items[m.index];
    if (!item) return;
    setInput(value.slice(0, m.from) + item.value + m.suffix);
    setMenu(null);
  }

  /** Tab completes a single match or opens the menu for multiple matches. */
  async function openMenu(value: string) {
    let found = completions(value, context());
    if (found?.needs) {
      try { await load(found.needs); } catch { return text("error", t("shell.suggestionsError")); }
      // The line changed (or ran) while the names loaded: completing the old text would bring it back.
      if (field.current?.value !== value) return;
      found = completions(value, context(tab.session));
    }
    if (!found || !found.items.length) { setMenu(null); return; }
    const m = { ...found, index: 0 };
    if (found.items.length === 1) accept(m, value); else setMenu(m);
  }

  /** Typing filters an open completion menu. */
  function refilter(value: string) {
    if (!menu) return;
    const found = completions(value, context());
    setMenu(found && found.items.length ? { ...found, index: 0 } : null);
  }

  useLayoutEffect(() => {
    menuList.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [menu]);

  function onKey(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (pasted) {
      if (event.key === "Enter") { event.preventDefault(); runPasted(); return; }
      if (event.key === "Escape") { event.preventDefault(); setInput(pasted.join("\n")); setPasted(null); return; }
    }
    if (inspecting && event.key === "Escape") { event.preventDefault(); setInspecting(null); return; }
    if (menu) {
      const move = (step: number) => setMenu({ ...menu, index: (menu.index + step + menu.items.length) % menu.items.length });
      if (event.key === "Tab") { event.preventDefault(); move(event.shiftKey ? -1 : 1); return; }
      if (event.key === "ArrowDown") { event.preventDefault(); move(1); return; }
      if (event.key === "ArrowUp") { event.preventDefault(); move(-1); return; }
      if (event.key === "Enter" || event.key === "ArrowRight") { event.preventDefault(); accept(menu); return; }
      if (event.key === "Escape") { event.preventDefault(); setMenu(null); return; }
    }
    if (search) {
      const match = findMatch(search.query, search.skip);
      if (event.ctrlKey && event.key.toLowerCase() === "r") { event.preventDefault(); if (findMatch(search.query, search.skip + 1)) setSearch({ ...search, skip: search.skip + 1 }); return; }
      // Enter runs what was found; Ctrl+G or Ctrl+C gives the line back as it was; any other move leaves the match to edit.
      if (event.ctrlKey && ["g", "c"].includes(event.key.toLowerCase())) { event.preventDefault(); setSearch(null); setInput(search.original); return; }
      const found = match?.command ?? search.original;
      if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); setSearch(null); if (busy) setInput(found); else send(found); return; }
      if (["Escape", "Tab", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
        event.preventDefault(); setSearch(null); setInput(found); setCursor(event.key === "Home" ? 0 : found.length); return;
      }
      return;
    }
    if (event.ctrlKey && event.key.toLowerCase() === "r" && !confirming) { event.preventDefault(); setMenu(null); setSearch({ query: "", skip: 0, original: input }); setInput(""); return; }
    if (busy) {
      if ((event.ctrlKey && event.key.toLowerCase() === "c" && !window.getSelection()?.toString()) || event.key === "Escape") { event.preventDefault(); discard(); return; }
      if (event.key === "Enter") { event.preventDefault(); return; }
    }
    if (ghost && (event.key === "ArrowRight" || event.key === "End") && event.currentTarget.selectionStart === input.length) {
      event.preventDefault();
      setInput(input + ghost);
      setCursor(null);
      return;
    }
    // Shift+Enter inserts a newline for long values and domains.
    if (event.key === "Enter" && event.shiftKey && !confirming) return;
    if (event.key === "Enter") {
      event.preventDefault();
      send(input);
      return;
    }
    if (event.key === "Tab") { event.preventDefault(); if (!confirming) void openMenu(input); return; }
    if (event.ctrlKey && event.key.toLowerCase() === "l") { event.preventDefault(); update((s) => ({ ...s, entries: [] })); return; }
    if ((event.ctrlKey && event.key.toLowerCase() === "c" && !window.getSelection()?.toString()) || event.key === "Escape") {
      event.preventDefault();
      if (confirming) { confirmingNow.current = null; setConfirming(null); text("info", t("shell.cancelled")); stopBatch(); }
      else if (input) { print({ kind: "input", prompt: prompt(), text: `${input}^C` }); setInput(""); }
      return;
    }
    if (confirming) return;
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && !event.ctrlKey && !event.metaKey) {
      // Inside a multi-line command the arrows move between lines; history starts at the edges.
      const el = event.currentTarget;
      if (event.key === "ArrowUp" && el.value.slice(0, el.selectionStart).includes("\n")) return;
      if (event.key === "ArrowDown" && el.value.slice(el.selectionEnd).includes("\n")) return;
      event.preventDefault();
      const h = (tab.session.history ?? []).map((e) => e.command);
      if (!h.length) return;
      const next = cursor === null ? (event.key === "ArrowUp" ? h.length - 1 : null) : cursor + (event.key === "ArrowUp" ? -1 : 1);
      if (next === null || next >= h.length) { setCursor(null); setInput(""); return; }
      const i = Math.max(0, next);
      setCursor(i);
      setInput(h[i]);
    }
  }

  /** Match containing `query`, with `skip=0` selecting the newest. */
  function findMatch(query: string, skip: number): HistoryEntry | undefined {
    const q = query.toLowerCase();
    return [...(tab.session.history ?? [])].reverse().filter((h) => !q || h.command.toLowerCase().includes(q))[skip];
  }
  const match = search ? findMatch(search.query, search.skip) : undefined;
  // As in bash, a query that stops matching keeps showing the last match its beginning found.
  const shownMatch = search && (match ?? Array.from({ length: search.query.length }, (_, n) => findMatch(search.query.slice(0, search.query.length - n - 1), 0)).find(Boolean));
  // fish-style suggestion: the newest single-line history command that extends the input.
  const ghost = !search && !confirming && !menu && input.trim() && !input.includes("\n")
    ? [...(tab.session.history ?? [])].reverse().find((h) => h.command.length > input.length && h.command.startsWith(input) && !h.command.includes("\n"))?.command.slice(input.length)
    : undefined;
  const current = search ? `${t(match ? "shell.reverseSearch" : "shell.reverseSearchFailed")} '` : confirming ? t("shell.confirmPrompt", { name: confirming.env?.name }) : prompt();
  const tabEnv = envs.find((env) => env.id === tab.envId);
  const connected = session.env ? tabEnv : undefined;
  // Disconnected and busy can only mean the health check is running: the first connection or a Retry.
  const connecting = retrying || (busy && !connected);
  const connectingLabel = retrying ? t("shell.retrying") : t("shell.connecting");
  const insert = (command: string) => { setInput(command); setHelp(false); requestAnimationFrame(() => { field.current?.focus(); field.current?.setSelectionRange(command.length, command.length); }); };

  // The output's contexts and toggle stay the same object while typing, so finished entries (memo) do not render again.
  // Their functions read the latest render's closures through this ref.
  const latest = useRef({ rerun, fold, follow, inspect, exportTable, tableFields, insert, tabEnv, popOut: (_table: Table, _footer?: string) => {} });
  latest.current = { rerun, fold, follow, inspect, exportTable, tableFields, insert, tabEnv, popOut: (table: Table, footer?: string) => {
    // Named after the command that printed it (the input line above the table), then where it ran.
    const at = tab.session.entries.findIndex((entry) => entry.kind === "table" && entry.table === table);
    const source = tab.session.entries.slice(0, at).reverse().find((entry) => entry.kind === "input");
    // "environment (database) - command"
    const where = `${tabEnv?.name ?? tab.envName}${tabEnv ? ` (${tabEnv.database})` : ""}`;
    const title = [where, source?.text ?? table.model].filter(Boolean).join(" - ");
    // With the environment and session context, the window can repeat the read to add a column, and nothing more.
    void ipc.openTableWindow({ table, footer, fields: table.model ? tableFields(table.model) : undefined, title, envId: tab.envId, context: tab.session.context }, title).catch(() => undefined);
  } };
  const aliasNames = useMemo(() => Object.keys(aliases), [aliases]);
  const sessionActions = useMemo(() => ({
    rerun: (command: string) => latest.current.rerun(command), fold: (id: number) => latest.current.fold(id), busy: busy || !!confirming, aliases: aliasNames,
  }), [busy, confirming, aliasNames]);
  // Field lists and the environment change what cells show: a new value then renders the tables again.
  const cellActions = useMemo((): NonNullable<ContextType<typeof CellActions>> => ({
    follow: (link, inOdoo) => void latest.current.follow(link, inOdoo), inspect: (title, value) => latest.current.inspect(title, value),
    exportTable: (table, format) => void latest.current.exportTable(table, format), fields: (model) => latest.current.tableFields(model),
    callCode: (call) => latest.current.tabEnv ? pythonCall(latest.current.tabEnv, call, tab.session.context) : null,
    insert: (command) => latest.current.insert(command),
    // The window gets the table with what describes it (field labels and types); it shares nothing else with the session.
    popOut: (table, footer) => latest.current.popOut(table, footer), aliases: aliasNames,
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }), [session.fields, tabEnv, aliasNames]);
  const toggle = useCallback((id: number) => update((s) => ({
    ...s, entries: s.entries.map((e) => e.id === id && e.kind === "table" ? { ...e, expanded: !e.expanded } : e),
    // eslint-disable-next-line react-hooks/exhaustive-deps
  })), []);

  return <div className="term" style={{ fontSize }} onKeyDown={onTermKey} onMouseUp={(event) => {
    if (!window.getSelection()?.toString() && !(event.target as HTMLElement).closest(".term-find, .term-scroll, .term-status, .term-help, .term-confirm")) field.current?.focus();
  }}>
    {find && <div className="term-find" role="search">
      <Icon name="search" />
      <input ref={findField} value={find.query} aria-label={t("shell.findLabel")} placeholder={t("shell.findLabel")} spellCheck={false}
        onChange={(e) => setFind({ query: e.target.value, index: 0 })}
        onKeyDown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); setFind({ ...find, index: findIndex + (e.shiftKey ? -1 : 1) }); }
          if (e.key === "Escape") { e.preventDefault(); closeFind(); }
        }} />
      <span className="term-find-count" role="status">{find.query ? (matches.length ? t("shell.findCount", { index: findIndex + 1, count: matches.length }) : t("shell.noMatches")) : ""}</span>
      <button type="button" className="icon-btn sm" aria-label={t("shell.previousMatch")} disabled={!matches.length} onClick={() => setFind({ ...find, index: findIndex - 1 })}><Icon name="caretUp" /></button>
      <button type="button" className="icon-btn sm" aria-label={t("shell.nextMatch")} disabled={!matches.length} onClick={() => setFind({ ...find, index: findIndex + 1 })}><Icon name="caretDown" /></button>
      <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={closeFind}><Icon name="x" /></button>
    </div>}
    <div className="term-out" ref={out} role="log" aria-live="polite" aria-label={t("shell.output")}>
      <SessionActions.Provider value={sessionActions}>
        <CellActions.Provider value={cellActions}>
          {groups(visible(session.entries)).map((group) => <div key={group[0].entry.id} className="term-group" data-outcome={outcomeOf(group.map(({ entry }) => entry))}>
            {group.map(({ entry, hidden }) => <Line key={entry.id} entry={entry} hidden={hidden} onToggle={toggle} />)}
          </div>)}
        </CellActions.Provider>
      </SessionActions.Provider>
      {/* Connecting is shown on the status chip; the output only tracks calls. */}
      {busy && !connecting && <Running since={started} />}
    </div>
    {help && <HelpPanel insert={insert} close={() => { setHelp(false); field.current?.focus(); }} />}
    {confirming && <WritePreview pending={confirming} answer={(yes) => void submit(yes ? "y" : "n")} />}
    {pasted && <div className="term-batch" role="group" aria-label={t("shell.pastedCommands", { count: pasted.length })} onMouseUp={(e) => e.stopPropagation()}>
      <span className="grow">{t("shell.pastedCommands", { count: pasted.length })}</span>
      <button type="button" className="btn sm" onClick={() => { setInput(pasted.join("\n")); setPasted(null); }}>{t("shell.pasteAsText")} <kbd>Esc</kbd></button>
      <button type="button" className="btn sm primary" onClick={runPasted}>{t("shell.runInOrder")} <kbd>Enter</kbd></button>
    </div>}
    {batch && <div className="term-batch" role="status" onMouseUp={(e) => e.stopPropagation()}>
      <Icon name="refresh" className="spin" /><span className="grow">{t("shell.batchProgress", { done: batch.total - batch.lines.length, total: batch.total })}</span>
      <button type="button" className="btn sm" onClick={() => setBatch(null)}>{t("shell.stop")}</button>
    </div>}
    {inspecting && <div className="term-inspect" role="dialog" aria-label={inspecting.title} onMouseUp={(e) => e.stopPropagation()}>
      <div className="term-inspect-head">
        <span className="term-inspect-title">{inspecting.title}</span>
        <CopyButton text={inspecting.text} className="term-copy" tip={t("common.copy")} ariaLabel={t("common.copy")} />
        <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={() => { setInspecting(null); field.current?.focus(); }}><Icon name="x" /></button>
      </div>
      <pre className="term-inspect-body">{inspecting.text}</pre>
    </div>}
    {menu && <ul className="term-menu" ref={menuList} role="listbox" aria-label={t("shell.suggestions")}>
      {menu.items.map((item, i) => <li key={`${item.kind}-${item.value}`} role="option" aria-selected={i === menu.index}
        onMouseDown={(e) => { e.preventDefault(); accept({ ...menu, index: i }); }}>
        <span className="term-menu-value">{item.value}</span>
        {item.detail && <span className="term-menu-detail">{item.translateDetail ? t(`shell.completions.${item.detail}` as "shell.completions.search") : item.detail}</span>}
      </li>)}
    </ul>}
    <label className={`term-input ${confirming ? "confirm" : ""} ${search ? "searching" : ""}`}>
      <span className="term-prompt">{current}</span>
      <span className="term-editor" style={search ? { flex: "none", width: `calc(${input.length}ch + 2px)` } : undefined}>
        {!confirming && <div className="term-highlight" ref={editorHighlight} aria-hidden="true">{search ? input : highlight(input, Object.keys(aliases))}{ghost && <span className="term-ghost">{ghost}</span>}</div>}
        <textarea ref={field} disabled={!tabEnv} value={input} rows={Math.min(8, input.split("\n").length)} wrap="off" spellCheck={false} autoComplete="off" autoCorrect="off" aria-invalid={hint ? true : undefined} aria-label={t("shell.command")} aria-description={search ? t("shell.reverseSearchDescription") : undefined}
          className={confirming ? "" : "highlighted"}
          onChange={(e) => { setInput(e.target.value); setCursor(null); if (search) setSearch({ ...search, query: e.target.value, skip: 0 }); else refilter(e.target.value); }}
          onScroll={syncScroll} onKeyUp={syncScroll} onSelect={syncScroll}
          onBlur={() => setMenu(null)} onKeyDown={onKey} onPaste={onPaste} />
      </span>
      {/* Reverse search keeps one line: the query, then what it finds. */}
      {search && <span className="term-search" role="status">{"': "}{shownMatch && <SearchMatch command={shownMatch.command} query={match ? search.query : ""} />}</span>}
      {/* Where the line goes and what it may do there; a broken connection offers its fix in place. */}
      {!search && !confirming && <span className="term-status">
        {!tabEnv ? <span className="chip chip-tone tone-danger">{t("shell.environmentGone")}</span> : <>
          {/* The database stays in view whatever the connection state. */}
          <span className="term-db">{tabEnv.database}</span>
          {connected ? <span className={`chip chip-tone term-mode tone-${modeTone[tabEnv.write_mode]}`}
            data-tip={[t(modeLabels[tabEnv.write_mode] as "mode.readOnly"), server && `${server.version ? `Odoo ${server.version} · ` : ""}${formatNumber(server.latency)} ms`].filter(Boolean).join("\n")}>
            <Icon name={modeIcons[tabEnv.write_mode]} />{t(modeShortLabels[tabEnv.write_mode] as "mode.readOnlyShort")}</span>
            // The state is its own fix: click it to connect again; the tooltip says what went wrong.
            : <button type="button" key={nudge} className={`chip chip-tone chip-action tone-${connecting ? "info" : failure ? "danger" : "neutral"}${nudge ? " nudge" : ""}`}
              disabled={connecting} aria-label={connecting ? connectingLabel : t("shell.retryConnection")} data-tip={!connecting && failure ? connectionProblem(failure) : undefined}
              onClick={() => { retryStarted.current = Date.now(); setRetrying(true); reconnect(); }}>
              <Icon name="refresh" className={connecting ? "spin" : undefined} />{connecting ? connectingLabel : failure ? t("shell.connectionFailed") : t("shell.notConnected")}
            </button>}
        </>}
      </span>}
      {hint && !menu && <span className="term-hint" role="status">{hint}</span>}
      {saveFailed && !hint && <span className="term-hint" role="status">{t("shell.sessionsNotSaved")}</span>}
    </label>
  </div>;
});

/** The command Ctrl+R found, first line only, with the query marked where it occurs. */
function SearchMatch({ command, query }: { command: string; query: string }) {
  const line = command.split("\n")[0];
  const at = query ? line.toLowerCase().indexOf(query.toLowerCase()) : -1;
  return <span className="term-search-match">{at < 0 ? line : <>{line.slice(0, at)}<mark>{line.slice(at, at + query.length)}</mark>{line.slice(at + query.length)}</>}{line !== command && " …"}</span>;
}

/** Entries after a folded command stay in the session but are not rendered. */
function visible(entries: Entry[]) {
  const shown: { entry: Entry; hidden?: number }[] = [];
  let folded: { hidden?: number } | null = null;
  for (const entry of entries) {
    if (entry.kind === "input") {
      const item = { entry, hidden: entry.folded ? 0 : undefined };
      folded = entry.folded ? item : null;
      shown.push(item);
    } else if (folded) folded.hidden = (folded.hidden ?? 0) + 1;
    else shown.push({ entry });
  }
  return shown;
}

/** A command and its output form one block, so hovering either shows the command's actions. */
function groups<T extends { entry: Entry }>(items: T[]): T[][] {
  const out: T[][] = [];
  for (const item of items) {
    if (item.entry.kind === "input" || !out.length) out.push([item]);
    else out[out.length - 1].push(item);
  }
  return out;
}

/** Past this many environments the menu gets a search box, as the tag filter does. */
const MENU_SEARCH_FROM = 8;

/** Environments for a new session; arrows move, Enter opens the first match, Escape and outside clicks close. */
function EnvironmentMenu({ envs, at, choose, close }: { envs: Environment[]; at: { left: number; top: number }; choose: (env: Environment) => void; close: () => void }) {
  const { t } = useTranslation();
  const searchable = envs.length > MENU_SEARCH_FROM;
  const [query, setQuery] = useState("");
  const q = query.trim().toLowerCase();
  const shown = envs.filter((env) => !q || [env.name, hostOf(env.url), env.database, ...env.tags].some((value) => value.toLowerCase().includes(q)));
  return <FloatingMenu at={at} label={t("shell.newSession")} className="more-menu-list floating shell-env-menu" close={close}
    onKey={(event) => {
      // Enter in the search box opens the first match.
      if (event.key === "Enter" && (event.target as Element).matches("input") && shown[0]) { event.preventDefault(); choose(shown[0]); }
    }}>
    {searchable && <label className="search shell-env-search"><Icon name="search" />
      <input type="search" value={query} placeholder={t("shell.findEnvironment")} aria-label={t("shell.findEnvironment")} spellCheck={false} onChange={(e) => setQuery(e.target.value)} /></label>}
    <div className="shell-env-options">
      {shown.map((env) => <button key={env.id} type="button" role="menuitem" onClick={() => choose(env)}>
        <EnvIcon env={env} /><span className="grow truncate">{env.name}</span><span className={`status-dot tone-${modeTone[env.write_mode]}`} data-tip={t(modeLabels[env.write_mode] as "mode.readOnly")} />
      </button>)}
      {!shown.length && <p className="hint shell-env-empty">{t("shell.noMatches")}</p>}
    </div>
  </FloatingMenu>;
}

/** `\?` as sections of commands; picking one types its first word, ready for arguments. Parsed from the translated
 * help so both views share one text: section titles start a line, commands are indented, the rest are notes. */
function helpSections(text: string) {
  const sections: { title: string; items: { usage: string; description: string }[] }[] = [];
  const notes: string[] = [];
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    if (/^\s/.test(line) && sections.length) {
      const [usage, ...rest] = line.trim().split(/\s{2,}/);
      sections[sections.length - 1].items.push({ usage, description: rest.join(" ") });
    } else if (!notes.length && !/[.:]\s/.test(line) && line.length < 40) sections.push({ title: line, items: [] });
    else notes.push(line);
  }
  return { sections, notes };
}

function HelpPanel({ insert, close }: { insert: (command: string) => void; close: () => void }) {
  const { t } = useTranslation();
  const { sections, notes } = helpSections(t("shell.help"));
  return <aside className="term-help" aria-label={t("shell.helpTitle")} onMouseUp={(e) => e.stopPropagation()}>
    <div className="term-inspect-head"><span className="term-inspect-title">{t("shell.helpTitle")}</span>
      <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={close}><Icon name="x" /></button></div>
    <div className="term-help-body">
      {sections.map((section) => <section key={section.title}><h4>{section.title}</h4>
        <ul>{section.items.map((item) => <li key={item.usage}>
          <button type="button" onClick={() => insert(`${item.usage.split(/\s/)[0]} `)}><code>{item.usage}</code>{item.description && <span>{item.description}</span>}</button>
        </li>)}</ul></section>)}
      {notes.map((note) => <p key={note} className="hint">{note}</p>)}
    </div>
  </aside>;
}

/** What a write will change, shown while the prompt asks to confirm it: the same view as Approvals. */
function WritePreview({ pending, answer }: { pending: Confirmation; answer: (yes: boolean) => void }) {
  const { t } = useTranslation();
  const { call, env } = pending;
  const ids = recordIds(call);
  const values = writtenValues(call);
  return <section className="term-confirm" aria-label={t("shell.confirmPrompt", { name: env?.name })} onMouseUp={(e) => e.stopPropagation()}>
    <header className="row gap-2">
      <code className={`method tone-${methodTone(call.method)}`}>{call.method}</code><strong className="mono truncate">{call.model}</strong>
      <span className="hint truncate">{env?.name}{ids ? ` · ${ids.join(", ")}` : ""}</span>
      <span className="row gap-2 ml-auto">
        <button type="button" className="btn sm" onClick={() => answer(false)}>{t("common.cancel")}</button>
        <button type="button" className={`btn sm ${call.method === "unlink" ? "danger" : "primary"}`} onClick={() => answer(true)}>{t("shell.send")}</button>
      </span>
    </header>
    {values?.length ? <ValuesTable values={values} /> : <CallDetails item={{ args: call.args, kwargs: call.kwargs, context: {} }} />}
  </section>;
}


const SessionActions = createContext<{ busy: boolean; rerun: (command: string) => void; fold: (id: number) => void; aliases: string[] } | null>(null);

const Line = memo(function Line({ entry, hidden, onToggle }: { entry: Entry; hidden?: number; onToggle: (id: number) => void }): ReactNode {
  const { t } = useTranslation();
  const actions = useContext(SessionActions);
  if (entry.kind === "input") return <div className="term-line term-cmd">{entry.agent && <span className="term-agent" data-tip={t("shell.ranBy", { name: entry.agent })}>{entry.agent}</span>}<span className="term-prompt">{entry.prompt}</span> {highlight(entry.text, actions?.aliases)}
    {hidden !== undefined && <button type="button" className="term-folded" onClick={() => actions?.fold(entry.id)}>{t("shell.linesHidden", { count: hidden })}</button>}
    {entry.command && actions && <span className="term-cmd-actions">
      <button type="button" className="icon-btn sm" aria-label={t("shell.rerun")} data-tip={t("shell.rerun")} disabled={actions.busy} onClick={() => actions.rerun(entry.text)}><Icon name="refresh" /></button>
      <CopyButton text={entry.text} className="icon-btn sm" tip={t("shell.copyCommand")} ariaLabel={t("shell.copyCommand")} iconOnly />
      <button type="button" className="icon-btn sm" aria-label={t(entry.folded ? "shell.unfold" : "shell.fold")} data-tip={t(entry.folded ? "shell.unfold" : "shell.fold")} aria-expanded={!entry.folded} onClick={() => actions.fold(entry.id)}><Icon name={entry.folded ? "chevron" : "caretDown"} /></button>
    </span>}
  </div>;
  // Sessions saved before connection errors moved to the status chip may still hold them as lines.
  if (entry.kind !== "table") return <div className={`term-line ${entry.kind}`}>{entry.connectionError
    ? connectionProblem(entry.connectionError)
    : entry.appError ? <ErrorText error={entry.appError} /> : entry.translationKey ? t(entry.translationKey) : entry.text}</div>;
  return <TableBlock table={entry.table} expanded={entry.expanded} footer={entry.footer} onToggle={() => onToggle(entry.id)} />;
});


/** Elapsed time of the call in progress. */
function Running({ since }: { since: number }) {
  const { t } = useTranslation();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const timer = setInterval(() => setNow(Date.now()), 100); return () => clearInterval(timer); }, []);
  const seconds = Math.max(0, now - since) / 1000;
  return <div className="term-line info term-running" role="status">
    <Icon name="refresh" className="spin" /> {formatNumber(seconds, { minimumFractionDigits: 1, maximumFractionDigits: 1 })} s · {t("shell.discardHint")}
  </div>;
}



/** Odoo's own message comes first; long messages fold their tail. */
function ErrorText({ error }: { error: AppError }) {
  const { t } = useTranslation();
  const title = formatAppError({ ...error, detail: undefined });
  const detail = error.detail?.trim();
  if (!detail) return <>{title}</>;
  const lines = detail.split("\n");
  const [first, ...rest] = lines;
  return <>
    <span className="term-error-title">{title}</span>{"\n"}{first}
    {rest.length > 0 && <details className="term-error-more">
      <summary>{t("shell.moreErrorLines", { count: rest.length })}</summary>
      {rest.join("\n")}
    </details>}
  </>;
}
