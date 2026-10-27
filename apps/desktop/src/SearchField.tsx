import type { InputHTMLAttributes, ReactNode, RefObject } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import { searchIcon } from "./ui";

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
  return <div className={`relative group/search flex items-center gap-0.75 min-h-6 py-0.5 pr-1 pl-6.25 border rounded-md bg-surface cursor-text focus-within:border-accent focus-within:ring-3 focus-within:ring-accent-soft ${active ? "border-accent" : "border-line-strong"} ${className}`} onMouseDown={(e) => { if (e.target === e.currentTarget) { e.preventDefault(); inputRef?.current?.focus(); } }}>
    <Icon name="search" className={searchIcon} />
    {chips && <span className="flex flex-initial gap-0.75 min-w-0 overflow-x-auto [scrollbar-width:none] [&>.chip]:flex-none">{chips}</span>}
    <input ref={inputRef} type="search" className="flex-[1_1_40px] min-w-8 h-5 px-0.5 text-sm border-0 bg-transparent shadow-none" aria-label={label} placeholder={placeholder} {...inputProps} value={value}
      onChange={(e) => onChange(e.target.value)}
      onKeyDown={(e) => {
        inputProps?.onKeyDown?.(e);
        if (!e.defaultPrevented && e.key === "Escape" && value) { e.preventDefault(); onChange(""); }
      }} />
    {active
      ? <>{count && <span className="flex-none text-fg-3 text-xs tabular-nums" role="status"><span aria-hidden="true">{count.shown}/{count.total}</span><span className="sr-only">{count.text}</span></span>}
        <button type="button" className="inline-grid place-items-center size-4 flex-none p-0 border-0 rounded-sm bg-transparent text-fg-3 opacity-70 cursor-pointer hover:opacity-100 hover:bg-hover" onClick={() => { if (onClear) onClear(); else onChange(""); inputRef?.current?.focus(); }} aria-label={clearLabel ?? t("common.clearSearch")} data-tip={clearLabel ?? t("common.clearSearch")}><Icon name="x" className="size-3" /></button></>
      : shortcut && !value && <kbd className="min-w-4 h-4 px-1 border border-line-strong rounded-sm font-mono text-2xs font-medium leading-[14px] text-center text-fg-3 pointer-events-none group-focus-within/search:hidden" aria-hidden="true">/</kbd>}
  </div>;
}
