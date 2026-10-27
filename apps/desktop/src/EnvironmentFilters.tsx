import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import type { Environment, WriteMode } from "./ipc";
import { modeIcons, modeLabels, modeTone } from "./ModeSelect";
import { sameTag, uniqueTags } from "./TagEditor";
import { tagColor, useTagColors } from "./tagColors";
import { useDismiss } from "./ContextMenu";
import { Button, filterBadge, filterCount, filterHeading, filterOption, filterOptions, filterPanel, filterSection, searchIcon, searchInput } from "./ui";

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
  useDismiss(open, () => root.current, () => setOpen(false));
  const colors = useTagColors();
  const allTags = uniqueTags(envs.flatMap((env) => env.tags)).sort((a, b) => a.localeCompare(b));
  const count = (tag: string) => envs.filter((env) => env.tags.some((other) => sameTag(other, tag))).length;
  const checked = (tag: string) => tags.some((other) => sameTag(other, tag));
  const q = query.trim().toLowerCase();
  const listed = allTags.filter((tag) => !q || tag.toLowerCase().includes(q));
  const active = tags.length + modes.length;
  const toggle = (tag: string) => setTags(checked(tag) ? tags.filter((other) => !sameTag(other, tag)) : [...tags, tag]);

  return <div className="flex-none" ref={root} onKeyDown={(e) => { if (e.key === "Escape" && open) { e.stopPropagation(); setOpen(false); } }}>
    <Button size="sm" iconOnly variant={active ? "accent" : "default"} className="relative" aria-expanded={open} aria-haspopup="dialog"
      aria-label={t("environments.filters")} data-tip={t("environments.filters")} onClick={() => setOpen(!open)}>
      <Icon name="filter" />{active > 0 && <span className={filterBadge} aria-hidden="true">{active}</span>}
    </Button>
    {open && <div className={filterPanel} role="dialog" aria-label={t("environments.filters")}>
      <div className={filterSection}>
        <span className={filterHeading}>{t("environments.tags")}</span>
        {allTags.length > 8 && <label className="relative block"><Icon name="search" className={searchIcon} /><input className={searchInput} autoFocus type="search" value={query} placeholder={t("environments.searchTags")} aria-label={t("environments.searchTags")} onChange={(e) => setQuery(e.target.value)} /></label>}
        {allTags.length === 0 ? <p className="text-fg-3 text-sm">{t("environments.noTagsYet")}</p>
          : <ul className={filterOptions} aria-label={t("environments.tagFilter")}>
            {listed.map((tag) => <li key={tag}><label className={filterOption}>
              <input type="checkbox" checked={checked(tag)} onChange={() => toggle(tag)} />
              <span className={`size-2 flex-none rounded-full tone-${tagColor(colors, tag)} ${tagColor(colors, tag) === "neutral" ? "bg-line-strong" : "bg-tone"}`} aria-hidden="true" /><span className="truncate">{tag}</span><span className={filterCount}>{count(tag)}</span>
            </label></li>)}
            {listed.length === 0 && <li className="text-fg-3 text-sm">{t("environments.noMatches")}</li>}
          </ul>}
      </div>
      <div className={filterSection}>
        <span className={filterHeading}>{t("environments.writeMode")}</span>
        <ul className={filterOptions} aria-label={t("environments.modeFilter")}>
          {WRITE_MODES.map((mode) => <li key={mode}><label className={filterOption}>
            <input type="checkbox" checked={modes.includes(mode)} onChange={() => setModes(modes.includes(mode) ? modes.filter((m) => m !== mode) : [...modes, mode])} />
            <span className={`inline-grid place-items-center w-3.5 text-tone tone-${modeTone[mode]}`} aria-hidden="true"><Icon name={modeIcons[mode]} className="size-[13px]" /></span>
            <span className="truncate">{t(modeLabels[mode] as "mode.readOnly")}</span><span className={filterCount}>{envs.filter((env) => env.write_mode === mode).length}</span>
          </label></li>)}
        </ul>
      </div>
      {active > 0 && <div className="flex justify-end px-2 py-1.5 border-t border-line"><Button size="sm" variant="ghost" onClick={() => { setTags([]); setModes([]); }}>{t("environments.clearFilters")}</Button></div>}
    </div>}
  </div>;
}
