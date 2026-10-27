import { useState, type ReactNode } from "react";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";

/** Remember each section's collapsed state on this device; `fallback` applies until the user toggles it. */
export function useCollapsed(key: string, fallback = true): [boolean, (v: boolean) => void] {
  const storageKey = `odx.collapsed.${key}`;
  const [collapsed, set] = useState(() => {
    try { const stored = localStorage.getItem(storageKey); return stored === null ? fallback : stored !== "0"; } catch { return fallback; }
  });
  const update = (v: boolean) => {
    set(v);
    try { localStorage.setItem(storageKey, v ? "1" : "0"); } catch { /* Preferencia opcional. */ }
  };
  return [collapsed, update];
}

/** The collapse button's look, also used by the environment list's host groups; the chevron gets `chevron`. */
export const panelToggle = "group/toggle inline-flex items-center gap-1.5 -ml-1 px-1 py-0.5 border-0 rounded-sm bg-transparent text-inherit cursor-pointer hover:[&_h3]:text-fg [&_.icon]:size-3 [&_.icon]:text-fg-3 hover:[&_.icon]:text-fg";
export const chevron = "transition-transform duration-120 rotate-90 group-aria-[expanded=false]/toggle:rotate-0";

export default function PanelTitle({ title, collapsed, toggle, children }: { title: string; collapsed: boolean; toggle: () => void; children?: ReactNode }) {
  const { t } = useTranslation();
  return <span className="flex items-center gap-2">
    <button type="button" className={panelToggle} aria-expanded={!collapsed} aria-label={`${collapsed ? t("common.expand") : t("common.collapse")} ${title}`} onClick={toggle}>
      <Icon name="chevron" className={chevron} /><h3>{title}</h3>
    </button>
    {children}
  </span>;
}

