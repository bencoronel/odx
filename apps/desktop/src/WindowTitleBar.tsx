import { useEffect, useState, type ReactNode } from "react";
import { getName } from "@tauri-apps/api/app";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
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

const resizeDirections = ["North", "South", "East", "West", "NorthEast", "NorthWest", "SouthEast", "SouthWest"] as const;

/** The border only makes sense on a floating window. Wayland does not always send a resize event to Tauri on maximize, so the browser's own resize event also triggers the check. */
function useFilledWindow() {
  useEffect(() => {
    let active = true;
    const sync = () => {
      Promise.all([appWindow.isMaximized(), appWindow.isFullscreen()]).then(([maximized, fullscreen]) => {
        if (active) document.documentElement.toggleAttribute("data-window-filled", maximized || fullscreen);
      }, () => undefined);
    };
    sync();
    window.addEventListener("resize", sync);
    const listener = appWindow.onResized(sync);
    return () => {
      active = false;
      window.removeEventListener("resize", sync);
      void listener.then((unlisten) => unlisten(), () => undefined);
    };
  }, []);
}

let revealed = false;
/** The window starts hidden so an unpainted WebView never shows as an empty box; reveal it once after the first paint. */
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
    className="window-resize-handle"
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

/** `title`: what this window shows, after the ODX mark (table windows). */
export function WindowTitleBar({ children, title }: { children?: ReactNode; title?: string }) {
  const name = useAppName();
  const { t } = useTranslation();
  useFilledWindow();
  useRevealWindow();
  // Hidden where the desktop gives no way to stay on top (Wayland outside KDE).
  const [canKeepAbove, setCanKeepAbove] = useState(false);
  const [above, setAbove] = useState(false);
  useEffect(() => { void Promise.resolve().then(() => invoke<boolean>("window_keep_above_supported")).then(setCanKeepAbove, () => undefined); }, []);
  return <><WindowResizeHandles /><div
    className="window-titlebar"
    onMouseDown={(event) => {
      if (event.button === 0 && !(event.target as HTMLElement).closest("button")) {
        event.preventDefault();
        void appWindow.startDragging();
      }
    }}
    onDoubleClick={(event) => { if (!(event.target as HTMLElement).closest("button")) void appWindow.toggleMaximize(); }}
  >
    <div className="window-title" aria-label={name.toUpperCase()}>
      <img className="window-mark" src={name === "Odx" ? icon : devIcon} alt="" />
      <span>{name.toUpperCase()}</span>
      {title && <span className="window-subtitle">{title}</span>}
    </div>
    <div className="titlebar-tools">{children}</div>
    <div className="window-controls" onMouseDown={(event) => event.stopPropagation()} onDoubleClick={(event) => event.stopPropagation()}>
      {canKeepAbove && <button type="button" className={above ? "on" : undefined} aria-pressed={above} aria-label={t("window.keepAbove")} data-tip={t(above ? "window.stopKeepAbove" : "window.keepAbove")}
        onClick={() => void invoke("window_keep_above", { above: !above }).then(() => setAbove(!above), () => undefined)}>
        <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M3 7.5 6 4.5l3 3" /></svg>
      </button>}
      <button type="button" aria-label={t("window.minimize")} data-tip={t("window.minimize")} onClick={() => void appWindow.minimize()}>
        <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2 6h8" /></svg>
      </button>
      <button type="button" aria-label={t("window.maximizeRestore")} data-tip={t("window.maximizeRestore")} onClick={() => void appWindow.toggleMaximize()}>
        <svg viewBox="0 0 12 12" aria-hidden="true"><rect x="2.25" y="2.25" width="7.5" height="7.5" rx="0.6" /></svg>
      </button>
      <button className="window-close" type="button" aria-label={t("common.close")} data-tip={t("common.close")} onClick={() => void appWindow.close()}>
        <svg viewBox="0 0 12 12" aria-hidden="true"><path d="m3 3 6 6M9 3 3 9" /></svg>
      </button>
    </div>
  </div></>;
}
