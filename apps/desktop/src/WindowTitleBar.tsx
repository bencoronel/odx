import { useEffect, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";
import odxIcon from "../src-tauri/icons/icon.png";

const appWindow = getCurrentWindow();

const resizeDirections = ["North", "South", "East", "West", "NorthEast", "NorthWest", "SouthEast", "SouthWest"] as const;

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
    void listener.then(() => { if (active) void refresh(); }, () => undefined);
    return () => {
      active = false;
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

export function WindowTitleBar({ children }: { children?: ReactNode }) {
  const { t } = useTranslation();
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
    <div className="window-title" aria-label="ODX">
      <img className="window-mark" src={odxIcon} alt="" />
      <span>ODX</span>
    </div>
    <div className="titlebar-tools">{children}</div>
    <div className="window-controls" onMouseDown={(event) => event.stopPropagation()} onDoubleClick={(event) => event.stopPropagation()}>
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
