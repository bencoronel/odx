import { MODEL_VERBS } from "./commands";

export type SuggestionKind = "command" | "model" | "option" | "field" | "operator" | "value";
export type Suggestion = { value: string; detail?: string; translateDetail?: boolean; kind: SuggestionKind };
export type Field = { name: string; label: string; type: string; relation?: string; selection?: [string, string][] };
export type CompletionContext = {
  /** `undefined` means not loaded for the connected environment. */
  models?: string[];
  fields: (model: string) => Field[] | undefined;
  connected: boolean;
  /** Saved aliases (name → command), offered as commands. */
  aliases?: Record<string, string>;
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
  { name: "read_group", detail: "readGroup", translateDetail: true },
  { name: "name", detail: "name", translateDetail: true },
  { name: "ref", detail: "ref", translateDetail: true },
  { name: "metadata", detail: "metadata", translateDetail: true },
  { name: "whoami", detail: "whoami", translateDetail: true },
  { name: "cam", detail: "access", translateDetail: true },
  { name: "uhg", detail: "hasGroup", translateDetail: true },
  { name: "create", detail: "create", translateDetail: true },
  { name: "write", detail: "write", translateDetail: true },
  { name: "unlink", detail: "unlink", translateDetail: true },
  { name: "call", detail: "call", translateDetail: true },
  { name: "\\l", detail: "list", translateDetail: true },
  { name: "\\d", detail: "fields", translateDetail: true },
  { name: "\\dm", detail: "models", translateDetail: true },
  { name: "\\x", detail: "expanded", translateDetail: true },
  { name: "\\timing", detail: "timing", translateDetail: true },
  { name: "\\history", detail: "history", translateDetail: true },
  { name: "\\next", detail: "next", translateDetail: true },
  { name: "\\prev", detail: "prev", translateDetail: true },
  { name: "\\grep", detail: "grep", translateDetail: true },
  { name: "\\cols", detail: "cols", translateDetail: true },
  { name: "\\set", detail: "set", translateDetail: true },
  { name: "\\alias", detail: "alias", translateDetail: true },
  { name: "\\context", detail: "context", translateDetail: true },
  { name: "\\clear", detail: "clear", translateDetail: true },
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
  read_group: [
    { name: "-d", detail: "domain", translateDetail: true },
    { name: "-f", detail: "aggregates", translateDetail: true },
    { name: "-l", detail: "limit", translateDetail: true },
    { name: "--order", detail: "order", translateDetail: true },
  ],
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

/** Open bracket inside a domain or a values dict; `values` holds closed strings per item. */
type Frame = { open: string; at: number; item: number; colon: boolean; values: (string | undefined)[] };

/** Where the cursor (end of input) sits inside brackets and strings. */
function scan(input: string) {
  const stack: Frame[] = [];
  let quote: { char: string; at: number } | null = null;
  let last = "";
  for (let i = 0; i < input.length; i++) {
    const c = input[i];
    if (quote) {
      if (c === "\\") { i++; continue; }
      if (c === quote.char) {
        last = input.slice(quote.at + 1, i);
        const top = stack[stack.length - 1];
        if (top) top.values[top.item] = last;
        quote = null;
      }
      continue;
    }
    if (c === "'" || c === '"') { quote = { char: c, at: i }; continue; }
    if ("[({".includes(c)) stack.push({ open: c, at: i, item: 0, colon: false, values: [] });
    else if ("])}".includes(c)) stack.pop();
    else if (c === "," && stack.length) { const top = stack[stack.length - 1]; top.item++; top.colon = false; }
    else if (c === ":" && stack.length) stack[stack.length - 1].colon = true;
    else if (/\s/.test(c) && !stack.length) last = "";
  }
  return { stack, quote };
}

const OPERATORS: Record<string, string[]> = {
  text: ["ilike", "=", "!=", "not ilike", "like", "=ilike", "in", "not in"],
  number: ["=", "!=", ">", ">=", "<", "<=", "in", "not in"],
  relation: ["=", "!=", "in", "not in", "ilike", "child_of", "parent_of"],
  plain: ["=", "!="],
  other: ["=", "!=", "in", "not in"],
};
function operatorsFor(type?: string) {
  if (!type) return OPERATORS.other;
  if (["char", "text", "html"].includes(type)) return OPERATORS.text;
  if (["integer", "float", "monetary", "date", "datetime"].includes(type)) return OPERATORS.number;
  if (["many2one", "many2many", "one2many"].includes(type)) return OPERATORS.relation;
  if (type === "boolean") return OPERATORS.plain;
  return OPERATORS.other;
}
/** Values worth offering: selection keys and booleans. */
function valuesFor(field: Field | undefined, quoted: boolean): Suggestion[] {
  if (field?.type === "selection" && field.selection) return field.selection.map(([value, label]) => ({ value: quoted ? value : `'${value}'`, detail: label, kind: "value" }));
  if (field?.type === "boolean" && !quoted) return ["True", "False"].map((value) => ({ value, kind: "value" }));
  return [];
}

/** Field names, operators and values inside `-d [...]` and create/write `{...}`. */
function structured(input: string, ctx: CompletionContext): Completion | null {
  const { stack, quote } = scan(input);
  if (!stack.length) return null;
  const words = input.slice(0, stack[0].at).trim().split(/\s+/);
  const [verb, model] = words;
  const domain = words[words.length - 1] === "-d" && stack[0].open === "[";
  const values = (verb === "create" || verb === "write") && stack[0].open === "{" && stack.length === 1;
  if (!model || (!domain && !values) || !ctx.connected) return null;
  // Partial text: after the open quote, or after the last separator when unquoted.
  const from = quote ? quote.at + 1 : input.length - (/[^\s,([{:]*$/.exec(input)?.[0].length ?? 0);
  const partial = input.slice(from);
  const q = quote?.char ?? "'";
  const fields = ctx.fields(model);
  const wrap = (v: string) => (quote ? v : `${q}${v}${q}`);
  const byName = (name?: string) => fields?.find((f) => f.name === name);
  const fieldItems = () => fields
    ? rank(fields, (f) => f.name, partial).map((f) => ({ value: wrap(f.name), detail: `${f.label} · ${f.type}`, kind: "field" as const }))
    : null;
  if (domain) {
    const tuple = stack[1];
    if (stack.length !== 2 || !tuple || tuple.open === "{") return null;
    if (tuple.item === 0) {
      const items = fieldItems();
      if (!items) return { from, items: [], suffix: "", needs: { fields: model } };
      return { from, items, suffix: quote ? `${q}, ${q}` : `, ${q}` };
    }
    if (tuple.item === 1) {
      if (!fields && tuple.values[0]) return { from, items: [], suffix: "", needs: { fields: model } };
      const items = rank(operatorsFor(byName(tuple.values[0])?.type), (o) => o, partial).map((o) => ({ value: wrap(o), kind: "operator" as const }));
      return { from, items, suffix: quote ? `${q}, ` : ", " };
    }
    if (tuple.item === 2) {
      const items = valuesFor(byName(tuple.values[0]), !!quote).filter((v) => v.value.toLowerCase().startsWith(partial.toLowerCase()));
      return { from, items, suffix: quote ? `${q})` : ")" };
    }
    return null;
  }
  const dict = stack[0];
  if (!dict.colon) {
    const items = fieldItems();
    if (!items) return { from, items: [], suffix: "", needs: { fields: model } };
    return { from, items, suffix: quote ? `${q}: ` : ": " };
  }
  const items = valuesFor(byName(dict.values[dict.item]), !!quote).filter((v) => v.value.toLowerCase().startsWith(partial.toLowerCase()));
  return { from, items, suffix: quote ? q : "" };
}

export function completions(input: string, ctx: CompletionContext): Completion | null {
  const inside = structured(input, ctx);
  if (inside) return inside;
  const from = input.search(/\S*$/);
  const partial = input.slice(from);
  const before = input.slice(0, from).trim().split(/\s+/).filter(Boolean);
  if (!before.length) {
    const aliases = Object.entries(ctx.aliases ?? {}).map(([name, command]) => ({ name, detail: command, translateDetail: false }));
    const items = rank([...COMMANDS, ...aliases], (c) => c.name, partial).map((c) => ({ value: c.name, detail: c.detail, translateDetail: c.translateDetail, kind: "command" as const }));
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
  // read_group takes a comma-separated groupby right after the model.
  const groupby = verb === "read_group" && before.length === 2;
  const fieldList = previous === "-f" || groupby || (verb === "\\d" && before.length === 2);
  if (fieldList && model) {
    const fields = ctx.fields(model);
    const comma = previous === "-f" || groupby ? partial.lastIndexOf(",") + 1 : 0;
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
