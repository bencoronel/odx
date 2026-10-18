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

export default function PanelTitle({ title, collapsed, toggle, children }: { title: string; collapsed: boolean; toggle: () => void; children?: ReactNode }) {
  const { t } = useTranslation();
  return <span className="row gap-2">
    <button type="button" className="panel-toggle" aria-expanded={!collapsed} aria-label={`${collapsed ? t("common.expand") : t("common.collapse")} ${title}`} onClick={toggle}>
      <Icon name="chevron" /><h3>{title}</h3>
    </button>
    {children}
  </span>;
}

