import { forgetHealth } from "./HealthChip";
import { useEffect, useState } from "react";
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
  const [shellTarget, setShellTarget] = useState<string | null>(null);
  const [decision, setDecision] = useState<{ approval: Pending; approve: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [creatingEnvironment, setCreatingEnvironment] = useState(false);
  const [modeDecision, setModeDecision] = useState<{ environment: Environment; mode: "temporary" | "permanent" } | null>(null);
  const [minutes, setMinutes] = useState("60");
  useWindowLifecycle((error) => setError(normalizeAppError(error)));
  usePreventUnload(status === "loading" || busy || creatingEnvironment || decision !== null || modeDecision !== null);

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

  const themeToggle = <button type="button" className="icon-btn" aria-label={theme === "dark" ? t("app.theme.useLight") : t("app.theme.useDark")} data-tip={theme === "dark" ? t("app.theme.light") : t("app.theme.dark")} onClick={() => setTheme(theme === "dark" ? "light" : "dark")}><Icon name={theme === "dark" ? "sun" : "moon"} /></button>;

  if (status !== "unlocked") {
    return <div className="app-window"><TooltipLayer /><WindowTitleBar>{themeToggle}</WindowTitleBar><DesktopNotices />
      <UnlockScreen status={status} busy={busy} error={error} retry={() => void run()} submit={(password) => run(() => status === "absent" ? ipc.create(password) : ipc.unlock(password))} />
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
          <ShellPage envs={envs} visible={page === "shell"} connectTo={shellTarget} onConnectHandled={() => setShellTarget(null)} />
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
        refresh={refresh}
        busy={busy}
        creating={creatingEnvironment}
        setCreating={setCreatingEnvironment}
        save={(draft) => mutate(async () => {
          const created = await ipc.saveEnvironment(draft);
          setCreatingEnvironment(false);
          return created;
        })}
        update={(id, draft) => mutate(() => ipc.updateEnvironment(id, draft))}
        remove={(id) => run(() => ipc.deleteEnvironment(id, true))}
        clone={(id, name) => mutate(() => ipc.cloneEnvironment(id, name))}
        openShell={(env) => { setShellTarget(env.name); setPage("shell"); }}
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
      /> : <ApprovalsPage pending={pending} envs={envs} busy={busy} decide={(approval, approve) => setDecision({ approval, approve })} />}
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
      busy={busy}
      tone={decision?.approve ? (decision.approval.method === "unlink" ? "danger" : "warn") : "danger"}
      title={decision?.approve ? t("app.approval.confirmSendTitle") : t("app.approval.confirmRejectTitle")}
      description={decision?.approve ? t("app.approval.confirmSendDescription") : t("app.approval.confirmRejectDescription")}
      confirmLabel={decision?.approve ? t("app.approval.confirmSend") : t("app.approval.confirmReject")}
      destructive={decision?.approve === false || decision?.approval.method === "unlink"}
      onOpenChange={(open) => { if (!open && !busy) setDecision(null); }}
      onConfirm={async () => {
        if (!decision) return;
        await mutate(() => ipc.decide(decision.approval.id, decision.approve));
        setDecision(null);
      }}
    >
      {decision && <div className="call-box">
        <div className="row gap-2"><code className={`method tone-${methodTone(decision.approval.method)}`}>{decision.approval.method}</code><strong className="mono">{decision.approval.model}</strong><span className="chip ml-auto"><Icon name="server" />{decision.approval.environment_name}</span></div>
        <CallDetails item={decision.approval} />
      </div>}
    </ApprovalDialog>
  </div>;
}
