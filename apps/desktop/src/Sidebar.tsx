import { Icon, type IconName } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatNumber } from "./i18n/format";

export type Page = "environments" | "approvals" | "shell" | "mcp" | "activity" | "settings";
const pages = {
  environments: { title: "navigation.environments.title", description: "navigation.environments.description", icon: "server" },
  approvals: { title: "navigation.approvals.title", description: "navigation.approvals.description", icon: "check" },
  shell: { title: "navigation.shell.title", description: "navigation.shell.description", icon: "terminal" },
  activity: { title: "navigation.activity.title", description: "navigation.activity.description", icon: "activity" },
  mcp: { title: "navigation.mcp.title", description: "navigation.mcp.description", icon: "clients" },
  api: { title: "navigation.api.title", description: "navigation.serverPending", icon: "server" },
  settings: { title: "navigation.settings.title", description: "navigation.settings.description", icon: "settings" },
} as const satisfies Record<Page | "api", { title: string; description: string; icon: IconName }>;
export const PAGE_ORDER: Page[] = ["environments", "approvals", "shell", "mcp", "activity", "settings"];
const shortcut = (p: Page) => `Ctrl+${PAGE_ORDER.indexOf(p) + 1}`;
const sections: { label: "navigation.workspace" | "navigation.administration"; items: (Page | "api")[] }[] = [
  { label: "navigation.workspace", items: ["environments", "approvals", "shell"] },
  { label: "navigation.administration", items: ["mcp", "api", "activity", "settings"] },
];

export default function Sidebar({ page, setPage, counts, busy, lock }: {
  page: Page;
  setPage: (page: Page) => void;
  counts: { environments: number; approvals: number };
  busy: boolean;
  lock: () => void;
}) {
  const { t } = useTranslation();
  const count = (item: Page | "api") => item === "environments" ? <span className="count">{formatNumber(counts.environments)}</span>
    : item === "approvals" && counts.approvals > 0 ? <span className="count tone-warn">{formatNumber(counts.approvals)}</span> : null;
  return <aside className="sidebar">
    <nav aria-label={t("navigation.main")}>
      {sections.map((section) => [
        <div key={section.label} className="nav-label">{t(section.label)}</div>,
        ...section.items.map((item) => item === "mcp" ? <div key={item} role="group" aria-label={t("navigation.mcp.title")}>
          <div className="nav-group-title"><Icon name="clients" /><span>{t("navigation.mcp.title")}</span></div>
          <div className="nav-submenu">
            <button className="nav-item" aria-label={t("navigation.mcp.local")} data-tip={`MCP · ${t("navigation.mcp.local")}\n${shortcut("mcp")}`} aria-current={page === "mcp" ? "page" : undefined} onClick={() => setPage("mcp")}>
              <Icon name="terminal" /><span>{t("navigation.mcp.local")}</span>
            </button>
            <button className="nav-item" disabled aria-label={t("navigation.mcp.remote")} data-tip={`MCP · ${t("navigation.mcp.remote")}\n${t("navigation.serverPending")}`}>
              <Icon name="globe" /><span>{t("navigation.mcp.remote")}</span>
            </button>
          </div>
        </div> : <button key={item} className="nav-item" disabled={item === "api"} aria-label={t(pages[item].title)} data-tip={`${t(pages[item].title)}\n${item === "api" ? t("navigation.serverPending") : shortcut(item)}`} aria-current={page === item ? "page" : undefined} onClick={() => { if (item !== "api") setPage(item); }}>
          <Icon name={pages[item].icon} /><span>{t(pages[item].title)}</span>{count(item)}
        </button>),
      ])}
    </nav>
    {/* Shows the current state; hover previews the action. */}
    <button className="sidebar-lock" disabled={busy} onClick={lock} aria-label={t("navigation.lock")}
      data-tip={[t("navigation.lockHint"), ...(counts.approvals ? [t("navigation.lockRevokes", { count: counts.approvals })] : [])].join("\n")}>
      <span className="sidebar-lock-icon"><Icon name="unlock" /><Icon name="lock" /></span>
      <span className="sidebar-lock-text"><strong>{t("navigation.unlocked")}</strong>
        <small>{counts.approvals ? t("navigation.lockPending", { count: counts.approvals }) : t("navigation.lock")}</small></span>
    </button>
  </aside>;
}
