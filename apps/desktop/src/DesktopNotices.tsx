import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Icon } from "./Icon";
import { UpdateNotice } from "./Updates";
import { useTranslation } from "react-i18next";

export type DesktopStatus = { tray: boolean; tray_error?: string | null; notifications_error?: string | null };

/** Report unavailable tray or notifications. */
export function DesktopNoticeList({ status }: { status: DesktopStatus | null }) {
  const { t } = useTranslation();
  if (!status) return null;
  const notices: string[] = [];
  if (!status.tray) notices.push(t("desktop.trayUnavailable"));
  if (status.notifications_error) notices.push(t("desktop.notificationsUnavailable"));
  if (!notices.length) return null;
  return <div className="desktop-notices">
    {notices.map((text) => <div key={text} className="banner info page-banner" role="status"><Icon name="alert" /><span className="grow">{text}</span></div>)}
  </div>;
}

export default function DesktopNotices() {
  const [status, setStatus] = useState<DesktopStatus | null>(null);
  useEffect(() => {
    const load = () => { void invoke<DesktopStatus>("desktop_status").then(setStatus).catch(() => undefined); };
    load();
    const stop = listen("desktop-status-changed", load);
    return () => { void stop.then((unlisten) => unlisten()); };
  }, []);
  return <><DesktopNoticeList status={status} /><UpdateNotice /></>;
}
