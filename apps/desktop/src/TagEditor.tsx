import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import HeaderInput from "./HeaderInput";
import { Icon } from "./Icon";
import { TAG_COLORS, setTagColor, tagColor, tagTone, useTagColors } from "./tagColors";
import { useDismiss } from "./ContextMenu";

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
  return <div className="tag-editor" ref={root} onKeyDown={(e) => {
    if (e.key === "Escape" && picking) { e.stopPropagation(); setPicking(null); return; }
    // Enter adds the tag instead of submitting a surrounding form; a chosen suggestion stops propagation.
    if (e.key === "Enter" && e.target instanceof HTMLInputElement) { e.preventDefault(); add(draft); }
  }}>
    {tags.map((tag) => <span key={tag} className={`chip tag-chip${tagTone(colors, tag)}`}>
      <button type="button" className="tag-label" disabled={disabled} aria-haspopup="true" aria-expanded={picking === tag} aria-label={t("environments.tagColor", { tag })} data-tip={picking === tag ? undefined : t("environments.tagColorTip")} onClick={() => setPicking(picking === tag ? null : tag)}>{tag}</button>
      {picking === tag && <span className="tag-colors" role="radiogroup" aria-label={t("environments.tagColor", { tag })}>
        {TAG_COLORS.map((color) => <button key={color} type="button" role="radio" aria-checked={tagColor(colors, tag) === color} className={`tag-swatch tone-${color}`} aria-label={t(`environments.colors.${color}`)} data-tip={t(`environments.colors.${color}`)} autoFocus={tagColor(colors, tag) === color} onClick={() => { setTagColor(tag, color); setPicking(null); }} />)}
      </span>}
      <button type="button" className="tag-remove" disabled={disabled} aria-label={t("environments.removeTag", { tag })} onClick={() => onChange(tags.filter((existing) => existing !== tag))}><Icon name="x" /></button>
    </span>)}
    {tags.length < MAX_TAGS && (adding
      ? <HeaderInput className="tag-input" aria-label={t("environments.addTag")} placeholder={t("environments.addTag")} maxLength={32} disabled={disabled} autoFocus={collapsed}
        value={draft} showAll suggestions={suggestions.filter((s) => !tags.some((tag) => sameTag(tag, s)))}
        onChange={setDraft} onPick={add} onBlur={() => { add(draft); if (collapsed) setAdding(false); }} />
      : <button type="button" className="tag-add" disabled={disabled} aria-label={t("environments.addTag")} onClick={() => setAdding(true)}><Icon name="plus" />{tags.length ? "" : t("environments.addTag")}</button>)}
  </div>;
}
