import { forgetHealth } from "./HealthChip";
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { ipc, type Client, type Environment, type Pending, type ServiceStatus } from "./ipc";
import { ApprovalDialog } from "@/components/ui/approval-dialog";
import SettingsPage, { type Theme } from "./SettingsPage";
import ApprovalsPage from "./ApprovalsPage";
import McpLocalPage from "./McpLocalPage";
import ActivityPage from "./ActivityPage";
import ShellPage from "./ShellPage";
import UnlockScreen from "./UnlockScreen";
import EnvironmentsPage from "./EnvironmentsPage";
import type { FormDraft } from "./EnvironmentForm";
import { CallDetails, methodTone } from "./ApprovalDetails";
import { PageLoadBoundary } from "./PageLoadBoundary";
import { WindowTitleBar } from "./WindowTitleBar";
import { TooltipLayer } from "./TooltipLayer";
import { Icon } from "./Icon";
import Sidebar, { PAGE_ORDER, type Page } from "./Sidebar";
import ServiceStatusBar from "./ServiceStatus";
import DesktopNotices from "./DesktopNotices";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { discardRestoredWindowState, hasWindowState, useWindowState, usePreventUnload, useWindowLifecycle } from "./windowState";

function initialPage(): Page {
  try {
    const stored = localStorage.getItem("odx.page") as Page | "clients" | null;
    if (stored === "clients") return "mcp";
    if (stored && PAGE_ORDER.includes(stored)) return stored;
  } catch { /* Preferences are optional when storage is unavailable. */ }
  return "environments";
}

