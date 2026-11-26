import { useEffect, useRef, useState, type ReactNode } from "react";
import { getName } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { useDismiss } from "./ContextMenu";
import { contextMenu } from "./ui";
import icon from "../src-tauri/icons/icon.png";
import devIcon from "../src-tauri/icons/icon-dev.png";

const appWindow = getCurrentWindow();

/** The app's name from Tauri: "Odx", or "Odx Dev" for a development build, which runs beside a release app
 * and must look different (name and icon). */
function useAppName() {
  const [name, setName] = useState("Odx");
  useEffect(() => { void getName().then(setName, () => undefined); }, []);
  return name;
}

/** Edges are 5px strips that leave the 12px corners to the corner handles. */
const resizeHandles = {
  North: "top-0 inset-x-3 h-[5px] cursor-ns-resize",
  South: "bottom-0 inset-x-3 h-[5px] cursor-ns-resize",
  East: "right-0 inset-y-3 w-[5px] cursor-ew-resize",
  West: "left-0 inset-y-3 w-[5px] cursor-ew-resize",
  NorthEast: "top-0 right-0 size-3 cursor-nesw-resize",
  NorthWest: "top-0 left-0 size-3 cursor-nwse-resize",
  SouthEast: "bottom-0 right-0 size-3 cursor-nwse-resize",
  SouthWest: "bottom-0 left-0 size-3 cursor-nesw-resize",
} as const;
const resizeDirections = Object.keys(resizeHandles) as (keyof typeof resizeHandles)[];

/** Rounded corners and the border only make sense on a floating window. Wayland does not always send a resize event to Tauri on maximize, so the browser's own resize event also triggers the check. */
function useWindowChromeState() {
  const [maximized, setMaximized] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const [focused, setFocused] = useState(true);
  useEffect(() => {
    let active = true;
    const sync = () => {
      Promise.all([appWindow.isMaximized(), appWindow.isFullscreen()]).then(([max, full]) => {
        if (!active) return;
        setMaximized(max);
        setFullscreen(full);
        document.documentElement.toggleAttribute("data-window-filled", max || full);
      }, () => undefined);
    };
    const setFocusState = (next: boolean) => {
      if (!active) return;
      setFocused(next);
      document.documentElement.toggleAttribute("data-window-focused", next);
    };
    sync();
    void Promise.resolve().then(() => appWindow.isFocused?.()).then((f) => {
      if (typeof f === "boolean") setFocusState(f);
      else setFocusState(document.hasFocus());
    }, () => setFocusState(document.hasFocus()));
    const onFocus = () => setFocusState(true);
    const onBlur = () => setFocusState(false);
    window.addEventListener("resize", sync);
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    const resizeListener = appWindow.onResized(sync);
    const focusListener = typeof appWindow.onFocusChanged === "function"
      ? appWindow.onFocusChanged(({ payload }) => setFocusState(payload))
      : Promise.resolve(() => undefined);
    return () => {
      active = false;
      window.removeEventListener("resize", sync);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      void resizeListener.then((unlisten) => unlisten(), () => undefined);
      void focusListener.then((unlisten) => unlisten(), () => undefined);
    };
  }, []);
  return { maximized, fullscreen, focused };
}

const ZOOM_MIN = 85;
const ZOOM_MAX = 125;
const ZOOM_STEP = 5;

function applyZoom(pct: number) {
  const clamped = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(pct)));
  (document.documentElement.style as CSSStyleDeclaration & { zoom?: string }).zoom = clamped === 100 ? "" : `${clamped}%`;
  try { localStorage.setItem("odx.zoom", String(clamped)); } catch { /* optional */ }
  return clamped;
}

