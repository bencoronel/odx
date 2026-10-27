import type { Service, ServiceStatus as Services } from "./ipc";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "./i18n/format";
import { formatAppError } from "./appError";
import { StatusDot, type Tone } from "./ui";

/** Vault state from `vault_status`. */
export type VaultState = "loading" | "absent" | "locked" | "unlocked" | string;

/** A service is active only while listening and unlocked. MCP keeps listening while locked but rejects requests. */
export function serviceState(service: Service, unlocked: boolean): { text: string; tone: Tone } {
  if (service.state === "error") return { text: "error", tone: "danger" };
  if (service.state === "stopped") return { text: "stopped", tone: "neutral" };
  return unlocked ? { text: "active", tone: "ok" } : { text: "lockedListening", tone: "neutral" };
}

const item = "inline-flex items-center gap-1.5 px-2 whitespace-nowrap [&_.icon]:size-3";
// A toned button keeps its color on hover.
const button = "h-5 rounded-sm cursor-pointer hover:bg-hover";

function ServiceItem({ label, service, unlocked, open, count, details = [] }: { label: string; service: Service; unlocked: boolean; open?: () => void; count?: number; details?: string[] }) {
  const { t } = useTranslation();
  const state = serviceState(service, unlocked);
  const text = state.text === "error" ? t("service.error") : state.text === "stopped" ? t("service.stopped") : state.text === "active" ? t("service.active") : t("service.lockedListening");
  const tip = [`${label} ${text}`, service.error ? formatAppError({ code: service.error }) : service.address ?? "",
    ...(unlocked ? details : [t("service.unlockRequired")])].filter(Boolean).join("\n");
  const content = <><StatusDot kind="small" tone={state.tone} />{label} {text}{count ? ` · ${t("service.connections", { count })}` : ""}</>;
  const tone = service.state === "error" ? "text-danger" : "";
  return open ? <button className={`${item} ${button} ${tone || "hover:text-fg"}`} onClick={open} data-tip={tip}>{content}</button>
    : <span className={`${item} h-full ${tone}`} data-tip={tip}>{content}</span>;
}

/** Keep the status bar mounted across navigation. */
export default function ServiceStatus({ vault, services, pending = 0, openMcp, openApprovals }: {
  vault: VaultState;
  services: Services | null;
  pending?: number;
  openMcp?: () => void;
  openApprovals?: () => void;
}) {
  const { t } = useTranslation();
  const unlocked = vault === "unlocked";
  return <footer className="h-6 flex-[0_0_24px] flex items-center gap-0.5 px-1.5 bg-chrome border-t border-line text-fg-3 text-xs select-none" aria-label={t("service.statusLabel")}>
    {unlocked && services && <ServiceItem label="MCP" service={services.mcp} unlocked={unlocked} open={openMcp}
      count={unlocked ? services.connections.length : undefined}
      details={services.connections.length ? services.connections.map((c) => t("service.connectionSince", { client: c.cwd ?? c.client_label ?? t("common.deletedClient"), time: formatDateTime(new Date(c.since), { hour: "2-digit", minute: "2-digit" }) })
        + (c.calls ? ` · ${t("mcpLocal.calls", { count: c.calls })}` : "")) : [t("service.noConnector")]} />}
    {unlocked
      ? <button className={`${item} ${button} ml-auto ${pending ? "text-warn" : "hover:text-fg"}`} onClick={openApprovals}><Icon name="check" />{pending ? t("service.pendingApprovals", { count: pending }) : t("service.noPendingApprovals")}</button>
      : <span className={`${item} h-full`}>{t("service.unlockToAccess")}</span>}
  </footer>;
}
