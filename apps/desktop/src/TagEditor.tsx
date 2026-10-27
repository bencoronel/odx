import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import HeaderInput from "./HeaderInput";
import { Icon } from "./Icon";
import { TAG_COLORS, setTagColor, tagColor, tagTone, useTagColors } from "./tagColors";
import { useDismiss } from "./ContextMenu";
import { chip, cx } from "./ui";

export const MAX_TAGS = 20;
export const sameTag = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
/** First spelling of each tag, ignoring case. */
export const uniqueTags = (tags: string[]) => tags.filter((tag, i) => tags.findIndex((other) => sameTag(other, tag)) === i);

/** Tag chips plus an input that suggests tags used by other environments. */
export default function TagEditor({ tags, suggestions, disabled, collapsed, onChange }: {
  tags: string[]; suggestions: string[]; disabled?: boolean;
  /** Show an add button until the user starts adding. */
  collapsed?: boolean;
  onChange: (tags: string[]) => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");
  const [adding, setAdding] = useState(!collapsed);
  const colors = useTagColors();
  const [picking, setPicking] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  useDismiss(!!picking, () => root.current?.querySelector(".tag-colors"), () => setPicking(null));
  const add = (value: string) => {
    const tag = value.trim();
    setDraft("");
    if (tag && tags.length < MAX_TAGS && !tags.some((existing) => sameTag(existing, tag))) onChange([...tags, tag]);
  };
  return <div className="flex flex-wrap items-center gap-1 min-w-0 min-h-[22px] mt-1 [&_.header-suggestions]:min-w-[180px]" ref={root} onKeyDown={(e) => {
    if (e.key === "Escape" && picking) { e.stopPropagation(); setPicking(null); return; }
    // Enter adds the tag instead of submitting a surrounding form; a chosen suggestion stops propagation.
    if (e.key === "Enter" && e.target instanceof HTMLInputElement) { e.preventDefault(); add(draft); }
  }}>
    {tags.map((tag) => <span key={tag} className={cx(chip({ tone: tagTone(colors, tag) }), "relative pl-0 pr-0.5")}>
      <button type="button" className="p-0 pr-0.5 pl-1.75 border-0 bg-transparent cursor-pointer disabled:cursor-default" disabled={disabled} aria-haspopup="true" aria-expanded={picking === tag} aria-label={t("environments.tagColor", { tag })} data-tip={picking === tag ? undefined : t("environments.tagColorTip")} onClick={() => setPicking(picking === tag ? null : tag)}>{tag}</button>
      {picking === tag && <span className="tag-colors absolute left-0 top-[calc(100%+4px)] z-30 flex gap-1.5 p-1.5 border border-line-strong rounded-md bg-surface shadow-pop" role="radiogroup" aria-label={t("environments.tagColor", { tag })}>
        {TAG_COLORS.map((color) => <button key={color} type="button" role="radio" aria-checked={tagColor(colors, tag) === color} className={`size-4.5 p-0 border rounded-full cursor-pointer tone-${color} ${color === "neutral" ? "border-line-strong bg-surface-2 aria-checked:shadow-[0_0_0_2px_var(--surface),0_0_0_4px_var(--text-3)]" : "border-tone bg-tone-soft aria-checked:shadow-[0_0_0_2px_var(--surface),0_0_0_4px_var(--tone)]"}`} aria-label={t(`environments.colors.${color}`)} data-tip={t(`environments.colors.${color}`)} autoFocus={tagColor(colors, tag) === color} onClick={() => { setTagColor(tag, color); setPicking(null); }} />)}
      </span>}
      <button type="button" className={`inline-grid place-items-center size-4 p-0 border-0 rounded-sm bg-transparent cursor-pointer enabled:hover:text-fg enabled:hover:bg-hover ${tagTone(colors, tag) ? "text-tone" : "text-fg-3"}`} disabled={disabled} aria-label={t("environments.removeTag", { tag })} onClick={() => onChange(tags.filter((existing) => existing !== tag))}><Icon name="x" className="size-2.5!" /></button>
    </span>)}
    {tags.length < MAX_TAGS && (adding
      ? <HeaderInput className="w-[130px] h-[22px] px-1.75 text-xs" aria-label={t("environments.addTag")} placeholder={t("environments.addTag")} maxLength={32} disabled={disabled} autoFocus={collapsed}
        value={draft} showAll suggestions={suggestions.filter((s) => !tags.some((tag) => sameTag(tag, s)))}
        onChange={setDraft} onPick={add} onBlur={() => { add(draft); if (collapsed) setAdding(false); }} />
      : <button type="button" className="inline-flex items-center gap-1 h-5 px-1.5 border border-dashed border-line-strong rounded-sm bg-transparent text-fg-3 text-xs cursor-pointer enabled:hover:text-fg enabled:hover:border-fg-3" disabled={disabled} aria-label={t("environments.addTag")} onClick={() => setAdding(true)}><Icon name="plus" className="size-[11px]" />{tags.length ? "" : t("environments.addTag")}</button>)}
  </div>;
}
