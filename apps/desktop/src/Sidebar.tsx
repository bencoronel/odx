import { Icon, type IconName } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatNumber } from "./i18n/format";
import { count as countPill, cx, navItem } from "./ui";

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
  // Collapsed sidebar: the count becomes a badge on the icon's corner.
  const badge = "max-md:absolute max-md:-top-0.5 max-md:-right-0.5 max-md:h-[15px] max-md:min-w-[15px] max-md:px-[3px] max-md:text-3xs";
  const count = (item: Page | "api") => item === "environments" ? <span className={cx(countPill(), badge)}>{formatNumber(counts.environments)}</span>
    : item === "approvals" && counts.approvals > 0 ? <span className={cx(countPill("warn"), badge)}>{formatNumber(counts.approvals)}</span> : null;
  return <aside className="w-[200px] flex-[0_0_200px] flex flex-col p-2 bg-chrome border-r border-line overflow-y-auto overscroll-y-contain max-md:w-[52px] max-md:basis-[52px] max-md:px-1.5">
    <nav className="grid gap-px select-none cursor-default" aria-label={t("navigation.main")}>
      {openPalette && <button className={navItem} aria-label={t("palette.title")} aria-keyshortcuts="Control+K" data-tip={t("palette.hint")} onClick={openPalette}>
        <Icon name="search" /><span className="flex-1 max-md:hidden">{t("palette.open")}</span><kbd className="ml-auto max-md:hidden" aria-hidden="true">Ctrl K</kbd>
      </button>}
      {sections.map((section) => [
        <div key={section.label} className="px-2 pt-3 pb-1 first:pt-1 text-2xs font-semibold tracking-[.06em] uppercase text-fg-3 max-md:h-0 max-md:p-0 max-md:mx-1 max-md:my-1.5 max-md:border-t max-md:border-line max-md:text-[0px] max-md:first:m-0 max-md:first:border-0">{t(section.label)}</div>,
        ...section.items.map((item) => item === "mcp" ? <div key={item} role="group" aria-label={t("navigation.mcp.title")}>
          <div className="flex items-center gap-[9px] h-7 px-2 text-fg-2 text-base max-md:justify-center max-md:p-0"><Icon name="clients" /><span className="max-md:hidden">{t("navigation.mcp.title")}</span></div>
          <div className="grid gap-px ml-[15px] pl-2 border-l border-line max-md:m-0 max-md:p-0 max-md:border-l-0">
            <button className={navItem} aria-label={t("navigation.mcp.local")} data-tip={`MCP · ${t("navigation.mcp.local")}\n${shortcut("mcp")}`} aria-current={page === "mcp" ? "page" : undefined} onClick={() => setPage("mcp")}>
              <Icon name="terminal" /><span className="flex-1 max-md:hidden">{t("navigation.mcp.local")}</span>
            </button>
            <button className={navItem} disabled aria-label={t("navigation.mcp.remote")} data-tip={`MCP · ${t("navigation.mcp.remote")}\n${t("navigation.serverPending")}`}>
              <Icon name="globe" /><span className="flex-1 max-md:hidden">{t("navigation.mcp.remote")}</span>
            </button>
          </div>
        </div> : <button key={item} className={navItem} disabled={item === "api"} aria-label={t(pages[item].title)} data-tip={`${t(pages[item].title)}\n${item === "api" ? t("navigation.serverPending") : shortcut(item)}`} aria-current={page === item ? "page" : undefined} onClick={() => { if (item !== "api") setPage(item); }}>
          <Icon name={pages[item].icon} /><span className="flex-1 max-md:hidden">{t(pages[item].title)}</span>{count(item)}
        </button>),
      ])}
    </nav>
    {/* Shows the current state; hover previews the action. */}
    <button className="group mt-auto flex items-center gap-[9px] w-full p-1.5 border border-line rounded-md bg-surface text-fg text-left cursor-pointer enabled:hover:border-warn/45 enabled:hover:bg-warn-soft disabled:opacity-55 disabled:cursor-default max-md:justify-center max-md:px-0 max-md:py-1 max-md:gap-0" disabled={busy} onClick={lock} aria-label={t("navigation.lock")}
      data-tip={counts.approvals ? t("navigation.lockRevokes", { count: counts.approvals }) : undefined}>
      <span className="grid place-items-center flex-[0_0_26px] h-[26px] rounded-sm bg-ok-soft text-ok group-enabled:group-hover:bg-warn-soft group-enabled:group-hover:text-warn">
        <Icon name="unlock" className="group-enabled:group-hover:hidden" /><Icon name="lock" className="hidden group-enabled:group-hover:block" /></span>
      <span className="grid min-w-0 leading-[1.3] max-md:hidden"><strong className="text-base font-medium">{t("navigation.unlocked")}</strong>
        <small className="truncate text-fg-3 text-xs">{counts.approvals ? t("navigation.lockPending", { count: counts.approvals }) : t("navigation.lock")}</small></span>
    </button>
  </aside>;
}