function initialTheme(): Theme {
  try {
    const stored = localStorage.getItem("odx.theme");
    if (stored === "light" || stored === "dark") return stored;
  } catch { /* Preferences are optional when storage is unavailable. */ }
  return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

export default function App() {
  const { t } = useTranslation();
  const [theme, setTheme] = useState<Theme>(initialTheme);
  const [status, setStatus] = useState("loading");
  const [error, setError] = useState<ErrorState>("");
  const [envs, setEnvs] = useState<Environment[]>([]);
  const [page, setPage] = useState<Page>(initialPage);
  const [shellVisited, setShellVisited] = useState(() => initialPage() === "shell" || hasWindowState("shell.tabs"));

  useEffect(() => { if (page === "shell") setShellVisited(true); }, [page]);

  // Remember the last page on this device.
  useEffect(() => { try { localStorage.setItem("odx.page", page); } catch { /* Preferencia opcional. */ } }, [page]);

  // Ctrl+1…6 follows sidebar order.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.metaKey || event.shiftKey) return;
      const target = PAGE_ORDER[Number(event.key) - 1];
      if (!target) return;
      event.preventDefault();
      setPage(target);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const [pending, setPending] = useState<Pending[]>([]);
  const [clients, setClients] = useState<Client[]>([]);
  const [services, setServices] = useState<ServiceStatus | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [writableOnly, setWritableOnly] = useWindowState("app.writableOnly", false);
  const [approvalFilter, setApprovalFilter] = useWindowState("app.approvalFilter", "");
  // Environment to show when Environments opens from elsewhere (an approval).
  const [environmentFocus, setEnvironmentFocus] = useState<string | null>(null);
  useEffect(() => { if (page === "environments") setEnvironmentFocus(null); }, [page]);
  const [shellTarget, setShellTarget] = useState<{ env: string; command?: string } | null>(null);
  const [decision, setDecision] = useState<{ approvals: Pending[]; approve: boolean } | null>(null);
  const [batchReason, setBatchReason] = useState("");
  const batchReasonInput = useRef<HTMLInputElement>(null);
  const [notice, setNotice] = useState<{ text: string; activity: boolean } | null>(null);
  const [deciding, setDeciding] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [creatingEnvironment, setCreatingEnvironment] = useState(false);
  const environmentDraft = useRef<FormDraft | null>(null);
  // The draft (secret included) lives only while the form is open: cancel, save and lock drop it.
  useEffect(() => { if (!creatingEnvironment) environmentDraft.current = null; }, [creatingEnvironment]);
  const [modeDecision, setModeDecision] = useState<{ environment: Environment; mode: "temporary" | "permanent" } | null>(null);
  const [minutes, setMinutes] = useState("60");
  useWindowLifecycle((error) => setError(normalizeAppError(error)));
  usePreventUnload(status === "loading" || busy || deciding.size > 0 || creatingEnvironment || decision !== null || modeDecision !== null);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.style.colorScheme = theme;
    try { localStorage.setItem("odx.theme", theme); } catch { /* Preferencia opcional. */ }
  }, [theme]);

  async function refresh() {
    const next = await ipc.status();
    if (next === "unlocked") {
      const [environments, approvals, clientList, serviceStatus] = await Promise.all([ipc.environments(), ipc.approvals(), ipc.clients(), ipc.serviceStatus()]);
      setEnvs(environments);
      setPending(approvals);
      setClients(clientList);
      setServices(serviceStatus);
      setRefreshKey((k) => k + 1);
    } else {
      discardRestoredWindowState();
      setEnvs([]);
      setPending([]);
      setClients([]);
      setDecision(null);
      setCreatingEnvironment(false);
      setModeDecision(null);
      // MCP listens while locked and rejects requests until unlock.
      setServices(await ipc.serviceStatus().catch(() => null));
    }
    setStatus(next);
  }
  useEffect(() => { void refresh().catch((e) => setError(normalizeAppError(e))); }, []);

  // Receive new approval requests immediately.
  useEffect(() => {
    if (status !== "unlocked") return;
    const stop = listen("approvals-changed", () => { void ipc.approvals().then(setPending).catch(() => undefined); });
    return () => { void stop.then((unlisten) => unlisten()); };
  }, [status]);

  // Poll for requests while unlocked.
  useEffect(() => {
    if (status !== "unlocked") return;
    const timer = window.setInterval(() => {
      // Reflect locks triggered outside the window, including the tray.
      void ipc.status().then((vault) => vault === "unlocked" ? Promise.all([ipc.approvals(), ipc.serviceStatus()])
        .then(([approvals, serviceStatus]) => { setPending(approvals); setServices(serviceStatus); }) : refresh())
        .catch(() => { /* The next full refresh reports errors. */ });
    }, 2000);
    return () => window.clearInterval(timer);
  }, [status]);

  useEffect(() => {
    const deadlines = envs.filter((env) => env.write_mode === "temporary" && env.write_until)
      .map((env) => Date.parse(env.write_until!)).filter(Number.isFinite);
    if (!deadlines.length) return;
    const timer = window.setTimeout(() => { void refresh().catch((reason) => setError(normalizeAppError(reason))); },
      Math.min(2_147_483_647, Math.max(0, Math.min(...deadlines) - Date.now() + 50)));
    return () => window.clearTimeout(timer);
  }, [envs]);

  async function run(f?: () => Promise<unknown>): Promise<boolean> {
    setBusy(true);
    setError("");
    try {
      await f?.();
      await refresh();
      return true;
    } catch (e) {
      setError(normalizeAppError(e));
      return false;
    } finally {
      setBusy(false);
    }
  }

  /** Return the operation result and propagate its errors. Show refresh failures without losing the result. */
  async function mutate<T>(f: () => Promise<T>): Promise<T> {
    setBusy(true);
    setError("");
    try {
      const result = await f();
      await refresh().catch((reason) => setError(normalizeAppError(reason)));
      return result;
    } finally {
      setBusy(false);
    }
  }

  /** Lock only these requests while their decisions are sent; the rest of the app stays usable. */
  async function decideAll(approvals: Pending[], approve: boolean, reason?: string) {
    const ids = approvals.map((a) => a.id);
    setDeciding((current) => new Set([...current, ...ids]));
    setError("");
    try {
      const results = await Promise.allSettled(approvals.map((a) => approve ? ipc.decide(a.id, true) : ipc.decide(a.id, false, reason)));
      const failed = results.find((r): r is PromiseRejectedResult => r.status === "rejected");
      if (failed) setError(normalizeAppError(failed.reason));
      const done = results.filter((r) => r.status === "fulfilled").length;
      if (done) setNotice({ text: t(approve ? "approvals.approvedNotice" : "approvals.rejectedNotice", { count: done }), activity: approve });
      // One refresh for the whole batch, and unlock only after it, so decided requests never look pending again.
      await refresh().catch((reason) => setError(normalizeAppError(reason)));
    } finally {
      setDeciding((current) => new Set([...current].filter((id) => !ids.includes(id))));
    }
  }
  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(null), 5000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  const themeToggle = <button type="button" className="icon-btn" aria-label={theme === "dark" ? t("app.theme.useLight") : t("app.theme.useDark")} data-tip={theme === "dark" ? t("app.theme.light") : t("app.theme.dark")} onClick={() => setTheme(theme === "dark" ? "light" : "dark")}><Icon name={theme === "dark" ? "sun" : "moon"} /></button>;

  if (status !== "unlocked") {
    return <div className="app-window"><TooltipLayer /><WindowTitleBar>{themeToggle}</WindowTitleBar><DesktopNotices />
      <UnlockScreen status={status} busy={busy} error={error} retry={() => void run()} submit={(password) => run(() => status === "absent" ? ipc.create(password) : ipc.unlock(password))} reset={() => run(ipc.reset)} />
      <ServiceStatusBar vault={status} services={services} />
    </div>;
  }


  return <div className="app-window"><TooltipLayer /><WindowTitleBar>{themeToggle}</WindowTitleBar><DesktopNotices /><div className="shell">
    <Sidebar page={page} setPage={setPage} busy={busy} lock={() => void run(() => ipc.lock())}
      counts={{ environments: envs.length, approvals: pending.length }} />
    <main className="main" id="main-content" aria-busy={busy}>
      {error && <div className="banner danger page-banner" role="alert"><Icon name="alert" /><span className="grow">{formatAppError(error)}</span><button className="icon-btn sm" aria-label={t("common.dismissError")} onClick={() => setError("")}><Icon name="x" /></button></div>}
      <div className="shell-workspace" hidden={page !== "shell"}>
        {shellVisited && <PageLoadBoundary>
          <ShellPage envs={envs} visible={page === "shell"} connectTo={shellTarget?.env} command={shellTarget?.command} onConnectHandled={() => setShellTarget(null)} />
        </PageLoadBoundary>}
      </div>
      <PageLoadBoundary key={page}>
      {page === "settings" ? <SettingsPage theme={theme} setTheme={setTheme} busy={busy} pendingCount={pending.length}
        onContextSaved={refresh}
        onHeadersSaved={async () => { envs.forEach((e) => forgetHealth(e.id)); await refresh(); }}
        lock={() => void run(() => ipc.lock())}
        changePassword={async (current, next) => { setBusy(true); setError(""); try { await ipc.changePassword(current, next); } finally { setBusy(false); } }} />
        : page === "environments" ? <EnvironmentsPage
        envs={envs}
        pending={pending}
        focusId={environmentFocus}
        refresh={refresh}
        busy={busy}
        creating={creatingEnvironment}
        setCreating={setCreatingEnvironment}
        draftRef={environmentDraft}
        save={(draft, tags) => mutate(async () => {
          const created = await ipc.saveEnvironment(draft);
          setCreatingEnvironment(false);
          // The environment already exists; report a tag failure without reopening the form.
          if (tags.length) await ipc.setTags(created.id, tags).catch((reason) => setError(normalizeAppError(reason)));
          return created;
        })}
        update={(id, draft) => mutate(() => ipc.updateEnvironment(id, draft))}
        remove={(id) => run(() => ipc.deleteEnvironment(id, true))}
        clone={(id, name) => mutate(() => ipc.cloneEnvironment(id, name))}
        openShell={(env) => { setShellTarget({ env: env.name }); setPage("shell"); }}
        writableOnly={writableOnly}
        setWritableOnly={setWritableOnly}
        changeMode={(env, mode) => {
          if (mode === "temporary" || mode === "permanent") { setMinutes("60"); setModeDecision({ environment: env, mode }); }
          else void run(() => ipc.mode(env.id, mode));
        }}
      /> : page === "shell" ? null : page === "activity" ? <ActivityPage envs={envs} refreshKey={refreshKey} /> : page === "mcp" ? <McpLocalPage
        clients={clients}
        services={services}
        envs={envs}
        busy={busy}
        update={(id, label, ids, caps) => run(() => ipc.updateClient(id, label, ids, caps)).then((ok) => { if (!ok) throw { code: "app.client_update_failed" }; })}
      /> : <ApprovalsPage pending={pending} envs={envs} deciding={deciding}
        // Irreversible deletions still need an explicit confirmation.
        decide={(approval, approve, reason) => approve && approval.method === "unlink" ? setDecision({ approvals: [approval], approve }) : void decideAll([approval], approve, reason)}
        decideMany={(approvals, approve) => { setBatchReason(""); setDecision({ approvals, approve }); }}
        openInShell={(approval, command) => { setShellTarget({ env: approval.environment_name, command }); setPage("shell"); }}
        openEnvironment={(id) => { setEnvironmentFocus(id); setPage("environments"); }}
        filter={approvalFilter} setFilter={setApprovalFilter} />}
      </PageLoadBoundary>
    </main>
  </div>
    <ServiceStatusBar vault={status} services={services} pending={pending.length} openMcp={() => setPage("mcp")} openApprovals={() => setPage("approvals")} />
    <ApprovalDialog
      open={modeDecision !== null}
      busy={busy}
      tone={modeDecision?.mode === "permanent" ? "danger" : "warn"}
      title={modeDecision?.mode === "temporary" ? t("app.writeMode.temporaryTitle") : t("app.writeMode.permanentTitle")}
      description={modeDecision ? t(modeDecision.mode === "temporary" ? "app.writeMode.temporaryDescription" : "app.writeMode.permanentDescription", { name: modeDecision.environment.name }) : ""}
      confirmLabel={modeDecision?.mode === "temporary" ? t("app.writeMode.enableTemporary") : t("app.writeMode.enablePermanent")}
      confirmDisabled={modeDecision?.mode === "temporary" && (!Number.isSafeInteger(Number(minutes)) || Number(minutes) < 1 || Number(minutes) > 4_294_967_295)}
      onOpenChange={(open) => { if (!open && !busy) setModeDecision(null); }}
      onConfirm={async () => {
        if (!modeDecision) return;
        // mutate: a failed refresh after a successful change must not report the change as failed.
        await mutate(() => ipc.mode(modeDecision.environment.id, modeDecision.mode, modeDecision.mode === "temporary" ? Number(minutes) : undefined));
        setModeDecision(null);
      }}
    >
      {modeDecision && <dl className="props compact">
        <div><dt>{t("common.environment")}</dt><dd>{modeDecision.environment.name}</dd></div>
        <div><dt>{t("common.database")}</dt><dd className="mono">{modeDecision.environment.database}</dd></div>
        <div><dt>URL</dt><dd className="mono">{modeDecision.environment.url}</dd></div>
      </dl>}
      {modeDecision?.mode === "temporary" && <label className="field" htmlFor="write-minutes"><span>{t("app.writeMode.durationMinutes")}</span>
        <div className="row gap-2">
          <input id="write-minutes" className="w-28" type="number" min={1} max={4_294_967_295} step={1} value={minutes} disabled={busy} onChange={(event) => setMinutes(event.target.value)} />
          {[15, 60, 240].map((value) => <button key={value} type="button" className="btn sm" aria-pressed={minutes === String(value)} disabled={busy} onClick={() => setMinutes(String(value))}>{value < 60 ? `${value} min` : `${value / 60} h`}</button>)}
        </div>
        <small className="hint">{t("app.writeMode.durationHint")}</small>
      </label>}
      <p className="banner info compact"><Icon name="shield" />{t("app.writeMode.warning")}</p>
    </ApprovalDialog>
    <ApprovalDialog
      open={decision !== null}
      busy={false}
      tone={decision?.approve && !decision.approvals.some((a) => a.method === "unlink") ? "warn" : "danger"}
      title={t(decision?.approve === false ? "app.approval.confirmRejectTitle" : "app.approval.confirmSendTitle", { count: decision?.approvals.length ?? 1 })}
      description={t(decision?.approve === false ? "app.approval.confirmRejectDescription" : "app.approval.confirmSendDescription", { count: decision?.approvals.length ?? 1 })}
      confirmLabel={t(decision?.approve === false ? "app.approval.confirmReject" : "app.approval.confirmSend", { count: decision?.approvals.length ?? 1 })}
      destructive={decision?.approve === false || !!decision?.approvals.some((a) => a.method === "unlink")}
      onOpenChange={(open) => { if (!open) setDecision(null); }}
      // Close at once; each request shows its progress in Approvals.
      // Rejecting starts in the reason field so R, Enter rejects; approving keeps the safe default focus.
      initialFocus={decision?.approve === false ? batchReasonInput : undefined}
      onConfirm={async () => { if (decision) void decideAll(decision.approvals, decision.approve, decision.approve ? undefined : batchReason); }}
    >
      {decision?.approvals.length === 1 && <div className="call-box">
        <div className="row gap-2"><code className={`method tone-${methodTone(decision.approvals[0].method)}`}>{decision.approvals[0].method}</code><strong className="mono">{decision.approvals[0].model}</strong><span className="chip ml-auto"><Icon name="server" />{decision.approvals[0].environment_name}</span></div>
        <CallDetails item={decision.approvals[0]} />
      </div>}
      {decision && decision.approvals.length > 1 && <ul className="batch-list">
        {decision.approvals.map((a) => <li key={a.id}><code className={`method tone-${methodTone(a.method)}`}>{a.method}</code><span className="mono">{a.model}</span><span className="chip ml-auto"><Icon name="server" />{a.environment_name}</span></li>)}
      </ul>}
      {decision?.approve === false && <label className="field"><span>{t("approvals.reasonLabel")}</span>
        <input ref={batchReasonInput} value={batchReason} maxLength={500} placeholder={t("approvals.batchReasonPlaceholder")} onChange={(event) => setBatchReason(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); void decideAll(decision.approvals, false, batchReason); setDecision(null); } }} /></label>}
    </ApprovalDialog>
    {notice && <div className="toast" role="status"><Icon name="check" /><span>{notice.text}</span>
      {notice.activity && <button type="button" className="btn ghost sm" onClick={() => { setNotice(null); setPage("activity"); }}>{t("approvals.viewActivity")}</button>}
      <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={() => setNotice(null)}><Icon name="x" /></button>
    </div>}
  </div>;
}
