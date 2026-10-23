import { Icon, type IconName } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatNumber } from "./i18n/format";

export type Page = "environments" | "approvals" | "shell" | "mcp" | "activity" | "settings";
export const pages = {
  environments: { title: "navigation.environments.title", icon: "server" },
  approvals: { title: "navigation.approvals.title", icon: "check" },
  shell: { title: "navigation.shell.title", icon: "terminal" },
  activity: { title: "navigation.activity.title", icon: "activity" },
  mcp: { title: "navigation.mcp.title", icon: "clients" },
  api: { title: "navigation.api.title", icon: "server" },
  settings: { title: "navigation.settings.title", icon: "settings" },
} as const satisfies Record<Page | "api", { title: string; icon: IconName }>;
export const PAGE_ORDER: Page[] = ["environments", "approvals", "shell", "mcp", "activity", "settings"];
export const shortcut = (p: Page) => `Ctrl+${PAGE_ORDER.indexOf(p) + 1}`;
const sections: { label: "navigation.workspace" | "navigation.administration"; items: (Page | "api")[] }[] = [
  { label: "navigation.workspace", items: ["environments", "approvals", "shell"] },
  { label: "navigation.administration", items: ["mcp", "api", "activity", "settings"] },
];

export default function Sidebar({ page, setPage, counts, busy, lock, openPalette }: {
  page: Page;
  setPage: (page: Page) => void;
  counts: { environments: number; approvals: number };
  busy: boolean;
  lock: () => void;
  openPalette?: () => void;
}) {
  const { t } = useTranslation();
  const count = (item: Page | "api") => item === "environments" ? <span className="count">{formatNumber(counts.environments)}</span>
    : item === "approvals" && counts.approvals > 0 ? <span className="count tone-warn">{formatNumber(counts.approvals)}</span> : null;
  return <aside className="sidebar">
    <nav aria-label={t("navigation.main")}>
      {openPalette && <button className="nav-item sidebar-palette" aria-label={t("palette.title")} aria-keyshortcuts="Control+K" data-tip={t("palette.hint")} onClick={openPalette}>
        <Icon name="search" /><span>{t("palette.open")}</span><kbd aria-hidden="true">Ctrl K</kbd>
      </button>}
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
      data-tip={counts.approvals ? t("navigation.lockRevokes", { count: counts.approvals }) : undefined}>
      <span className="sidebar-lock-icon"><Icon name="unlock" /><Icon name="lock" /></span>
      <span className="sidebar-lock-text"><strong>{t("navigation.unlocked")}</strong>
        <small>{counts.approvals ? t("navigation.lockPending", { count: counts.approvals }) : t("navigation.lock")}</small></span>
    </button>
  </aside>;
}
