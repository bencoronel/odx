import { useSyncExternalStore, type CSSProperties } from "react";
import { ipc } from "./ipc";
import type { Tone } from "./ui";

/** Theme tones a tag can use; "neutral" is the default look. Any `#rrggbb` hex string is also supported. */
export const TAG_COLORS = ["neutral", "info", "ok", "warn", "danger", "accent"] as const;
export type NamedTagColor = typeof TAG_COLORS[number];
export type TagColor = NamedTagColor | `#${string}`;
export const EXTRA_HEX_SWATCHES = ["#14b8a6", "#f97316", "#ec4899", "#6366f1"] as const;
type Colors = Record<string, TagColor>;

export const isHexColor = (value: string): value is `#${string}` => /^#[0-9a-f]{6}$/i.test(value);
const isValidColor = (value: string): value is TagColor => (TAG_COLORS as readonly string[]).includes(value) || isHexColor(value);

const listeners = new Set<() => void>();
let colors: Colors = {};
const publish = (next: Colors) => { colors = next; listeners.forEach((listener) => listener()); };
const valid = (map: Record<string, string>) => Object.fromEntries(
  Object.entries(map)
    .filter(([, color]) => isValidColor(color))
    .map(([tag, color]) => [tag, isHexColor(color) ? color.toLowerCase() : color]),
) as Colors;

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
  const normalized = isHexColor(color) ? color.toLowerCase() as TagColor : color;
  const previous = colors;
  const { [key]: _, ...rest } = colors;
  publish(normalized === "neutral" ? rest : { ...rest, [key]: normalized });
  ipc.setTagColor(tag, normalized === "neutral" ? null : normalized).catch(() => publish(previous));
}
export const tagColor = (all: Colors, tag: string): TagColor => all[tag.toLowerCase()] ?? "neutral";
/** A tag's built-in chip tone; none for neutral or custom hex (use `tagChipStyle` for hex). */
export const tagTone = (all: Colors, tag: string): Tone | undefined => {
  const color = tagColor(all, tag);
  return color === "neutral" || isHexColor(color) ? undefined : color;
};
/** Inline CSS variables for a custom hex tag chip. */
export const tagChipStyle = (all: Colors, tag: string): CSSProperties | undefined => {
  const color = tagColor(all, tag);
  if (!isHexColor(color)) return undefined;
  return {
    "--tone": color,
    "--tone-soft": `color-mix(in srgb, ${color} 18%, transparent)`,
    color,
    backgroundColor: `color-mix(in srgb, ${color} 18%, transparent)`,
    borderColor: `color-mix(in srgb, ${color} 32%, transparent)`,
  } as CSSProperties;
};
/** Dot color props for tag lists and filters. */
export const tagDotProps = (all: Colors, tag: string): { className: string; style?: CSSProperties } => {
  const color = tagColor(all, tag);
  if (isHexColor(color)) return { className: "size-2 flex-none rounded-full", style: { backgroundColor: color } };
  return { className: `size-2 flex-none rounded-full tone-${color} ${color === "neutral" ? "bg-line-strong" : "bg-tone"}` };
};
/** Approximate hex value to prefill the native `<input type="color">`. */
const NAMED_HEX: Record<NamedTagColor, string> = {
  neutral: "#8a93a0",
  info: "#5b9bff",
  ok: "#4ac26b",
  warn: "#e3b341",
  danger: "#ff6b63",
  accent: "#7465ee",
};
export const tagColorHex = (color: TagColor): string => isHexColor(color) ? color : NAMED_HEX[color];
