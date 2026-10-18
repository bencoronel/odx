import { MODEL_VERBS } from "./commands";

export type SuggestionKind = "command" | "environment" | "model" | "option" | "field";
export type Suggestion = { value: string; detail?: string; translateDetail?: boolean; kind: SuggestionKind };
export type Field = { name: string; label: string; type: string };
export type CompletionContext = {
  envs: { name: string; detail: string }[];
  /** `undefined` means not loaded for the connected environment. */
  models?: string[];
  fields: (model: string) => Field[] | undefined;
  connected: boolean;
};
export type Completion = {
  /** Start position of text replaced on acceptance. */
  from: number;
  items: Suggestion[];
  /** Suffix appended to the accepted suggestion. */
  suffix: string;
  /** Data required to offer suggestions. */
  needs?: { models: true } | { fields: string };
};

export const COMMANDS: { name: string; detail: string; translateDetail: true }[] = [
  { name: "search", detail: "search", translateDetail: true },
  { name: "count", detail: "count", translateDetail: true },
  { name: "read", detail: "read", translateDetail: true },
  { name: "name", detail: "name", translateDetail: true },
  { name: "create", detail: "create", translateDetail: true },
  { name: "write", detail: "write", translateDetail: true },
  { name: "unlink", detail: "unlink", translateDetail: true },
  { name: "call", detail: "call", translateDetail: true },
  { name: "\\c", detail: "connect", translateDetail: true },
  { name: "\\l", detail: "list", translateDetail: true },
  { name: "\\d", detail: "fields", translateDetail: true },
  { name: "\\dm", detail: "models", translateDetail: true },
  { name: "\\x", detail: "expanded", translateDetail: true },
  { name: "\\timing", detail: "timing", translateDetail: true },
  { name: "\\history", detail: "history", translateDetail: true },
  { name: "\\next", detail: "next", translateDetail: true },
  { name: "\\context", detail: "context", translateDetail: true },
  { name: "\\clear", detail: "clear", translateDetail: true },
  { name: "\\q", detail: "disconnect", translateDetail: true },
  { name: "\\?", detail: "help", translateDetail: true },
];
const OPTIONS: Record<string, { name: string; detail: string; translateDetail: true }[]> = {
  search: [
    { name: "-d", detail: "domain", translateDetail: true },
    { name: "-f", detail: "fields", translateDetail: true },
    { name: "-l", detail: "limit", translateDetail: true },
    { name: "-o", detail: "offset", translateDetail: true },
    { name: "--order", detail: "order", translateDetail: true },
  ],
  count: [{ name: "-d", detail: "domain", translateDetail: true }],
  read: [{ name: "-f", detail: "fields", translateDetail: true }],
  name: [{ name: "-l", detail: "limit", translateDetail: true }],
};
const VALUE_OPTIONS = ["-d", "-f", "-l", "-o", "--order"];

/** Rank prefix matches before substring matches. */
function rank<T>(items: T[], key: (t: T) => string, partial: string): T[] {
  const p = partial.toLowerCase();
  const starts = items.filter((i) => key(i).toLowerCase().startsWith(p));
  const contains = p ? items.filter((i) => !key(i).toLowerCase().startsWith(p) && key(i).toLowerCase().includes(p)) : [];
  return [...starts, ...contains];
}

export function completions(input: string, ctx: CompletionContext): Completion | null {
  // Environment names after \c may contain spaces.
  const connect = /^\\c\s+/.exec(input);
  if (connect) {
    const from = connect[0].length;
    const items = rank(ctx.envs, (e) => e.name, input.slice(from)).map((e) => ({ value: e.name, detail: e.detail, kind: "environment" as const }));
    return { from, items, suffix: "" };
  }
  const from = input.search(/\S*$/);
  const partial = input.slice(from);
  const before = input.slice(0, from).trim().split(/\s+/).filter(Boolean);
  if (!before.length) {
    const items = rank(COMMANDS, (c) => c.name, partial).map((c) => ({ value: c.name, detail: c.detail, translateDetail: c.translateDetail, kind: "command" as const }));
    return { from, items, suffix: " " };
  }
  const [verb] = before;
  if (!ctx.connected) return null;
  if (before.length === 1 && MODEL_VERBS.includes(verb)) {
    if (!ctx.models) return { from, items: [], suffix: " ", needs: { models: true } };
    return { from, items: rank(ctx.models, (m) => m, partial).map((m) => ({ value: m, kind: "model" as const })), suffix: " " };
  }
  const model = before[1];
  const previous = before[before.length - 1];
  const fieldList = previous === "-f" || (verb === "\\d" && before.length === 2);
  if (fieldList && model) {
    const fields = ctx.fields(model);
    const comma = previous === "-f" ? partial.lastIndexOf(",") + 1 : 0;
    if (!fields) return { from: from + comma, items: [], suffix: "", needs: { fields: model } };
    const taken = partial.slice(0, comma).split(",");
    const items = rank(fields.filter((f) => !taken.includes(f.name)), (f) => f.name, partial.slice(comma))
      .map((f) => ({ value: f.name, detail: `${f.label} · ${f.type}`, kind: "field" as const }));
    return { from: from + comma, items, suffix: "" };
  }
  if (OPTIONS[verb] && model && !VALUE_OPTIONS.includes(previous) && (partial === "" || partial.startsWith("-"))) {
    const used = new Set(before);
    const items = rank(OPTIONS[verb].filter((o) => !used.has(o.name)), (o) => o.name, partial)
      .map((o) => ({ value: o.name, detail: o.detail, translateDetail: o.translateDetail, kind: "option" as const }));
    return { from, items, suffix: " " };
  }
  return null;
}
