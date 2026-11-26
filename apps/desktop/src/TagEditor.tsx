import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import { EXTRA_HEX_SWATCHES, TAG_COLORS, isHexColor, setTagColor, tagChipStyle, tagColor, tagColorHex, tagDotProps, tagTone, useTagColors, type TagColor } from "./tagColors";
import { useDismiss } from "./ContextMenu";
import { chip, cx } from "./ui";

export const MAX_TAGS = 20;
export const sameTag = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
/** First spelling of each tag, ignoring case. */
export const uniqueTags = (tags: string[]) => tags.filter((tag, i) => tags.findIndex((other) => sameTag(other, tag)) === i);

function ColorSwatchBar({ current, label, onSelect }: { current: TagColor; label: string; onSelect: (color: TagColor) => void }) {
  const { t } = useTranslation();
  const [hexDraft, setHexDraft] = useState(() => tagColorHex(current));
  const applyHex = (raw: string) => {
    setHexDraft(raw);
    const cleaned = raw.trim().startsWith("#") ? raw.trim() : `#${raw.trim()}`;
    if (isHexColor(cleaned)) onSelect(cleaned.toLowerCase() as TagColor);
  };
  return <div className="grid gap-2">
    <div className="flex flex-wrap items-center gap-1.5" role="radiogroup" aria-label={label}>
      {TAG_COLORS.map((color) => <button key={color} type="button" role="radio" aria-checked={current === color}
        className={`size-4.5 p-0 border rounded-full cursor-pointer tone-${color} ${color === "neutral" ? "border-line-strong bg-fg-3/50 aria-checked:shadow-[0_0_0_2px_var(--surface),0_0_0_4px_var(--text-3)]" : "border-tone bg-tone aria-checked:shadow-[0_0_0_2px_var(--surface),0_0_0_4px_var(--tone)]"}`}
        aria-label={t(`environments.colors.${color}`)} data-tip={t(`environments.colors.${color}`)} autoFocus={current === color}
        onClick={() => { setHexDraft(tagColorHex(color)); onSelect(color); }} />)}
      {EXTRA_HEX_SWATCHES.map((hex) => <button key={hex} type="button" role="radio" aria-checked={current.toLowerCase() === hex}
        className="size-4.5 p-0 border rounded-full cursor-pointer"
        style={{ backgroundColor: hex, borderColor: hex, boxShadow: current.toLowerCase() === hex ? `0 0 0 2px var(--surface), 0 0 0 4px ${hex}` : undefined }}
        aria-label={hex} data-tip={hex}
        onClick={() => { setHexDraft(hex); onSelect(hex); }} />)}
    </div>
    <div className="flex items-center gap-1.5 pt-1 border-t border-line">
      <label className="relative size-6 rounded-sm overflow-hidden border border-line-strong cursor-pointer flex-none" title={t("environments.customColor")}>
        <input type="color" className="absolute -inset-1 size-8 p-0 border-0 cursor-pointer bg-transparent" aria-label={t("environments.customColor")}
          value={tagColorHex(current)} onChange={(e) => applyHex(e.target.value)} />
      </label>
      <input type="text" className="h-6! px-1.5! font-mono text-xs! w-[86px]!" aria-label={t("environments.customColorHex")}
        value={hexDraft} maxLength={7} placeholder="#7465ee"
        onChange={(e) => applyHex(e.target.value)} />
    </div>
  </div>;
}

