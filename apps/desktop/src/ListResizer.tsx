import { useState } from "react";

const LIST_WIDTH = { min: 220, max: 520, initial: 300 };
const clampWidth = (width: number) => Math.round(Math.min(LIST_WIDTH.max, Math.max(LIST_WIDTH.min, width)));

/** List width, remembered on this computer under `storageKey`. */
export function useListWidth(storageKey: string): [number, (width: number, persist?: boolean) => void] {
  const [width, set] = useState(() => {
    try { const stored = Number(localStorage.getItem(storageKey)); return stored ? clampWidth(stored) : LIST_WIDTH.initial; } catch { return LIST_WIDTH.initial; }
  });
  return [width, (next, persist = true) => {
    const value = clampWidth(next);
    set(value);
    if (persist) try { localStorage.setItem(storageKey, String(value)); } catch { /* Optional preference. */ }
  }];
}

/** Drag, arrow keys or double-click (reset) to resize the list. */
export function ListResizer({ width, setWidth, label }: { width: number; setWidth: (width: number, persist?: boolean) => void; label: string }) {
  return <div className="absolute z-3 inset-y-0 -right-1 w-[7px] cursor-col-resize touch-none after:absolute after:inset-y-0 after:left-[3px] after:w-px after:bg-transparent after:transition-colors after:duration-120 focus-visible:outline-none focus-visible:after:left-0.5 focus-visible:after:w-[3px] focus-visible:after:bg-accent max-sm:hidden" role="separator" aria-orientation="vertical" tabIndex={0}
    aria-label={label} aria-valuemin={LIST_WIDTH.min} aria-valuemax={LIST_WIDTH.max} aria-valuenow={width}
    onPointerDown={(e) => {
      e.preventDefault();
      const handle = e.currentTarget;
      const start = e.clientX - width;
      handle.setPointerCapture(e.pointerId);
      let last = width;
      // Save once on release, not on every pointer move.
      const move = (event: PointerEvent) => { last = event.clientX - start; setWidth(last, false); };
      const stop = () => { setWidth(last); handle.removeEventListener("pointermove", move); handle.removeEventListener("pointerup", stop); handle.removeEventListener("pointercancel", stop); };
      handle.addEventListener("pointermove", move);
      handle.addEventListener("pointerup", stop);
      handle.addEventListener("pointercancel", stop);
    }}
    onDoubleClick={() => setWidth(LIST_WIDTH.initial)}
    onKeyDown={(e) => {
      const step = e.shiftKey ? 64 : 16;
      if (e.key === "ArrowLeft") setWidth(width - step);
      else if (e.key === "ArrowRight") setWidth(width + step);
      else if (e.key === "Home") setWidth(LIST_WIDTH.min);
      else if (e.key === "End") setWidth(LIST_WIDTH.max);
      else return;
      e.preventDefault();
    }} />;
}
