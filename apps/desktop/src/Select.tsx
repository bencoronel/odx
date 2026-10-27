import { createPortal } from "react-dom";
import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { Icon } from "./Icon";
import { cx, selectOption } from "./ui";

/** Looks like an input; the width has zero specificity so a caller's `w-*` wins. The chevron is two gradient triangles. */
const control = "[:where(&)]:w-full h-7 flex items-center justify-start px-[9px] pr-[30px] border border-line-strong rounded-md text-fg text-base text-left truncate appearance-none cursor-pointer transition-[border-color,box-shadow] duration-120 bg-surface-2 bg-[image:linear-gradient(45deg,transparent_50%,var(--text-2)_50%),linear-gradient(135deg,var(--text-2)_50%,transparent_50%)] bg-[position:calc(100%-13px)_11px,calc(100%-9px)_11px] bg-[size:4px_4px,4px_4px] bg-no-repeat enabled:hover:border-accent focus-visible:border-accent focus-visible:shadow-[0_0_0_3px_var(--accent-soft)] focus-visible:outline-none disabled:opacity-55 disabled:cursor-not-allowed";

export type SelectOption<T extends string> = { value: T; label: string };

/** Use a themed dropdown. */
export default function Select<T extends string>({
  id, className = "", label, value, options, onChange,
}: {
  id?: string;
  className?: string;
  label: string;
  value: T;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
}) {
  const generatedId = useId();
  const listId = `${id ?? generatedId}-options`;
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(Math.max(0, options.findIndex((option) => option.value === value)));
  const [placement, setPlacement] = useState<{ top?: number; bottom?: number; left: number; width: number; maxHeight: number } | null>(null);

  useEffect(() => {
    if (!open) return;
    const updatePlacement = () => {
      const rect = trigger.current?.getBoundingClientRect();
      if (!rect) return;
      const maxHeight = Math.min(240, window.innerHeight - 16);
      const below = window.innerHeight - rect.bottom;
      const above = rect.top;
      const openBelow = below >= Math.min(maxHeight, options.length * 32 + 6) || below >= above;
      const availableHeight = openBelow ? Math.min(maxHeight, below - 8) : Math.min(maxHeight, above - 8);
      // Anchor above by the bottom edge; actual height may be below the maximum.
      const position = openBelow ? { top: rect.bottom + 4 } : { bottom: window.innerHeight - rect.top + 4 };
      setPlacement({ ...position, left: Math.max(8, Math.min(rect.left, window.innerWidth - rect.width - 8)), width: rect.width, maxHeight: availableHeight });
    };
    const closeOutside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!trigger.current?.contains(target) && !menu.current?.contains(target)) setOpen(false);
    };
    updatePlacement();
    window.addEventListener("resize", updatePlacement);
    window.addEventListener("scroll", updatePlacement, true);
    document.addEventListener("pointerdown", closeOutside);
    return () => {
      window.removeEventListener("resize", updatePlacement);
      window.removeEventListener("scroll", updatePlacement, true);
      document.removeEventListener("pointerdown", closeOutside);
    };
  }, [open, options.length]);

  const show = (index = options.findIndex((option) => option.value === value)) => {
    setActive(Math.max(0, index));
    setOpen(true);
  };
  const choose = (index: number) => {
    const option = options[index];
    if (!option) return;
    onChange(option.value);
    setActive(index);
    setOpen(false);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "Escape" && open) { event.preventDefault(); setOpen(false); return; }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!open) show();
      else setActive((index) => (index + (event.key === "ArrowDown" ? 1 : -1) + options.length) % options.length);
    } else if (event.key === "Home" && open) { event.preventDefault(); setActive(0); }
    else if (event.key === "End" && open) { event.preventDefault(); setActive(options.length - 1); }
    else if ((event.key === "Enter" || event.key === " ") && open) { event.preventDefault(); choose(active); }
    else if ((event.key === "Enter" || event.key === " ") && !open) { event.preventDefault(); show(); }
  };

  useEffect(() => {
    if (open) menu.current?.children[active]?.scrollIntoView?.({ block: "nearest" });
  }, [active, open]);

  return <>
    <button
      ref={trigger}
      id={id}
      type="button"
      role="combobox"
      aria-label={label}
      aria-haspopup="listbox"
      aria-expanded={open}
      aria-controls={listId}
      aria-activedescendant={open ? `${listId}-${active}` : undefined}
      className={cx(control, className)}
      onClick={() => open ? setOpen(false) : show()}
      onBlur={() => setOpen(false)}
      onKeyDown={onKeyDown}
    >{options.find((option) => option.value === value)?.label ?? value}</button>
    {open && placement && createPortal(
      <div ref={menu} id={listId} className="select-menu fixed z-120 overflow-auto p-[3px] border border-line-strong rounded-md bg-surface shadow-pop" role="listbox" aria-label={label} style={{ top: placement.top ?? "auto", bottom: placement.bottom ?? "auto", left: placement.left, width: placement.width, maxHeight: placement.maxHeight }}>
        {options.map((option, index) => <div
          key={option.value}
          id={`${listId}-${index}`}
          role="option"
          aria-selected={option.value === value}
          className={selectOption(index === active)}
          onMouseEnter={() => setActive(index)}
          onMouseDown={(event) => { event.preventDefault(); choose(index); }}
        ><span className="inline-flex items-center w-5 flex-[0_0_20px] text-accent-text [&_.icon]:size-[13px]" aria-hidden="true">{option.value === value && <Icon name="check" />}</span><span>{option.label}</span></div>)}
      </div>,
      document.body,
    )}
  </>;
}
