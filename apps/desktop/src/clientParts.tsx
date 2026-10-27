import type { Capability, Client, Environment } from "./ipc";
import { EnvIcon } from "./EnvIcon";
import { Icon } from "./Icon";
import { modeLabels, modeTone } from "./ModeSelect";
import CopyButton from "./CopyButton";
import { useTranslation } from "react-i18next";
import { groupByHost } from "./hosts";
import { Banner, Button, Chip, Segment, Segmented, StatusDot, button, cx, envAvatar, field } from "./ui";

/** Access levels shown to the user; write requests always come with reads. */
const levels = { read: ["odoo:read"], write: ["odoo:read", "odoo:write-request"] } as const satisfies Record<string, Capability[]>;
/** Write modes in which a client that may request writes creates and updates without approval. */
export const unattended = (env: Environment) => env.write_mode === "temporary" || env.write_mode === "permanent";
/** Offer "All / None" only when ticking one by one gets tedious. */
const BULK_FROM = 8;

export type SaveClient = (label: string, environmentIds: string[] | null, capabilities: Capability[]) => Promise<void>;
/** Environments and capabilities a client may use; `selected` is ignored while `allEnvs` is set. */
export type Scope = { allEnvs: boolean; selected: string[]; caps: Capability[] };

export const scopeOf = (client?: Client): Scope => ({ allEnvs: client?.all_environments ?? true, selected: client?.environment_ids ?? [], caps: client?.capabilities ?? ["odoo:read"] });
const sameSet = <T,>(a: T[], b: T[]) => a.length === b.length && a.every((value) => b.includes(value));
export const scopeChanged = (scope: Scope, client: Client) => scope.allEnvs !== client.all_environments
  || (!scope.allEnvs && !sameSet(scope.selected, client.environment_ids)) || !sameSet(scope.caps, client.capabilities);
export const scopeValid = (scope: Scope) => (scope.allEnvs || scope.selected.length > 0) && scope.caps.length > 0;

export function Snippet({ text }: { text: string }) {
  return <div className="relative border border-line rounded-md bg-surface-2 [&>.copy-error]:px-3 [&>.copy-error]:pb-2">
    <pre className="m-0 px-3 py-2.5 pr-[92px] text-xs leading-[1.55] whitespace-pre-wrap wrap-anywhere text-fg max-h-[220px] overflow-auto">{text}</pre>
    <CopyButton text={text} className={cx(button({ size: "sm" }), "absolute top-1.5 right-1.5")} />
  </div>;
}

export function ScopeFields({ scope, change, envs }: { scope: Scope; change: (scope: Scope) => void; envs: Environment[] }) {
  const { t } = useTranslation();
  const { allEnvs, selected, caps } = scope;
  const writes = caps.includes("odoo:write-request");
  // What a client may really do depends on both its level and each environment's mode.
  const risky = writes ? (allEnvs ? envs : envs.filter((env) => selected.includes(env.id))).filter(unattended) : [];
  const option = (env: Environment) => <label key={env.id} className="flex items-center gap-2 px-2 py-[7px] border border-line rounded-md bg-surface-2 cursor-pointer hover:border-line-strong has-[input:checked]:border-accent has-[input:checked]:bg-accent-soft">
    <input type="checkbox" checked={selected.includes(env.id)} onChange={() => change({ ...scope, selected: selected.includes(env.id) ? selected.filter((id) => id !== env.id) : [...selected, env.id] })} />
    <span className={envAvatar(true)}><EnvIcon env={env} /><StatusDot kind="badge" tone={modeTone[env.write_mode]} /></span>
    <span className="grid min-w-0"><strong className="text-base font-medium">{env.name}</strong><small className="text-fg-3 text-xs">{t(modeLabels[env.write_mode] as "mode.readOnly")}</small></span>
    {writes && unattended(env) && <Chip tone="warn" className="ml-auto">{t("clients.unattendedShort")}</Chip>}
  </label>;
  return <>
    <div className={field}>
      <span>{t("clients.environments")}</span>
      <Segmented className="justify-self-start" role="radiogroup" aria-label={t("clients.environmentScope")}>
        <Segment role="radio" aria-checked={allEnvs} aria-pressed={allEnvs} onClick={() => change({ ...scope, allEnvs: true })}><Icon name="globe" />{t("clients.allEnvironments")}</Segment>
        <Segment role="radio" aria-checked={!allEnvs} aria-pressed={!allEnvs} onClick={() => change({ ...scope, allEnvs: false })}><Icon name="server" />{t("clients.selectedEnvironments")}</Segment>
      </Segmented>
      {!allEnvs && envs.length > BULK_FROM && <span className="flex items-center gap-2">
        <Button variant="ghost" size="sm" onClick={() => change({ ...scope, selected: envs.map((env) => env.id) })}>{t("clients.selectAll")}</Button>
        <Button variant="ghost" size="sm" onClick={() => change({ ...scope, selected: [] })}>{t("clients.selectNone")}</Button>
      </span>}
      {/* Databases of one server sit under its host, as in Environments. */}
      {!allEnvs && <div className="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-1.5">{groupByHost(envs).map(([host, list]) => list.length === 1 ? option(list[0])
        : [<small key={host} className="mono col-span-full px-0.5 pt-1 text-fg-3 text-xs">{host}</small>, ...list.map(option)])}</div>}
    </div>
    <div className={field}>
      <span>{t("clients.access")}</span>
      <Segmented className="justify-self-start" role="radiogroup" aria-label={t("clients.access")}>
        {(Object.keys(levels) as (keyof typeof levels)[]).map((level) => {
          const on = (level === "write") === writes;
          return <Segment key={level} role="radio" aria-checked={on} aria-pressed={on} onClick={() => change({ ...scope, caps: [...levels[level]] })}>
            <Icon name={level === "write" ? "pencil" : "eye"} />{t(level === "write" ? "clients.levels.write" : "clients.levels.read")}</Segment>;
        })}
      </Segmented>
    </div>
    {risky.length > 0 && <Banner tone="warn" compact role="status"><Icon name="alert" />{t("clients.unattended", { names: risky.map((env) => env.name).join(", ") })}</Banner>}
  </>;
}
