import type { Service, ServiceStatus as Services } from "./ipc";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatDateTime } from "./i18n/format";
import { formatAppError } from "./appError";

/** Vault state from `vault_status`. */
export type VaultState = "loading" | "absent" | "locked" | "unlocked" | string;

/** A service is active only while listening and unlocked. MCP keeps listening while locked but rejects requests. */
export function serviceState(service: Service, unlocked: boolean) {
  if (service.state === "error") return { text: "error", tone: "danger" };
  if (service.state === "stopped") return { text: "stopped", tone: "neutral" };
  return unlocked ? { text: "active", tone: "ok" } : { text: "lockedListening", tone: "neutral" };
}

function ServiceItem({ label, service, unlocked, hint, open, count, details = [] }: { label: string; service: Service; unlocked: boolean; hint: string; open?: () => void; count?: number; details?: string[] }) {
  const { t } = useTranslation();
  const state = serviceState(service, unlocked);
  const text = state.text === "error" ? t("service.error") : state.text === "stopped" ? t("service.stopped") : state.text === "active" ? t("service.active") : t("service.lockedListening");
  const tip = [`${label} ${text}`, service.error ? formatAppError({ code: service.error }) : `${hint}${service.address ? ` · ${service.address}` : ""}`,
    ...(unlocked ? details : [t("service.unlockRequired")]), ...(open ? [t("service.connectHint")] : [])].join("\n");
  const content = <><span className={`status-dot tone-${state.tone}`} />{label} {text}{count ? ` · ${t("service.connections", { count })}` : ""}</>;
  const className = `status-item ${service.state === "error" ? "text-danger" : ""}`;
  return open ? <button className={className} onClick={open} data-tip={tip}>{content}</button>
    : <span className={className} data-tip={tip}>{content}</span>;
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
  return <footer className="statusbar" aria-label={t("service.statusLabel")}>
    {unlocked && services && <ServiceItem label="MCP" service={services.mcp} unlocked={unlocked} hint={t("service.stdioHint")} open={openMcp}
      count={unlocked ? services.connections.length : undefined}
      details={services.connections.length ? services.connections.map((c) => t("service.connectionSince", { client: c.client_label ?? t("common.deletedClient"), time: formatDateTime(new Date(c.since), { hour: "2-digit", minute: "2-digit" }) })) : [t("service.noConnector")]} />}
    {unlocked
      ? <button className={`status-item ml-auto ${pending ? "text-warn" : ""}`} onClick={openApprovals}><Icon name="check" />{pending ? t("service.pendingApprovals", { count: pending }) : t("service.noPendingApprovals")}</button>
      : <span className="status-item">{t("service.unlockToAccess")}</span>}
  </footer>;
}
