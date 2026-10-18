import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

const DELAY_MS = 350;
const GAP = 8;
const MARGIN = 8;

/** Themed `data-tip` tooltips: delayed on hover, immediate on focus, constrained to the window. Replaces native `title`. */
export function TooltipLayer() {
  const [tip, setTip] = useState<{ text: string; anchor: DOMRect } | null>(null);
  const [pos, setPos] = useState<{ left: number; top?: number; bottom?: number } | null>(null);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let timer = 0;
    let current: Element | null = null;
    const hide = () => { window.clearTimeout(timer); current = null; setTip(null); };
    const show = (el: Element, delay: number) => {
      const text = el.getAttribute("data-tip");
      if (!text || el === current) return;
      window.clearTimeout(timer);
      current = el;
      timer = window.setTimeout(() => { if (current === el && el.isConnected) setTip({ text, anchor: el.getBoundingClientRect() }); }, delay);
    };
    const over = (e: PointerEvent) => {
      const el = (e.target as Element | null)?.closest?.("[data-tip]");
      if (el) show(el, DELAY_MS); else if (current) hide();
    };
    const focus = (e: FocusEvent) => {
      const el = (e.target as Element | null)?.closest?.("[data-tip]");
      if (el && (e.target as HTMLElement).matches(":focus-visible")) show(el, 0);
    };
    document.addEventListener("pointerover", over);
    document.addEventListener("focusin", focus);
    document.addEventListener("focusout", hide);
    document.addEventListener("pointerdown", hide, true);
    document.addEventListener("keydown", hide, true);
    window.addEventListener("scroll", hide, true);
    window.addEventListener("blur", hide);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("pointerover", over);
      document.removeEventListener("focusin", focus);
      document.removeEventListener("focusout", hide);
      document.removeEventListener("pointerdown", hide, true);
      document.removeEventListener("keydown", hide, true);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("blur", hide);
    };
  }, []);

  // Center below; move above if needed and stay within the window.
  useLayoutEffect(() => {
    if (!tip || !box.current) { setPos(null); return; }
    const { width, height } = box.current.getBoundingClientRect();
    const a = tip.anchor;
    const left = Math.min(Math.max(MARGIN, a.left + a.width / 2 - width / 2), window.innerWidth - MARGIN - width);
    // Anchor above by the bottom edge to avoid relying on exact height.
    if (a.bottom + GAP + height > window.innerHeight - MARGIN) setPos({ left, bottom: window.innerHeight - a.top + GAP });
    else setPos({ left, top: a.bottom + GAP });
  }, [tip]);

  if (!tip) return null;
  return createPortal(
    <div ref={box} className="tooltip floating" role="tooltip" style={pos ? { left: pos.left, top: pos.top ?? "auto", bottom: pos.bottom ?? "auto" } : { left: -9999, top: 0 }}>
      {tip.text.split("\n").map((line, i) => i === 0 ? <strong key={i}>{line}</strong> : <span key={i}>{line}</span>)}
    </div>,
    document.body,
  );
}
