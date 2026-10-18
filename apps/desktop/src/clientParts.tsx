import type { Capability, Client, Environment } from "./ipc";
import { EnvIcon } from "./EnvIcon";
import { Icon } from "./Icon";
import { modeLabels, modeTone } from "./ModeSelect";
import CopyButton from "./CopyButton";
import { useTranslation } from "react-i18next";

const capabilityInfo = {
  "odoo:read": { label: "clients.capabilities.read", help: "clients.capabilities.readHelp" },
  "odoo:write-request": { label: "clients.capabilities.write", help: "clients.capabilities.writeHelp" },
} as const satisfies Record<Capability, { label: string; help: string }>;
const capabilities = Object.keys(capabilityInfo) as Capability[];

export type SaveClient = (label: string, environmentIds: string[] | null, capabilities: Capability[]) => Promise<void>;
/** Environments and capabilities a client may use; `selected` is ignored while `allEnvs` is set. */
export type Scope = { allEnvs: boolean; selected: string[]; caps: Capability[] };

export const scopeOf = (client?: Client): Scope => ({ allEnvs: client?.all_environments ?? true, selected: client?.environment_ids ?? [], caps: client?.capabilities ?? ["odoo:read"] });
const sameSet = <T,>(a: T[], b: T[]) => a.length === b.length && a.every((value) => b.includes(value));
export const scopeChanged = (scope: Scope, client: Client) => scope.allEnvs !== client.all_environments
  || (!scope.allEnvs && !sameSet(scope.selected, client.environment_ids)) || !sameSet(scope.caps, client.capabilities);
export const scopeValid = (scope: Scope) => (scope.allEnvs || scope.selected.length > 0) && scope.caps.length > 0;

export function Snippet({ text }: { text: string }) {
  return <div className="snippet">
    <pre>{text}</pre>
    <CopyButton text={text} />
  </div>;
}

export function ScopeFields({ scope, change, envs }: { scope: Scope; change: (scope: Scope) => void; envs: Environment[] }) {
  const { t } = useTranslation();
  const toggle = <T,>(list: T[], value: T) => list.includes(value) ? list.filter((v) => v !== value) : [...list, value];
  const { allEnvs, selected, caps } = scope;
  return <>
    <div className="field">
      <span>{t("clients.environments")}</span>
      <div className="segmented scope" role="radiogroup" aria-label={t("clients.environmentScope")}>
        <button type="button" role="radio" aria-checked={allEnvs} aria-pressed={allEnvs} onClick={() => change({ ...scope, allEnvs: true })}><Icon name="globe" />{t("clients.allEnvironments")}</button>
        <button type="button" role="radio" aria-checked={!allEnvs} aria-pressed={!allEnvs} onClick={() => change({ ...scope, allEnvs: false })}><Icon name="server" />{t("clients.selectedEnvironments")}</button>
      </div>
      {allEnvs
        ? <small className="hint">{t("clients.allEnvironmentsLongHint")}</small>
        : <><div className="option-grid">{envs.map((env) => <label key={env.id} className="option">
        <input type="checkbox" checked={selected.includes(env.id)} onChange={() => change({ ...scope, selected: toggle(selected, env.id) })} />
        <span className="env-avatar sm"><EnvIcon env={env} /><span className={`status-dot tone-${modeTone[env.write_mode]}`} /></span>
        <span className="option-text"><strong>{env.name}</strong><small>{t(modeLabels[env.write_mode] as "mode.readOnly")}</small></span>
      </label>)}</div><small className="hint">{t("clients.selectedEnvironmentsHint")}</small></>}
    </div>
    <div className="field">
      <span>{t("clients.capabilitiesTitle")}</span>
      <div className="option-grid">{capabilities.map((cap) => <label key={cap} className="option">
        <input type="checkbox" checked={caps.includes(cap)} onChange={() => change({ ...scope, caps: toggle(caps, cap) })} />
        <span className="option-text"><strong>{t(capabilityInfo[cap].label as "clients.capabilities.read")} <code className="hint">{cap}</code></strong><small>{t(capabilityInfo[cap].help as "clients.capabilities.readHelp")}</small></span>
      </label>)}</div>
    </div>
  </>;
}
