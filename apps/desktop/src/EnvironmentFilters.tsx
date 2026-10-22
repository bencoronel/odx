import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import type { Environment, WriteMode } from "./ipc";
import { modeIcons, modeLabels, modeTone } from "./ModeSelect";
import { sameTag, uniqueTags } from "./TagEditor";
import { tagColor, useTagColors } from "./tagColors";

export const WRITE_MODES: WriteMode[] = ["read_only", "approval", "temporary", "permanent"];

/** Filter button with a panel to pick tags (all must match) and write modes (any may match). */
export default function EnvironmentFilters({ envs, tags, setTags, modes, setModes }: {
  envs: Environment[];
  tags: string[];
  setTags: (tags: string[]) => void;
  modes: WriteMode[];
  setModes: (modes: WriteMode[]) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);
  const colors = useTagColors();
  const allTags = uniqueTags(envs.flatMap((env) => env.tags)).sort((a, b) => a.localeCompare(b));
  const count = (tag: string) => envs.filter((env) => env.tags.some((other) => sameTag(other, tag))).length;
  const checked = (tag: string) => tags.some((other) => sameTag(other, tag));
  const q = query.trim().toLowerCase();
  const listed = allTags.filter((tag) => !q || tag.toLowerCase().includes(q));
  const active = tags.length + modes.length;
  const toggle = (tag: string) => setTags(checked(tag) ? tags.filter((other) => !sameTag(other, tag)) : [...tags, tag]);

  return <div className="filter-menu" ref={root} onKeyDown={(e) => { if (e.key === "Escape" && open) { e.stopPropagation(); setOpen(false); } }}>
    <button type="button" className={`btn sm icon-only${active ? " filter-active" : ""}`} aria-expanded={open} aria-haspopup="dialog"
      aria-label={t("environments.filters")} data-tip={t("environments.filters")} onClick={() => setOpen(!open)}>
      <Icon name="filter" />{active > 0 && <span className="filter-badge" aria-hidden="true">{active}</span>}
    </button>
    {open && <div className="filter-panel" role="dialog" aria-label={t("environments.filters")}>
      <div className="filter-section">
        <span className="filter-heading">{t("environments.tags")}</span>
        {allTags.length > 8 && <label className="search"><Icon name="search" /><input autoFocus type="search" value={query} placeholder={t("environments.searchTags")} aria-label={t("environments.searchTags")} onChange={(e) => setQuery(e.target.value)} /></label>}
        {allTags.length === 0 ? <p className="hint">{t("environments.noTagsYet")}</p>
          : <ul className="filter-options" aria-label={t("environments.tagFilter")}>
            {listed.map((tag) => <li key={tag}><label className="filter-option">
              <input type="checkbox" checked={checked(tag)} onChange={() => toggle(tag)} />
              <span className={`tag-dot tone-${tagColor(colors, tag)}`} aria-hidden="true" /><span className="truncate">{tag}</span><span className="filter-count">{count(tag)}</span>
            </label></li>)}
            {listed.length === 0 && <li className="hint">{t("environments.noMatches")}</li>}
          </ul>}
      </div>
      <div className="filter-section">
        <span className="filter-heading">{t("environments.writeMode")}</span>
        <ul className="filter-options" aria-label={t("environments.modeFilter")}>
          {WRITE_MODES.map((mode) => <li key={mode}><label className="filter-option">
            <input type="checkbox" checked={modes.includes(mode)} onChange={() => setModes(modes.includes(mode) ? modes.filter((m) => m !== mode) : [...modes, mode])} />
            <span className={`filter-mode tone-${modeTone[mode]}`} aria-hidden="true"><Icon name={modeIcons[mode]} /></span>
            <span className="truncate">{t(modeLabels[mode] as "mode.readOnly")}</span><span className="filter-count">{envs.filter((env) => env.write_mode === mode).length}</span>
          </label></li>)}
        </ul>
      </div>
      {active > 0 && <div className="filter-footer"><button type="button" className="btn sm ghost" onClick={() => { setTags([]); setModes([]); }}>{t("environments.clearFilters")}</button></div>}
    </div>}
  </div>;
}
