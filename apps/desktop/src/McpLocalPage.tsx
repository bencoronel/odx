import { useEffect, useState } from "react";
import { ipc, type Agent, type AgentSetup, type Client, type Environment, type ServiceStatus } from "./ipc";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { ScopeFields, Snippet, scopeChanged, scopeOf, scopeValid, type SaveClient } from "./clientParts";
import { relativeTime, useNow } from "./time";
import { usePreventUnload } from "./windowState";

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
  return <div className="page-scroll"><div className="clients">
    <header className="settings-head mcp-head"><h2>{t("mcpLocal.title")}</h2>{used && <Status local={local!} services={services} />}</header>
    {services?.mcp.state === "error" && <p className="banner danger compact" role="alert"><Icon name="alert" />{formatAppError({ code: services.mcp.error ?? "unknown" })}</p>}
    {!used && connect}
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
  return <p className="mcp-status" role="status" aria-label={t("mcpLocal.status")}>
    <span className={`status-dot tone-${connections ? "ok" : "neutral"}`} />
    {t("mcpLocal.connectedAgents", { count: connections })} · {relativeTime(local.last_used_at!, now)}
  </p>;
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
  return <form className="panel" aria-label={t("mcpLocal.permissions")} onSubmit={async (event) => {
    event.preventDefault();
    if (locked || !changed || !scopeValid(scope)) return;
    setSaving(true);
    setError("");
    try { await save(scope.allEnvs ? null : scope.selected, scope.caps); } catch (reason) { setError(normalizeAppError(reason)); } finally { setSaving(false); }
  }}>
    <header className="panel-header"><h3>{t("mcpLocal.permissions")}</h3>
      {changed && <span className="row gap-2">
        <button type="button" className="btn sm" disabled={locked} onClick={() => { setScope(scopeOf(local)); setError(""); }}>{t("common.discard")}</button>
        <button type="submit" className="btn sm primary" disabled={locked || !scopeValid(scope)}>{saving ? t("common.saving") : t("common.save")}</button>
      </span>}
    </header>
    <fieldset className="panel-body stack-sm" disabled={locked}>
      {envs.length === 0 && <p className="banner warn compact"><Icon name="alert" />{t("clients.noEnvironmentsYet")}</p>}
      <ScopeFields scope={scope} change={setScope} envs={envs} />
      {changed && <p className="hint">{t("mcpLocal.saveRevokes")}</p>}
      {error && <p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p>}
    </fieldset>
  </form>;
}

function mcpLaunchCommand(services: ServiceStatus) {
  const quote = (value: string) => /^[a-zA-Z0-9_@%+=:,./-]+$/.test(value) ? value : `'${value.replaceAll("'", "'\\''")}'`;
  return [services.connector_command, ...services.connector_args].map(quote).join(" ");
}
const agents: Agent[] = ["claude", "codex"];
const targets = {
  claude: { label: "Claude Code", text: (s: ServiceStatus) => `claude mcp add --transport stdio --scope user odx -- ${mcpLaunchCommand(s)}` },
  // Codex starts MCP servers with a reduced environment; the connector needs XDG_RUNTIME_DIR to find the app socket.
  codex: { label: "Codex CLI", text: (s: ServiceStatus) => `codex mcp add odx --env XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" -- ${mcpLaunchCommand(s)}` },
  json: { label: "JSON", text: (s: ServiceStatus) => JSON.stringify({ mcpServers: { odx: { command: s.connector_command, args: s.connector_args } } }, null, 2) },
} as const;

/** Always open: its place on the page (first until an agent connects, last afterwards) sets its weight. */
function Connect({ services }: { services: ServiceStatus }) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<keyof typeof targets>("claude");
  const [setup, setSetup] = useState<Partial<Record<Agent, AgentSetup>>>({});
  useEffect(() => {
    for (const agent of agents) ipc.agentSetup(agent).then((state) => setSetup((current) => ({ ...current, [agent]: state })), () => undefined);
  }, []);
  return <section className="panel" aria-labelledby="mcp-connect-title">
    <header className="panel-header"><h3 id="mcp-connect-title">{t("mcpLocal.connect")}</h3></header>
    <div className="panel-body stack-sm">
      <div className="segmented mcp-targets" role="radiogroup" aria-label={t("mcpLocal.client")}>
        {(Object.keys(targets) as (keyof typeof targets)[]).map((key) => <button key={key} type="button" role="radio" aria-checked={target === key} aria-pressed={target === key} onClick={() => setTarget(key)}>{targets[key].label}</button>)}
      </div>
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
  if (!state) return <p className="hint" role="status">{t("mcpLocal.checking")}</p>;
  if (state === "no_cli") return null;
  if (state === "added") return <p className="banner ok compact" role="status"><Icon name="check" />{t("mcpLocal.added", { agent: name })}</p>;
  const outdated = state === "outdated";
  // Same shape as the update notice: what was found, then the action.
  return <div className="stack-sm">
    <div className={`banner compact ${outdated ? "warn" : "info"}`} role="status">
      <Icon name={outdated ? "alert" : "plug"} /><span className="grow">{t(outdated ? "mcpLocal.outdated" : "mcpLocal.detected", { agent: name })}</span>
      <button type="button" className={`btn sm ${outdated ? "" : "primary"}`} disabled={busy} onClick={() => {
        setBusy(true);
        setError("");
        ipc.agentInstall(agent).then(done, (reason) => setError(normalizeAppError(reason))).finally(() => setBusy(false));
      }}><Icon name={outdated ? "refresh" : "plus"} />{busy ? t("common.processing") : t(outdated ? "mcpLocal.update" : "mcpLocal.add")}</button>
    </div>
    {error && <p className="banner danger compact" role="alert"><Icon name="alert" />{formatAppError(error)}</p>}
  </div>;
}
