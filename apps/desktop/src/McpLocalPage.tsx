import { useEffect, useState } from "react";
import { ipc, type Agent, type AgentSetup, type Client, type Environment, type McpConnection, type ServiceStatus } from "./ipc";
import { formatDateTime } from "./i18n/format";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { ScopeFields, Snippet, scopeChanged, scopeOf, scopeValid, type SaveClient } from "./clientParts";
import { relativeTime, useNow } from "./time";
import { usePreventUnload } from "./windowState";
import { Banner, Button, PanelHeader, Segment, Segmented, StatusDot, banner, cx, panel, panelBody, settingRow } from "./ui";

export default function McpLocalPage({ clients, services, envs, busy, update }: {
  clients: Client[];
  services: ServiceStatus | null;
  envs: Environment[];
  busy: boolean;
  update: (id: string, ...client: Parameters<SaveClient>) => Promise<void>;
}) {
  const { t } = useTranslation();
  const local = clients.find((client) => client.local_stdio);
  // Until an agent has connected, connecting is the task; afterwards it is the permissions.
  const used = !!local?.last_used_at;
  const connect = services && <Connect services={services} />;
  return <div className="page-scroll flex-1 min-h-0 overflow-auto overscroll-contain px-5 pt-4 pb-6"><div className="grid gap-3 max-w-[980px]">
    <header className="grid gap-[3px] mb-1"><h2 className="text-fg">{t("mcpLocal.title")}</h2>{used && <Status local={local!} services={services} />}</header>
    {services?.mcp.state === "error" && <Banner tone="danger" compact role="alert"><Icon name="alert" />{formatAppError({ code: services.mcp.error ?? "unknown" })}</Banner>}
    {!used && connect}
    {local && <Connections connections={services?.connections.filter((connection) => connection.client_id === local.id) ?? []} />}
    {/* Remount after each save so the form starts from the stored permissions. */}
    {local && <Permissions key={[local.all_environments, ...local.environment_ids, ...local.capabilities].join()} local={local} envs={envs} busy={busy}
      save={(ids, caps) => update(local.id, local.label, ids, caps)} />}
    {used && connect}
  </div></div>;
}

function Status({ local, services }: { local: Client; services: ServiceStatus | null }) {
  const { t } = useTranslation();
  const connections = services?.connections.filter((connection) => connection.client_id === local.id).length ?? 0;
  const now = useNow(true, 30_000);
  return <p className="inline-flex items-center gap-1.5 text-fg-2 text-sm" role="status" aria-label={t("mcpLocal.status")}>
    <StatusDot tone={connections ? "ok" : "neutral"} />
    {t("mcpLocal.connectedAgents", { count: connections })} · {relativeTime(local.last_used_at!, now)}
  </p>;
}

/** One row per open connector, so stale agent sessions can be told apart and closed. */
function Connections({ connections }: { connections: McpConnection[] }) {
  const { t } = useTranslation();
  const [closing, setClosing] = useState<number[]>([]);
  const [error, setError] = useState<ErrorState>("");
  if (!connections.length) return null;
  return <section className={panel} aria-labelledby="mcp-connections-title">
    <PanelHeader><h3 id="mcp-connections-title">{t("mcpLocal.connections")}</h3></PanelHeader>
    {connections.map((connection) => <div key={connection.id} className={settingRow}>
      <div>
        <strong className="truncate" title={connection.cwd ?? undefined}>{connection.cwd ?? t("mcpLocal.unknownAgent")}</strong>
        <span className="mt-px text-fg-3 text-sm truncate" title={connection.command ?? undefined}>{[connection.command, connection.pid != null && `PID ${connection.pid}`,
          t("mcpLocal.calls", { count: connection.calls }), t("mcpLocal.since", { time: formatDateTime(new Date(connection.since), { hour: "2-digit", minute: "2-digit" }) })].filter(Boolean).join(" · ")}</span>
      </div>
      <Button size="sm" disabled={closing.includes(connection.id)} data-tip={t("mcpLocal.disconnectHint")} onClick={() => {
        setClosing((ids) => [...ids, connection.id]);
        setError("");
        ipc.mcpDisconnect(connection.id).catch((reason) => {
          setError(normalizeAppError(reason));
          setClosing((ids) => ids.filter((id) => id !== connection.id));
        });
      }}>{closing.includes(connection.id) ? t("mcpLocal.disconnecting") : t("mcpLocal.disconnect")}</Button>
    </div>)}
    {error && <div className={panelBody}><Banner tone="danger" compact role="alert"><Icon name="alert" />{formatAppError(error)}</Banner></div>}
  </section>;
}

