import type { CSSProperties } from "react";

export type IconName = "server" | "check" | "clients" | "activity" | "settings" | "lock" | "unlock" | "sun" | "moon" | "refresh" | "arrow" | "shield"
  | "plus" | "search" | "x" | "clock" | "database" | "user" | "globe" | "key" | "plug" | "alert" | "eye" | "eyeOff" | "pencil" | "infinity" | "code" | "trash" | "copy" | "terminal" | "chevron" | "external" | "table" | "rows" | "caretUp" | "caretDown" | "filter";

const paths: Record<IconName, string> = {
  server: "M4 4h16v6H4z M4 14h16v6H4z M8 7h.01 M8 17h.01",
  check: "M9 12l2 2 4-4 M12 3l8 3v6c0 5-8 9-8 9s-8-4-8-9V6z",
  clients: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2 M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8 M17 4a4 4 0 0 1 0 8 M22 21v-2a4 4 0 0 0-3-3.87",
  activity: "M3 12h4l3-8 4 16 3-8h4",
  settings: "M4 6h9 M17 6h3 M4 12h3 M11 12h9 M4 18h11 M19 18h1 M15 4v4 M9 10v4 M17 16v4",
  lock: "M5 11h14v10H5z M8 11V7a4 4 0 0 1 8 0v4",
  unlock: "M5 11h14v10H5z M8 11V7a4 4 0 0 1 7.5-2",
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8 M12 2v2 M12 20v2 M2 12h2 M20 12h2 M5 5l1.5 1.5 M17.5 17.5L19 19 M5 19l1.5-1.5 M17.5 6.5L19 5",
  moon: "M20.5 13A8.5 8.5 0 0 1 11 3.5 8.5 8.5 0 1 0 20.5 13z",
  refresh: "M20 11a8 8 0 0 0-14.7-4.4L4 8 M4 4v4h4 M4 13a8 8 0 0 0 14.7 4.4L20 16 M20 20v-4h-4",
  arrow: "M5 12h14 M14 7l5 5-5 5",
  shield: "M12 3l8 3v6c0 5-8 9-8 9s-8-4-8-9V6z",
  plus: "M12 5v14 M5 12h14",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14 M20 20l-4-4",
  x: "M6 6l12 12 M18 6L6 18",
  filter: "M4 5h16l-6 7.5V19l-4 1.5v-8z",
  clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18 M12 7v5l3 2",
  database: "M12 8c4.4 0 8-1.3 8-3s-3.6-3-8-3-8 1.3-8 3 3.6 3 8 3 M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5 M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3",
  user: "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8 M4 21v-1a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v1",
  globe: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18 M3 12h18 M12 3c2.5 2.5 3.5 5.5 3.5 9s-1 6.5-3.5 9c-2.5-2.5-3.5-5.5-3.5-9s1-6.5 3.5-9",
  key: "M15 14a5 5 0 1 0-4.6-3L3 18.5V21h3v-2h2v-2h2l1.4-1.4A5 5 0 0 0 15 14 M16.5 7.5h.01",
  plug: "M9 2v6 M15 2v6 M6 8h12v3a6 6 0 0 1-12 0z M12 17v5",
  alert: "M12 3l10 18H2z M12 10v4 M12 17.5h.01",
  eye: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12 M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6",
  eyeOff: "M3 3l18 18 M10.6 10.6a2 2 0 0 0 2.8 2.8 M9.9 5.2A10.8 10.8 0 0 1 12 5c6.5 0 10 7 10 7a15.8 15.8 0 0 1-3.1 3.9 M6.2 6.2C3.5 8 2 12 2 12s3.5 7 10 7a10.5 10.5 0 0 0 4-.8",
  pencil: "M4 20h4L19 9l-4-4L4 16z M13.5 6.5l4 4",
  infinity: "M7 16a4 4 0 1 1 0-8c3 0 7 8 10 8a4 4 0 1 0 0-8c-3 0-7 8-10 8",
  code: "M8 8l-5 4 5 4 M16 8l5 4-5 4 M14 4l-4 16",
  external: "M14 4h6v6 M20 4l-9 9 M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5",
  chevron: "M9 6l6 6-6 6",
  terminal: "M4 4h16v16H4z M8 9l3 3-3 3 M13 15h3",
  copy: "M9 9h11v11H9z M5 15H4V4h11v1",
  trash: "M4 7h16 M10 11v6 M14 11v6 M6 7l1 13h10l1-13 M9 7V4h6v3",
  table: "M4 5h16v14H4z M4 10h16 M4 15h16 M10 5v14",
  rows: "M4 4h16v7H4z M4 13h16v7H4z M7 7.5h6 M7 16.5h6",
  caretUp: "M7 14l5-5 5 5",
  caretDown: "M7 10l5 5 5-5",
};

export function Icon({ name, className, style }: { name: IconName; className?: string; style?: CSSProperties }) {
  return <svg className={className ? `icon ${className}` : "icon"} style={style} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>;
}
