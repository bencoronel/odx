import { useState } from "react";
import type { Client, Environment, ServiceStatus } from "./ipc";
import { Icon } from "./Icon";
import PanelTitle, { useCollapsed } from "./PanelTitle";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { formatNumber } from "./i18n/format";
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
  return <div className="page-scroll"><div className="clients">
    <header className="settings-head"><h2>{t("mcpLocal.title")}</h2><p className="hint">{t("mcpLocal.description")}</p></header>
    <Summary local={local} services={services} />
    {/* Remount after each save so the form starts from the stored permissions. */}
    {local && <Permissions key={[local.all_environments, ...local.environment_ids, ...local.capabilities].join()} local={local} envs={envs} busy={busy}
      save={(ids, caps) => update(local.id, local.label, ids, caps)} />}
    {services && <Connect services={services} used={!!local?.last_used_at} />}
  </div></div>;
}

function Summary({ local, services }: { local?: Client; services: ServiceStatus | null }) {
  const { t } = useTranslation();
  const connections = services?.connections.filter((connection) => connection.client_id === local?.id).length ?? 0;
  const now = useNow(!!local?.last_used_at, 30_000);
  return <section className="panel" aria-label={t("mcpLocal.status")}>
    <dl className="mcp-summary">
      <div><dt>{t("mcpLocal.agents")}</dt><dd>{formatNumber(connections)}</dd></div>
      <div><dt>{t("mcpLocal.lastUse")}</dt><dd>{local?.last_used_at ? relativeTime(local.last_used_at, now) : t("mcpLocal.never")}</dd></div>
    </dl>
    {services?.mcp.state === "error" && <div className="mcp-summary-note"><p className="banner danger compact" role="alert"><Icon name="alert" />{formatAppError({ code: services.mcp.error ?? "unknown" })}</p></div>}
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
  return <form className="panel" aria-label={t("mcpLocal.permissions")} onSubmit={async (event) => {
    event.preventDefault();
    if (locked || !changed || !scopeValid(scope)) return;
    setSaving(true);
    setError("");
    try { await save(scope.allEnvs ? null : scope.selected, scope.caps); } catch (reason) { setError(normalizeAppError(reason)); } finally { setSaving(false); }
  }}>
    <header className="panel-header"><div><h3>{t("mcpLocal.permissions")}</h3><p className="hint">{t("mcpLocal.permissionsHint")}</p></div>
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
const targets = {
  claude: { label: "Claude Code", hint: "clients.claudeCodeHint", text: (s: ServiceStatus) => `claude mcp add --transport stdio --scope user odx -- ${mcpLaunchCommand(s)}` },
  // Codex starts MCP servers with a reduced environment; the connector needs XDG_RUNTIME_DIR to find the app socket.
  codex: { label: "Codex CLI", hint: "clients.codexCliHint", text: (s: ServiceStatus) => `codex mcp add odx --env XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" -- ${mcpLaunchCommand(s)}` },
  json: { label: "JSON", hint: "mcpLocal.jsonHint", text: (s: ServiceStatus) => JSON.stringify({ mcpServers: { odx: { command: s.connector_command, args: s.connector_args } } }, null, 2) },
} as const;

/** `used`: fold setup instructions once an agent has connected, unless the user chose otherwise. */
function Connect({ services, used }: { services: ServiceStatus; used: boolean }) {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useCollapsed("mcp-stdio-setup", used);
  const [target, setTarget] = useState<keyof typeof targets>("claude");
  return <section className="panel" aria-label={t("mcpLocal.connect")}>
    <header className="panel-header"><div><PanelTitle title={t("mcpLocal.connect")} collapsed={collapsed} toggle={() => setCollapsed(!collapsed)} /><p className="hint">{t("mcpLocal.connectHint")}</p></div></header>
    {!collapsed && <div className="panel-body stack-sm">
      <div className="segmented mcp-targets" role="radiogroup" aria-label={t("mcpLocal.client")}>
        {(Object.keys(targets) as (keyof typeof targets)[]).map((key) => <button key={key} type="button" role="radio" aria-checked={target === key} aria-pressed={target === key} onClick={() => setTarget(key)}>{targets[key].label}</button>)}
      </div>
      <p className="hint">{t(targets[target].hint)}</p>
      <Snippet text={targets[target].text(services)} />
    </div>}
  </section>;
}