function Permissions({ local, envs, busy, save }: {
  local: Client;
  envs: Environment[];
  busy: boolean;
  save: (environmentIds: string[] | null, capabilities: Client["capabilities"]) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [scope, setScope] = useState(() => scopeOf(local));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<ErrorState>("");
  const changed = scopeChanged(scope, local);
  const locked = busy || saving;
  usePreventUnload(changed || locked);
  return <form className={panel} aria-label={t("mcpLocal.permissions")} onSubmit={async (event) => {
    event.preventDefault();
    if (locked || !changed || !scopeValid(scope)) return;
    setSaving(true);
    setError("");
    try { await save(scope.allEnvs ? null : scope.selected, scope.caps); } catch (reason) { setError(normalizeAppError(reason)); } finally { setSaving(false); }
  }}>
    <PanelHeader><h3>{t("mcpLocal.permissions")}</h3>
      {changed && <span className="flex flex-none items-center gap-2">
        <Button size="sm" disabled={locked} onClick={() => { setScope(scopeOf(local)); setError(""); }}>{t("common.discard")}</Button>
        <Button type="submit" size="sm" variant="primary" disabled={locked || !scopeValid(scope)}>{saving ? t("common.saving") : t("common.save")}</Button>
      </span>}
    </PanelHeader>
    <fieldset className={cx(panelBody, "grid gap-2.5")} disabled={locked}>
      {envs.length === 0 && <Banner tone="warn" compact><Icon name="alert" />{t("clients.noEnvironmentsYet")}</Banner>}
      <ScopeFields scope={scope} change={setScope} envs={envs} />
      {changed && <p className="text-fg-3 text-sm">{t("mcpLocal.saveRevokes")}</p>}
      {error && <Banner tone="danger" role="alert"><Icon name="alert" />{formatAppError(error)}</Banner>}
    </fieldset>
  </form>;
}

function mcpLaunchCommand(services: ServiceStatus) {
  const quote = (value: string) => /^[a-zA-Z0-9_@%+=:,./-]+$/.test(value) ? value : `'${value.replaceAll("'", "'\\''")}'`;
  return [services.connector_command, ...services.connector_args].map(quote).join(" ");
}
const agents: Agent[] = ["claude", "codex"];
const targets = {
  claude: { label: "Claude Code", text: (s: ServiceStatus) => `claude mcp add --transport stdio --scope user ${s.mcp_name} -- ${mcpLaunchCommand(s)}` },
  // Codex starts MCP servers with a reduced environment; the connector needs XDG_RUNTIME_DIR to find the app socket.
  codex: { label: "Codex CLI", text: (s: ServiceStatus) => `codex mcp add ${s.mcp_name} --env XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" -- ${mcpLaunchCommand(s)}` },
  json: { label: "JSON", text: (s: ServiceStatus) => JSON.stringify({ mcpServers: { [s.mcp_name]: { command: s.connector_command, args: s.connector_args } } }, null, 2) },
} as const;

/** Always open: its place on the page (first until an agent connects, last afterwards) sets its weight. */
function Connect({ services }: { services: ServiceStatus }) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<keyof typeof targets>("claude");
  const [setup, setSetup] = useState<Partial<Record<Agent, AgentSetup>>>({});
  useEffect(() => {
    for (const agent of agents) ipc.agentSetup(agent).then((state) => setSetup((current) => ({ ...current, [agent]: state })), () => undefined);
  }, []);
  return <section className={panel} aria-labelledby="mcp-connect-title">
    <PanelHeader><h3 id="mcp-connect-title">{t("mcpLocal.connect")}</h3></PanelHeader>
    <div className={cx(panelBody, "grid gap-2.5")}>
      <Segmented className="justify-self-start" role="radiogroup" aria-label={t("mcpLocal.client")}>
        {(Object.keys(targets) as (keyof typeof targets)[]).map((key) => <Segment key={key} role="radio" aria-checked={target === key} aria-pressed={target === key} onClick={() => setTarget(key)}>{targets[key].label}</Segment>)}
      </Segmented>
      {target !== "json" && <Install agent={target} name={targets[target].label} state={setup[target]} done={(state) => setSetup((current) => ({ ...current, [target]: state }))} />}
      <Snippet text={targets[target].text(services)} />
    </div>
  </section>;
}

/** Runs the agent's own `mcp add` for the user; the command below stays visible for those who prefer to run it. */
function Install({ agent, name, state, done }: { agent: Agent; name: string; state?: AgentSetup; done: (state: AgentSetup) => void }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorState>("");
  if (!state) return <p className="text-fg-3 text-sm" role="status">{t("mcpLocal.checking")}</p>;
  if (state === "no_cli") return null;
  if (state === "added") return <Banner tone="ok" compact role="status"><Icon name="check" />{t("mcpLocal.added", { agent: name })}</Banner>;
  const outdated = state === "outdated";
  // Same shape as the update notice: what was found, then the action.
  return <div className="grid gap-2.5">
    <div className={banner({ tone: outdated ? "warn" : "info", compact: true })} role="status">
      <Icon name={outdated ? "alert" : "plug"} /><span className="grow">{t(outdated ? "mcpLocal.outdated" : "mcpLocal.detected", { agent: name })}</span>
      <Button size="sm" variant={outdated ? "default" : "primary"} disabled={busy} onClick={() => {
        setBusy(true);
        setError("");
        ipc.agentInstall(agent).then(done, (reason) => setError(normalizeAppError(reason))).finally(() => setBusy(false));
      }}><Icon name={outdated ? "refresh" : "plus"} />{busy ? t("common.processing") : t(outdated ? "mcpLocal.update" : "mcpLocal.add")}</Button>
    </div>
    {error && <Banner tone="danger" compact role="alert"><Icon name="alert" />{formatAppError(error)}</Banner>}
  </div>;
}
