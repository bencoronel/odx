import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Icon, type IconName } from "./Icon";
import { placeInWindow } from "./ContextMenu";

/** A menu entry; `null` is a separator. */
export type MenuItem = { label: string; icon?: IconName; keys?: string; danger?: boolean; disabled?: boolean; run: () => void } | null;

/** ArrowDown and ArrowUp move the focus through `items`, round. */
function cycleFocus(event: KeyboardEvent, items: HTMLElement[]) {
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  event.preventDefault();
  const index = items.indexOf(document.activeElement as HTMLElement);
  items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
}

export function MenuItems({ items, close }: { items: MenuItem[]; close: () => void }) {
  return <>{items.map((item, i) => item
    ? <button key={item.label} type="button" role="menuitem" disabled={item.disabled} className={item.danger ? "text-danger" : undefined} onClick={() => { close(); item.run(); }}>
      {item.icon && <Icon name={item.icon} />}<span>{item.label}</span>{item.keys && <kbd>{item.keys}</kbd>}
    </button>
    : <hr key={i} />)}</>;
}

/** A menu at a point (a right click, or a button's corner with `alignEnd`), kept inside the window and focused on its first
 * field or item. Arrows move; Escape, Tab, a press outside, scrolling the page or leaving the window close it. A press on
 * the open menu's own button is left to that button, which toggles it. `onKey` handles keys before the arrows do. */
export function FloatingMenu({ at, label, close, className = "context-menu", alignEnd, onKey, children }: {
  at: { left: number; top: number }; label: string; close: () => void; className?: string; alignEnd?: boolean;
  onKey?: (event: KeyboardEvent<HTMLDivElement>) => void; children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = ref.current!;
    placeInWindow(el, alignEnd ? at.left - el.offsetWidth : at.left, at.top);
    el.querySelector<HTMLElement>("input, [role=menuitem]:not(:disabled)")?.focus();
  }, [at.left, at.top, alignEnd]);
  useEffect(() => {
    const inside = (event: Event) => !!ref.current?.contains(event.target as Node);
    const outside = (event: Event) => { if (!inside(event) && !(event.target as Element).closest?.("[aria-haspopup=menu][aria-expanded=true]")) close(); };
    // The menu's own list may scroll; the page scrolling away from it closes it.
    const scrolled = (event: Event) => { if (!inside(event)) close(); };
    // Escape with the focus elsewhere; inside the menu its own handler takes it first.
    const escape = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") close(); };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("keydown", escape);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("scroll", scrolled, true);
      window.removeEventListener("keydown", escape);
      window.removeEventListener("blur", close);
    };
  }, [close]);
  return <div ref={ref} className={className} role="menu" aria-label={label} style={{ left: at.left, top: at.top }} onKeyDown={(event) => {
    if (event.key === "Escape" || event.key === "Tab") { event.preventDefault(); event.stopPropagation(); close(); return; }
    onKey?.(event);
    if (!event.defaultPrevented) cycleFocus(event, [...ref.current!.querySelectorAll<HTMLElement>("[role=menuitem]:not(:disabled)")]);
  }}>{children}</div>;
}

/** A "⋯" button with a menu under its right edge, for actions not used every day. */
export function MoreMenu({ label, items, disabled }: { label: string; items: MenuItem[]; disabled?: boolean }) {
  const [at, setAt] = useState<{ left: number; top: number } | null>(null);
  const button = useRef<HTMLButtonElement>(null);
  const close = useCallback(() => setAt(null), []);
  return <>
    <button ref={button} type="button" className="btn sm icon-only" disabled={disabled} aria-haspopup="menu" aria-expanded={!!at} aria-label={label} data-tip={at ? undefined : label}
      onClick={() => { const rect = button.current!.getBoundingClientRect(); setAt(at ? null : { left: rect.right, top: rect.bottom + 4 }); }}>⋯</button>
    {at && <FloatingMenu at={at} alignEnd label={label} className="more-menu-list floating" close={close}><MenuItems items={items} close={close} /></FloatingMenu>}
  </>;
}
