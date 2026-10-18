import { useEffect, useState } from "react";
import { formatDateTime, formatRelativeTime } from "./i18n/format";

/** Run the countdown clock only while needed. */
export function useNow(active: boolean, intervalMs = 1000): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [active, intervalMs]);
  return now;
}

export function formatRemaining(ms: number, units: { hour: string; minute: string }): string {
  if (ms <= 0) return "0:00";
  const total = Math.ceil(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h} ${units.hour} ${String(m).padStart(2, "0")} ${units.minute}` : `${m}:${s}`;
}

/** Relative time for recent events; date for older events. */
export function relativeTime(iso: string, now = Date.now()): string {
  const elapsed = Math.max(0, (now - Date.parse(iso)) / 1000);
  if (elapsed < 60) return formatRelativeTime(-Math.round(elapsed), "second");
  if (elapsed < 3600) return formatRelativeTime(-Math.round(elapsed / 60), "minute");
  if (elapsed < 86400) return formatRelativeTime(-Math.round(elapsed / 3600), "hour");
  if (elapsed < 30 * 86400) return formatRelativeTime(-Math.round(elapsed / 86400), "day");
  return formatDateTime(new Date(iso), { day: "2-digit", month: "2-digit", year: "numeric" });
}