/** Global native window shortcuts: F11 fullscreen and Ctrl+/-/0 UI zoom. */
function useWindowShortcuts() {
  useEffect(() => {
    try {
      const saved = Number(localStorage.getItem("odx.zoom"));
      if (Number.isFinite(saved) && saved >= ZOOM_MIN && saved <= ZOOM_MAX && saved !== 100) applyZoom(saved);
    } catch { /* optional */ }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "F11" && !event.ctrlKey && !event.altKey && !event.metaKey) {
        event.preventDefault();
        void appWindow.isFullscreen().then((full) => appWindow.setFullscreen(!full)).catch(() => undefined);
        return;
      }
      if (!event.ctrlKey || event.altKey || event.metaKey) return;
      if (event.key === "=" || event.key === "+") {
        event.preventDefault();
        const current = Number(localStorage.getItem("odx.zoom") || 100) || 100;
        applyZoom(current + ZOOM_STEP);
      } else if (event.key === "-") {
        event.preventDefault();
        const current = Number(localStorage.getItem("odx.zoom") || 100) || 100;
        applyZoom(current - ZOOM_STEP);
      } else if (event.key === "0") {
        event.preventDefault();
        applyZoom(100);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

let revealed = false;
/** The window starts hidden so a transparent, unpainted WebView never shows as a black box; reveal it once after the first paint. */
function useRevealWindow() {
  useEffect(() => {
    if (revealed) return;
    revealed = true;
    // window_present shows, maximizes a window restored maximized, and asks KWin on Wayland for focus, which it refuses apps.
    void invoke("window_present").catch(() => appWindow.show()).catch(() => undefined);
  }, []);
}

function WindowResizeHandles() {
  const [enabled, setEnabled] = useState(false);
  useEffect(() => {
    let active = true;
    let revision = 0;
    const refresh = async () => {
      const current = ++revision;
      try {
        const [maximized, fullscreen, resizable] = await Promise.all([
          appWindow.isMaximized(), appWindow.isFullscreen(), appWindow.isResizable(),
        ]);
        if (active && current === revision) setEnabled(resizable && !maximized && !fullscreen);
      } catch {
        if (active && current === revision) setEnabled(false);
      }
    };
    const listener = appWindow.onResized(() => { void refresh(); });
    const onResize = () => { void refresh(); };
    window.addEventListener("resize", onResize);
    void listener.then(() => { if (active) void refresh(); }, () => undefined);
    return () => {
      active = false;
      window.removeEventListener("resize", onResize);
      void listener.then((unlisten) => unlisten(), () => undefined);
    };
  }, []);

  if (!enabled) return null;
  return <>{resizeDirections.map((direction) => <div
    key={direction}
    className={`fixed z-40 touch-none select-none ${resizeHandles[direction]}`}
    data-direction={direction}
    aria-hidden="true"
    onPointerDown={(event) => {
      if (event.button !== 0 || !event.isPrimary) return;
      event.preventDefault();
      event.stopPropagation();
      void appWindow.startResizeDragging(direction).catch(() => undefined);
    }}
  />)}</>;
}

// Window buttons: full bar height; their glyphs are hairline 12px strokes.
const idle = "text-fg-2 hover:bg-hover hover:text-fg [&_svg]:stroke-[1.15]";
const control = "w-11 h-full grid place-items-center cursor-pointer [&_svg]:size-3 [&_svg]:fill-none [&_svg]:stroke-current [&_svg]:[stroke-linecap:round] [&_svg]:[stroke-linejoin:round]";

/** `title`: what this window shows, after the ODX mark (table windows). */
export function WindowTitleBar({ children, title }: { children?: ReactNode; title?: string }) {
  const name = useAppName();
  const { t } = useTranslation();
  const { maximized, fullscreen, focused } = useWindowChromeState();
  useRevealWindow();
  useWindowShortcuts();
  // Hidden where the desktop gives no way to stay on top (Wayland outside KDE).
  const [canKeepAbove, setCanKeepAbove] = useState(false);
  const [above, setAbove] = useState(false);
  const [menuPos, setMenuPos] = useState<{ x: number; y: number } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  useDismiss(!!menuPos, () => menuRef.current, () => setMenuPos(null));
  useEffect(() => { void Promise.resolve().then(() => invoke<boolean>("window_keep_above_supported")).then(setCanKeepAbove, () => undefined); }, []);
  const toggleKeepAbove = () => void invoke("window_keep_above", { above: !above }).then(() => setAbove(!above), () => undefined);
  const toggleFullscreen = () => void appWindow.setFullscreen(!fullscreen).catch(() => undefined);
  const maxLabel = maximized ? t("window.restore") : t("window.maximize");
  return <><WindowResizeHandles /><div
    className="h-[34px] flex-[0_0_34px] flex items-center pl-3 bg-chrome border-b border-line select-none cursor-default"
    onMouseDown={(event) => {
      if (event.button === 0 && !(event.target as HTMLElement).closest("button")) {
        event.preventDefault();
        void appWindow.startDragging();
      }
    }}
    onContextMenu={(event) => {
      if ((event.target as HTMLElement).closest("button")) return;
      event.preventDefault();
      setMenuPos({ x: Math.min(event.clientX, window.innerWidth - 200), y: Math.min(event.clientY, window.innerHeight - 180) });
    }}
    onDoubleClick={(event) => { if (!(event.target as HTMLElement).closest("button")) void appWindow.toggleMaximize(); }}
  >
    <div className={`flex items-center gap-2 text-fg-2 text-sm font-semibold min-w-0 transition-opacity duration-150 ${focused ? "" : "opacity-60"}`} aria-label={name.toUpperCase()}>
      <img className="block size-[18px] rounded-[5px] object-cover shrink-0" src={name === "Odx" ? icon : devIcon} alt="" />
      <span className="shrink-0">{name.toUpperCase()}</span>
      {title && <span className="pl-2 border-l border-line text-fg-3 font-normal truncate">{title}</span>}
    </div>
    <div className="ml-auto flex items-center pr-1.5 [&>button]:size-[26px]">{children}</div>
    <div className={`h-full flex transition-opacity duration-150 ${focused ? "" : "opacity-65"}`} onMouseDown={(event) => event.stopPropagation()} onDoubleClick={(event) => event.stopPropagation()}>
      {canKeepAbove && <button type="button" className={`${control} ${above ? "bg-accent-soft text-accent-text [&_svg]:stroke-[1.8]" : idle}`} aria-pressed={above} aria-label={t("window.keepAbove")} data-tip={t(above ? "window.stopKeepAbove" : "window.keepAbove")}
        onClick={toggleKeepAbove}>
        <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M3 7.5 6 4.5l3 3" /></svg>
      </button>}
      <button type="button" className={`${control} ${idle}`} aria-label={t("window.minimize")} data-tip={t("window.minimize")} onClick={() => void appWindow.minimize()}>
        <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2 6h8" /></svg>
      </button>
      <button type="button" className={`${control} ${idle}`} aria-label={maxLabel} data-tip={maxLabel} onClick={() => void appWindow.toggleMaximize()}>
        {maximized
          ? <svg viewBox="0 0 12 12" aria-hidden="true"><rect x="2" y="3.75" width="6.25" height="6.25" rx="0.6" /><path d="M4.25 3.75V2.6a.6.6 0 0 1 .6-.6h4.55a.6.6 0 0 1 .6.6v4.55a.6.6 0 0 1-.6.6H8.25" /></svg>
          : <svg viewBox="0 0 12 12" aria-hidden="true"><rect x="2.25" y="2.25" width="7.5" height="7.5" rx="0.6" /></svg>}
      </button>
      <button className={`${control} text-fg-2 hover:bg-[#c42b3c] hover:text-white [&_svg]:stroke-[1.15]`} type="button" aria-label={t("common.close")} data-tip={t("common.close")} onClick={() => void appWindow.close()}>
        <svg viewBox="0 0 12 12" aria-hidden="true"><path d="m3 3 6 6M9 3 3 9" /></svg>
      </button>
    </div>
  </div>
  {menuPos && <div ref={menuRef} className={contextMenu} role="menu" aria-label={t("window.menu")} style={{ left: menuPos.x, top: menuPos.y }}>
    <button type="button" role="menuitem" onClick={() => { setMenuPos(null); void appWindow.minimize(); }}><span>{t("window.minimize")}</span></button>
    <button type="button" role="menuitem" onClick={() => { setMenuPos(null); void appWindow.toggleMaximize(); }}><span>{maxLabel}</span></button>
    {canKeepAbove && <button type="button" role="menuitem" onClick={() => { setMenuPos(null); toggleKeepAbove(); }}><span>{t(above ? "window.stopKeepAbove" : "window.keepAbove")}</span></button>}
    <button type="button" role="menuitem" onClick={() => { setMenuPos(null); toggleFullscreen(); }}><span>{t(fullscreen ? "window.exitFullscreen" : "window.fullscreen")}</span><kbd>F11</kbd></button>
    <hr />
    <button type="button" role="menuitem" className="text-danger" onClick={() => { setMenuPos(null); void appWindow.close(); }}><span>{t("common.close")}</span></button>
  </div>}</>;
}
