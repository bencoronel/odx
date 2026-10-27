import { useSyncExternalStore } from "react";
import { ipc } from "./ipc";
import type { Tone } from "./ui";

/** Theme tones a tag can use; "neutral" is the default look. */
export const TAG_COLORS = ["neutral", "info", "ok", "warn", "danger", "accent"] as const;
export type TagColor = typeof TAG_COLORS[number];
type Colors = Record<string, TagColor>;

const listeners = new Set<() => void>();
let colors: Colors = {};
const publish = (next: Colors) => { colors = next; listeners.forEach((listener) => listener()); };
const valid = (map: Record<string, string>) => Object.fromEntries(Object.entries(map).filter(([, color]) => TAG_COLORS.includes(color as TagColor))) as Colors;

/** Before colors lived in the vault they were kept per device. */
const LEGACY_KEY = "odx.tagColors";

/** Load colors from the vault, moving any kept on this device into it once. */
export async function loadTagColors() {
  const stored = valid(await ipc.tagColors());
  let legacy: Colors = {};
  try { legacy = valid(JSON.parse(localStorage.getItem(LEGACY_KEY) ?? "{}")); } catch { /* Nothing to move. */ }
  const moved = Object.entries(legacy).filter(([tag]) => !stored[tag]);
  await Promise.all(moved.map(([tag, color]) => ipc.setTagColor(tag, color).catch(() => undefined)));
  try { localStorage.removeItem(LEGACY_KEY); } catch { /* Optional. */ }
  publish({ ...Object.fromEntries(moved), ...stored });
}

/** Colors are per tag name, ignoring case, and saved in the vault. */
export function useTagColors() {
  return useSyncExternalStore((listener) => { listeners.add(listener); return () => listeners.delete(listener); }, () => colors);
}
/** Shows the new color at once and reverts it if the vault refuses. */
export function setTagColor(tag: string, color: TagColor) {
  const key = tag.toLowerCase();
  const previous = colors;
  const { [key]: _, ...rest } = colors;
  publish(color === "neutral" ? rest : { ...rest, [key]: color });
  ipc.setTagColor(tag, color === "neutral" ? null : color).catch(() => publish(previous));
}
export const tagColor = (all: Colors, tag: string): TagColor => all[tag.toLowerCase()] ?? "neutral";
/** A tag's chip tone; none for the default look. */
export const tagTone = (all: Colors, tag: string): Tone | undefined => { const color = tagColor(all, tag); return color === "neutral" ? undefined : color; };
