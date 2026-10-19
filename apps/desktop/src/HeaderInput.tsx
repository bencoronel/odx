import { useEffect, useId, useRef, useState, type ComponentProps } from "react";
import { useTranslation } from "react-i18next";

// IANA permanent HTTP fields, retrieved 2026-10-16.
// https://www.iana.org/assignments/http-fields/field-names.csv
import standardHeaderNames from "./http-header-names.json";
export const headerNames = standardHeaderNames;
export const reservedHeaderNames = ["host", "content-length", "transfer-encoding", "connection", "trailer", "upgrade", "proxy-authorization", "proxy-connection", "te"];
export const headerValues = new Map<string, string[]>([
  ["accept", ["application/json", "*/*"]],
  // The transport does not decompress responses; omit gzip/br suggestions.
  ["accept-encoding", ["identity"]],
  ["accept-language", ["es-ES", "en-US", "es", "en", "es-ES,es;q=0.9,en;q=0.8"]],
  ["cache-control", ["no-cache", "no-store", "max-age=0"]],
  ["content-type", ["application/json"]],
  ["prefer", ["return=minimal", "return=representation"]],
]);

type Props = Omit<ComponentProps<"input">, "value" | "onChange" | "onBlur"> & {
  value: string;
  suggestions: string[];
  disabledSuggestions?: string[];
  caseSensitive?: boolean;
  /** List every suggestion while the value is empty, opening on focus. */
  showAll?: boolean;
  onChange: (value: string) => void;
  /** Receives a chosen suggestion instead of `onChange`. */
  onPick?: (value: string) => void;
  onBlur: () => void;
};
export default function HeaderInput({ value, suggestions, disabledSuggestions = [], caseSensitive = false, showAll = false, onChange, onPick, onBlur, ...props }: Props) {
  const { t } = useTranslation();
  const id = useId();
  const menu = useRef<HTMLUListElement>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const matches = value.trim() ? suggestions.filter((s) => s.toLowerCase().includes(value.toLowerCase()) && (caseSensitive ? s !== value : s.toLowerCase() !== value.toLowerCase())) : showAll ? suggestions : [];
  const disabled = (suggestion: string) => disabledSuggestions.includes(suggestion.toLowerCase());
  const selectable = matches.map((s, i) => disabled(s) ? -1 : i).filter((i) => i >= 0);
  const expanded = open && matches.length > 0;
  useEffect(() => {
    if (expanded && active >= 0) menu.current?.children[active]?.scrollIntoView?.({ block: "nearest" });
  }, [expanded, active]);
  const choose = (suggestion: string) => { if (disabled(suggestion)) return; (onPick ?? onChange)(suggestion); setOpen(false); setActive(-1); };
  return <div className="header-input">
    <input {...props} value={value} role="combobox" aria-autocomplete="list" aria-expanded={expanded}
      aria-controls={expanded ? id : undefined} aria-activedescendant={expanded && active >= 0 && active < matches.length ? `${id}-${active}` : undefined}
      onFocus={() => { setOpen(showAll); setActive(-1); }}
      onChange={(e) => { onChange(e.target.value); setOpen(true); setActive(-1); }}
      onBlur={() => { setOpen(false); setActive(-1); onBlur(); }}
      onKeyDown={(e) => {
        if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); setOpen(false); setActive(-1); }
        else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && selectable.length) {
          e.preventDefault(); setOpen(true);
          const position = selectable.indexOf(active);
          const next = selectable[!expanded || position < 0 ? (e.key === "ArrowDown" ? 0 : selectable.length - 1) : (position + (e.key === "ArrowDown" ? 1 : -1) + selectable.length) % selectable.length];
          setActive(next);
        } else if (e.key === "Enter" && expanded && active >= 0 && active < matches.length) {
          e.preventDefault(); e.stopPropagation(); choose(matches[active]);
        }
      }} />
    {expanded && <ul id={id} ref={menu} className="term-menu header-suggestions" role="listbox" aria-label={t("headerInput.suggestionsFor", { field: props["aria-label"] })}>
      {matches.map((suggestion, index) => <li key={suggestion} id={`${id}-${index}`} role="option" aria-label={suggestion} aria-description={disabled(suggestion) ? t("headerInput.managedAutomatically") : undefined} aria-selected={index === active} aria-disabled={disabled(suggestion)}
        onMouseEnter={() => { if (!disabled(suggestion)) setActive(index); }} onMouseDown={(e) => { e.preventDefault(); choose(suggestion); }}>
        <span className="term-menu-value">{suggestion}</span>
        {disabled(suggestion) && <span className="term-menu-detail">{t("headerInput.managedAutomatically")}</span>}
      </li>)}
    </ul>}
  </div>;
}
