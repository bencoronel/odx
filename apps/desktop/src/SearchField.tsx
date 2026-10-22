import type { InputHTMLAttributes, ReactNode, RefObject } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";

/** The app's search box: optional filter chips inside it, a result count and a clear button while filtered. Escape clears the text. */
export default function SearchField({ value, onChange, label, placeholder, inputRef, shortcut, chips, filtered, count, onClear, clearLabel, className = "", inputProps }: {
  value: string;
  onChange: (value: string) => void;
  label: string;
  placeholder?: string;
  inputRef?: RefObject<HTMLInputElement | null>;
  /** Shows the "/" hint while empty; the page binds the key. */
  shortcut?: boolean;
  chips?: ReactNode;
  /** Text or other filters are active: show the count and the clear button. */
  filtered?: boolean;
  count?: { shown: number; total: number; text: string };
  onClear?: () => void;
  /** Defaults to "Clear search"; say "Clear filters" when the button also drops filters. */
  clearLabel?: string;
  className?: string;
  inputProps?: InputHTMLAttributes<HTMLInputElement>;
}) {
  const { t } = useTranslation();
  const active = filtered ?? !!value;
  return <div className={`search token-search${active ? " filtered" : ""} ${className}`} onMouseDown={(e) => { if (e.target === e.currentTarget) { e.preventDefault(); inputRef?.current?.focus(); } }}>
    <Icon name="search" />
    {chips && <span className="token-chips">{chips}</span>}
    <input ref={inputRef} type="search" aria-label={label} placeholder={placeholder} {...inputProps} value={value}
      onChange={(e) => onChange(e.target.value)}
      onKeyDown={(e) => {
        inputProps?.onKeyDown?.(e);
        if (!e.defaultPrevented && e.key === "Escape" && value) { e.preventDefault(); onChange(""); }
      }} />
    {active
      ? <>{count && <span className="token-count" role="status"><span aria-hidden="true">{count.shown}/{count.total}</span><span className="sr-only">{count.text}</span></span>}
        <button type="button" className="token-clear" onClick={() => { if (onClear) onClear(); else onChange(""); inputRef?.current?.focus(); }} aria-label={clearLabel ?? t("common.clearSearch")} data-tip={clearLabel ?? t("common.clearSearch")}><Icon name="x" /></button></>
      : shortcut && !value && <kbd className="search-kbd" aria-hidden="true">/</kbd>}
  </div>;
}