/** Tag chips plus a searchable multi-select & creator with full color picker. */
export default function TagEditor({ tags, suggestions, disabled, collapsed, onChange }: {
  tags: string[]; suggestions: string[]; disabled?: boolean;
  /** Show an add button until the user starts adding. */
  collapsed?: boolean;
  onChange: (tags: string[]) => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");
  const [adding, setAdding] = useState(!collapsed);
  const [menuOpen, setMenuOpen] = useState(false);
  const [newColor, setNewColor] = useState<TagColor>("neutral");
  const colors = useTagColors();
  const [picking, setPicking] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  useDismiss(!!picking, () => root.current?.querySelector(".tag-colors"), () => setPicking(null));
  useDismiss(menuOpen, () => root.current?.querySelector(".tag-picker-wrap"), () => {
    setMenuOpen(false);
    if (collapsed && !draft.trim()) setAdding(false);
  });

  const add = (value: string, chosenColor?: TagColor) => {
    const tag = value.trim();
    setDraft("");
    if (!tag || tags.length >= MAX_TAGS || tags.some((existing) => sameTag(existing, tag))) return;
    if (chosenColor && chosenColor !== "neutral") setTagColor(tag, chosenColor);
    setNewColor("neutral");
    onChange([...tags, tag]);
  };

  const toggleTag = (tag: string) => {
    if (tags.some((existing) => sameTag(existing, tag))) {
      onChange(tags.filter((existing) => !sameTag(existing, tag)));
    } else if (tags.length < MAX_TAGS) {
      onChange([...tags, tag]);
    }
  };

  const q = draft.trim().toLowerCase();
  const allKnown = uniqueTags([...suggestions, ...tags]).sort((a, b) => a.localeCompare(b));
  const filteredSuggestions = allKnown.filter((s) => !q || s.toLowerCase().includes(q));
  const exactExists = !!q && allKnown.some((s) => s.toLowerCase() === q);
  const showPopover = menuOpen && (filteredSuggestions.length > 0 || (!!q && !exactExists));

  return <div className="flex flex-wrap items-center gap-1 min-w-0 min-h-[22px] mt-1" ref={root} onKeyDown={(e) => {
    if (e.key === "Escape") {
      if (picking) { e.stopPropagation(); setPicking(null); return; }
      if (menuOpen) { e.stopPropagation(); setMenuOpen(false); if (collapsed) setAdding(false); return; }
    }
    if (e.key === "Enter" && e.target === inputRef.current) {
      e.preventDefault();
      add(draft, !exactExists ? newColor : undefined);
    }
  }}>
    {tags.map((tag) => <span key={tag} className={cx(chip({ tone: tagTone(colors, tag) }), "relative pl-0 pr-0.5")} style={tagChipStyle(colors, tag)}>
      <button type="button" className="p-0 pr-0.5 pl-1.75 border-0 bg-transparent cursor-pointer disabled:cursor-default text-inherit" disabled={disabled} aria-haspopup="true" aria-expanded={picking === tag} aria-label={t("environments.tagColor", { tag })} data-tip={picking === tag ? undefined : t("environments.tagColorTip")} onClick={() => { setMenuOpen(false); setPicking(picking === tag ? null : tag); }}>{tag}</button>
      {picking === tag && <div className="tag-colors absolute left-0 top-[calc(100%+4px)] z-30 grid gap-2 p-2.5 w-[212px] border border-line-strong rounded-md bg-surface shadow-pop">
        <div className="flex items-center justify-between gap-2">
          <span className={chip({ tone: tagTone(colors, tag) })} style={tagChipStyle(colors, tag)}>{tag}</span>
        </div>
        <ColorSwatchBar current={tagColor(colors, tag)} label={t("environments.tagColor", { tag })}
          onSelect={(color) => {
            setTagColor(tag, color);
            if (!isHexColor(color)) setPicking(null);
          }} />
      </div>}
      <button type="button" className={`inline-grid place-items-center size-4 p-0 border-0 rounded-sm bg-transparent cursor-pointer enabled:hover:text-fg enabled:hover:bg-hover ${tagTone(colors, tag) || isHexColor(tagColor(colors, tag)) ? "text-current opacity-80" : "text-fg-3"}`} disabled={disabled} aria-label={t("environments.removeTag", { tag })} onClick={() => onChange(tags.filter((existing) => existing !== tag))}><Icon name="x" className="size-2.5!" /></button>
    </span>)}
    {tags.length < MAX_TAGS && (adding
      ? <div className="tag-picker-wrap relative min-w-0">
          <input ref={inputRef} role="combobox" aria-autocomplete="list" aria-expanded={showPopover}
            className="w-[140px]! h-[22px]! px-1.75! text-xs!" aria-label={t("environments.addTag")} placeholder={t("environments.addTag")} maxLength={32} disabled={disabled} autoFocus={collapsed}
            value={draft}
            onFocus={() => { setPicking(null); setMenuOpen(true); }}
            onChange={(e) => { setDraft(e.target.value); setMenuOpen(true); }}
            onKeyDown={(e) => {
              if (e.key === "Backspace" && !draft && tags.length > 0) {
                onChange(tags.slice(0, -1));
              }
            }}
            onBlur={(e) => {
              if (e.relatedTarget && root.current?.querySelector(".tag-picker-wrap")?.contains(e.relatedTarget as Node)) return;
              if (draft.trim()) add(draft, !exactExists ? newColor : undefined);
              setMenuOpen(false);
              if (collapsed) setAdding(false);
            }} />
          {showPopover && <div className="absolute left-0 top-[calc(100%+4px)] z-30 grid gap-1.5 w-[236px] p-1.5 border border-line-strong rounded-md bg-surface shadow-pop">
            {filteredSuggestions.length > 0 && <ul className="grid max-h-[180px] overflow-auto gap-0.5" role="listbox" aria-label={t("environments.tags")}>
              {filteredSuggestions.map((tag) => {
                const selected = tags.some((existing) => sameTag(existing, tag));
                const dot = tagDotProps(colors, tag);
                return <li key={tag} role="option" aria-label={tag} aria-selected={selected}
                  className="flex items-center gap-2 px-2 h-6 rounded-sm text-xs text-fg cursor-pointer hover:bg-hover"
                  onMouseDown={(e) => { e.preventDefault(); toggleTag(tag); setDraft(""); }}>
                  <input type="checkbox" tabIndex={-1} readOnly checked={selected} className="pointer-events-none" />
                  <span className={dot.className} style={dot.style} aria-hidden="true" />
                  <span className="truncate">{tag}</span>
                </li>;
              })}
            </ul>}
            {draft.trim() && !exactExists && <div className="grid gap-2 pt-1.5 px-1 border-t border-line first:border-t-0 first:pt-0.5">
              <button type="button" className="flex items-center justify-between gap-2 px-1.5 py-1 rounded-sm bg-surface-2 hover:bg-hover text-xs text-fg cursor-pointer border border-line"
                onMouseDown={(e) => { e.preventDefault(); add(draft, newColor); }}>
                <span className="inline-flex items-center gap-1.5 truncate"><Icon name="plus" className="size-3 text-accent-text" />{t("environments.createTag", { tag: draft.trim() })}</span>
                <span className={chip({ tone: isHexColor(newColor) || newColor === "neutral" ? undefined : newColor })}
                  style={isHexColor(newColor) ? { color: newColor, backgroundColor: `color-mix(in srgb, ${newColor} 18%, transparent)`, borderColor: `color-mix(in srgb, ${newColor} 32%, transparent)` } : undefined}>{draft.trim()}</span>
              </button>
              <ColorSwatchBar current={newColor} label={t("environments.tagColor", { tag: draft.trim() })} onSelect={setNewColor} />
            </div>}
          </div>}
        </div>
      : <button type="button" className="inline-flex items-center gap-1 h-5 px-1.5 border border-dashed border-line-strong rounded-sm bg-transparent text-fg-3 text-xs cursor-pointer enabled:hover:text-fg enabled:hover:border-fg-3" disabled={disabled} aria-label={t("environments.addTag")} onClick={() => { setAdding(true); setMenuOpen(true); }}><Icon name="plus" className="size-[11px]" />{tags.length ? "" : t("environments.addTag")}</button>)}
  </div>;
}
