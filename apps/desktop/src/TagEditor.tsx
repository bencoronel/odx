import { useState } from "react";
import { useTranslation } from "react-i18next";
import HeaderInput from "./HeaderInput";
import { Icon } from "./Icon";

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
  const add = (value: string) => {
    const tag = value.trim();
    setDraft("");
    if (tag && tags.length < MAX_TAGS && !tags.some((existing) => sameTag(existing, tag))) onChange([...tags, tag]);
  };
  return <div className="tag-editor" onKeyDown={(e) => {
    // Enter adds the tag instead of submitting a surrounding form; a chosen suggestion stops propagation.
    if (e.key === "Enter" && e.target instanceof HTMLInputElement) { e.preventDefault(); add(draft); }
  }}>
    {tags.map((tag) => <span key={tag} className="chip tag-chip">{tag}
      <button type="button" className="tag-remove" disabled={disabled} aria-label={t("environments.removeTag", { tag })} onClick={() => onChange(tags.filter((existing) => existing !== tag))}><Icon name="x" /></button>
    </span>)}
    {tags.length < MAX_TAGS && (adding
      ? <HeaderInput className="tag-input" aria-label={t("environments.addTag")} placeholder={t("environments.addTag")} maxLength={32} disabled={disabled} autoFocus={collapsed}
        value={draft} showAll suggestions={suggestions.filter((s) => !tags.some((tag) => sameTag(tag, s)))}
        onChange={setDraft} onPick={add} onBlur={() => { add(draft); if (collapsed) setAdding(false); }} />
      : <button type="button" className="tag-add" disabled={disabled} aria-label={t("environments.addTag")} onClick={() => setAdding(true)}><Icon name="plus" />{tags.length ? "" : t("environments.addTag")}</button>)}
  </div>;
}
