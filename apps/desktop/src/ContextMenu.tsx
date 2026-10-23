import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

/** Close a popup on a mouse press outside `inside()`; WebKit does not focus clicked buttons, so blur cannot do this. */
export function useDismiss(open: boolean, inside: () => Element | null | undefined, close: () => void) {
  useEffect(() => {
    if (!open) return;
    const outside = (event: MouseEvent) => { if (!inside()?.contains(event.target as Node)) close(); };
    document.addEventListener("mousedown", outside);
    return () => document.removeEventListener("mousedown", outside);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
}

/** Moves a fixed menu to (x, y), kept 4px inside the window. */
export function placeInWindow(el: HTMLElement, x: number, y: number) {
  el.style.left = `${Math.max(4, Math.min(x, window.innerWidth - el.offsetWidth - 4))}px`;
  el.style.top = `${Math.max(4, Math.min(y, window.innerHeight - el.offsetHeight - 4))}px`;
}

const TEXT_INPUTS = new Set(["", "text", "search", "url", "email", "tel", "password", "number"]);

export type MenuContext = { field: HTMLElement | null; canCut: boolean; canCopy: boolean; canPaste: boolean };

/** What the right-clicked spot allows: an editable field (cut, copy, paste, select all) or selected text (copy). null: no menu. */
export function menuContext(target: EventTarget | null, selection: Selection | null): MenuContext | null {
  if (!(target instanceof Element)) return null;
  const input = target.closest("input");
  const field = input ? (TEXT_INPUTS.has(input.getAttribute("type") ?? "") ? input : null)
    : target.closest<HTMLElement>("textarea, [contenteditable]:not([contenteditable='false'])");
  if (field) {
    const control = field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement ? field : null;
    // Number inputs expose no selection range; assume there may be one.
    const selected = control ? control.selectionStart === null || control.selectionStart !== control.selectionEnd : !!selection && !selection.isCollapsed;
    const writable = !control || (!control.readOnly && !control.disabled);
    const secret = control?.type === "password";
    return { field, canCut: selected && writable && !secret, canCopy: selected && !secret, canPaste: writable };
  }
  if (input || !selection || selection.isCollapsed) return null;
  // intersectsNode, not containsNode: a selection inside the clicked element still counts.
  for (let i = 0; i < selection.rangeCount; i++) {
    if (selection.getRangeAt(i).intersectsNode(target)) return { field: null, canCut: false, canCopy: true, canPaste: false };
  }
  return null;
}

/** Replaces the browser menu (back, forward, reload…) with text actions only; Shift+right-click keeps the native one in dev builds. */
export default function ContextMenu({ dev }: { dev: boolean }) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<(MenuContext & { x: number; y: number }) | null>(null);

  useEffect(() => {
    const open = (event: MouseEvent) => {
      if (dev && event.shiftKey) return;
      event.preventDefault();
      const context = menuContext(event.target, window.getSelection());
      setMenu(context && { ...context, x: event.clientX, y: event.clientY });
    };
    const close = () => setMenu(null);
    const closeOutside = (event: Event) => { if (!ref.current?.contains(event.target as Node)) close(); };
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    window.addEventListener("contextmenu", open);
    window.addEventListener("pointerdown", closeOutside, true);
    window.addEventListener("keydown", closeOnEscape);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("contextmenu", open);
      window.removeEventListener("pointerdown", closeOutside, true);
      window.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, [dev]);

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    if (ref.current && menu) placeInWindow(ref.current, menu.x, menu.y);
  }, [menu]);

  if (!menu) return null;
  const run = (command: "cut" | "copy" | "paste" | "selectAll") => {
    menu.field?.focus();
    document.execCommand(command);
    setMenu(null);
  };
  const item = (command: "cut" | "copy" | "paste" | "selectAll", enabled: boolean, keys: string) =>
    <button key={command} type="button" role="menuitem" disabled={!enabled} onClick={() => run(command)}>
      <span>{t(`contextMenu.${command}`)}</span><kbd>{keys}</kbd>
    </button>;
  // preventDefault on mousedown keeps the field focused and the text selected while clicking an item.
  return <div ref={ref} className="context-menu" role="menu" aria-label={t("contextMenu.label")} style={{ left: menu.x, top: menu.y }} onMouseDown={(event) => event.preventDefault()}>
    {menu.field
      ? [item("cut", menu.canCut, "Ctrl+X"), item("copy", menu.canCopy, "Ctrl+C"), item("paste", menu.canPaste, "Ctrl+V"), <hr key="sep" />, item("selectAll", true, "Ctrl+A")]
      : item("copy", menu.canCopy, "Ctrl+C")}
  </div>;
}
