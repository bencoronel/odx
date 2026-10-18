import { LiteralError, parseIds, parseLiteral } from "./literal";
import { i18n } from "../i18n";

/** Parsed Odoo call; the session supplies the environment. */
export type OdooCall = { model: string; method: string; args: unknown[]; kwargs: Record<string, unknown> };

export type Command =
  | { kind: "help" }
  | { kind: "list" }
  | { kind: "connect"; env: string }
  | { kind: "disconnect" }
  | { kind: "expanded" }
  | { kind: "timing" }
  | { kind: "history"; q?: string; clear?: boolean }
  | { kind: "clear" }
  | { kind: "next" }
  | { kind: "context"; value?: Record<string, unknown> | null }
  | { kind: "call"; call: OdooCall; display: "rows" | "fields" | "models" | "names" | "value"; page?: { limit: number; offset: number }; filter?: string };

export const VERBS = ["search", "count", "read", "name", "create", "write", "unlink", "call", "help"];
export const META = ["\\?", "\\l", "\\c", "\\q", "\\x", "\\timing", "\\history", "\\s", "\\d", "\\dm", "\\next", "\\context", "\\clear"];
/** Verbs whose second token is a model, for autocomplete. */
export const MODEL_VERBS = ["search", "count", "read", "name", "create", "write", "unlink", "call", "\\d"];

export class CommandError extends Error {}

/** Split on spaces while preserving quoted and bracketed groups. */
export function tokenize(line: string): string[] {
  const tokens: string[] = [];
  let current = "";
  let depth = 0;
  let quote: string | null = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote) {
      current += c;
      if (c === "\\" && i + 1 < line.length) { current += line[++i]; continue; }
      if (c === quote) quote = null;
      continue;
    }
    if (c === "'" || c === '"') { quote = c; current += c; continue; }
    if ("[({".includes(c)) depth++;
    if ("])}".includes(c)) depth = Math.max(0, depth - 1);
    if (/\s/.test(c) && depth === 0) {
      if (current) tokens.push(current);
      current = "";
      continue;
    }
    current += c;
  }
  if (quote) throw new CommandError(i18n.t("shell.errors.unclosedQuote"));
  if (current) tokens.push(current);
  return tokens;
}

/** Strip outer quotes (`'Ada'` → `Ada`). */
function unquote(token: string) {
  return /^(['"]).*\1$/.test(token) ? String(parseLiteral(token)) : token;
}

function literal(token: string, what: "domain" | "fields" | "values" | "context" | "args" | "kwargs"): unknown {
  try { return parseLiteral(token); } catch (e) { throw new CommandError(i18n.t("shell.errors.literal", { what: i18n.t(`shell.literal.labels.${what}`), message: (e as LiteralError).message })); }
}
function ids(token: string | undefined): number[] {
  if (!token) throw new CommandError(i18n.t("shell.errors.missingIds"));
  try { return parseIds(token); } catch (e) { throw new CommandError((e as LiteralError).message); }
}
function model(token: string | undefined, usage: string): string {
  if (!token) throw new CommandError(i18n.t("shell.errors.missingModel", { usage }));
  if (!/^[a-z_][a-z0-9_]*(\.[a-z0-9_]+)*$/.test(token)) throw new CommandError(i18n.t("shell.errors.invalidModel", { model: token }));
  return token;
}

/** Parse `-x value` / `--option value` into positional arguments and options. */
function options(tokens: string[], allowed: Record<string, string>) {
  const positional: string[] = [];
  const opts: Record<string, string> = {};
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i];
    if (t.startsWith("-") && !/^-\d/.test(t)) {
      const key = allowed[t];
      if (!key) throw new CommandError(i18n.t("shell.errors.unknownOption", { option: t }));
      const v = tokens[++i];
      if (v === undefined) throw new CommandError(i18n.t("shell.errors.missingOptionValue", { option: t }));
      opts[key] = v;
    } else positional.push(t);
  }
  return { positional, opts };
}
function domainOf(raw: string | undefined): unknown[] {
  if (raw === undefined) return [];
  const d = literal(raw, "domain");
  if (!Array.isArray(d)) throw new CommandError(i18n.t("shell.errors.domainList"));
  return d;
}
function fieldsOf(raw: string | undefined): string[] {
  if (raw === undefined) return ["display_name"];
  const list = raw.startsWith("[") ? literal(raw, "fields") : raw.split(",").map((f) => f.trim()).filter(Boolean);
  if (!Array.isArray(list) || !list.length || !list.every((f) => typeof f === "string")) throw new CommandError(i18n.t("shell.errors.fieldsList"));
  return list as string[];
}
function intOf(raw: string | undefined, fallback: number, name: string, max = 1000) {
  if (raw === undefined) return fallback;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 0 || n > max) throw new CommandError(i18n.t("shell.errors.integerRange", { name, max }));
  return n;
}

export function searchCall(m: string, domain: unknown[], fields: string[], limit: number, offset: number, order?: string): OdooCall {
  const withId = fields.includes("id") ? fields : ["id", ...fields];
  return { model: m, method: "search_read", args: [], kwargs: { domain, fields: withId, limit: limit + 1, offset, order: order ?? "id asc" } };
}

export function parse(line: string): Command | null {
  const tokens = tokenize(line.trim());
  if (!tokens.length) return null;
  const [head, ...rest] = tokens;
  switch (head) {
    case "\\?": case "help": return { kind: "help" };
    case "\\l": return { kind: "list" };
    case "\\q": return { kind: "disconnect" };
    case "\\x": return { kind: "expanded" };
    case "\\timing": return { kind: "timing" };
    case "\\history": case "\\s": {
      const q = rest.join(" ");
      return q === "clear" ? { kind: "history", clear: true } : { kind: "history", q: q || undefined };
    }
    case "\\clear": return { kind: "clear" };
    case "\\next": return { kind: "next" };
    case "\\c": {
      if (!rest.length) throw new CommandError(i18n.t("shell.errors.connectUsage"));
      return { kind: "connect", env: unquote(rest.join(" ")) };
    }
    case "\\context": {
      if (!rest.length) return { kind: "context" };
      if (rest[0] === "-") return { kind: "context", value: null };
      const value = literal(rest.join(" "), "context");
      if (!value || typeof value !== "object" || Array.isArray(value)) throw new CommandError(i18n.t("shell.errors.contextObject"));
      return { kind: "context", value: value as Record<string, unknown> };
    }
    case "\\d": {
      const { positional, opts } = options(rest, { "--names": "names" });
      if (positional.length > 2) throw new CommandError(i18n.t("shell.errors.fieldsUsage"));
      const m = model(positional[0], i18n.t("shell.usage.fields"));
      const names = opts.names === undefined ? undefined : fieldsOf(opts.names);
      if (names && (names.length > 1000 || new Set(names).size !== names.length || names.some((name) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)))) throw new CommandError(i18n.t("shell.errors.technicalNames"));
      return { kind: "call", display: "fields", filter: positional[1], call: { model: m, method: "fields_get", args: [], kwargs: { attributes: ["type", "relation", "string", "required", "readonly", "store"], ...(names ? { allfields: names } : {}) } } };
    }
    case "\\dm": {
      const q = rest.join(" ");
      const domain = q ? ["|", ["model", "ilike", q], ["name", "ilike", q]] : [];
      // One extra row reveals whether more models exist.
      return { kind: "call", display: "models", page: { limit: 500, offset: 0 }, call: { model: "ir.model", method: "search_read", args: [], kwargs: { domain, fields: ["model", "name"], order: "model asc", limit: 501 } } };
    }
    case "search": {
      const { positional, opts } = options(rest, { "-d": "domain", "-f": "fields", "-l": "limit", "-o": "offset", "--order": "order" });
      const m = model(positional[0], i18n.t("shell.usage.search"));
      if (positional.length > 1) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: positional[1] }));
      const limit = intOf(opts.limit, 20, "-l");
      const offset = intOf(opts.offset, 0, "-o", Number.MAX_SAFE_INTEGER);
      return { kind: "call", display: "rows", page: { limit, offset }, call: searchCall(m, domainOf(opts.domain), fieldsOf(opts.fields), limit, offset, opts.order === undefined ? undefined : unquote(opts.order)) };
    }
    case "count": {
      const { positional, opts } = options(rest, { "-d": "domain" });
      return { kind: "call", display: "value", call: { model: model(positional[0], i18n.t("shell.usage.count")), method: "search_count", args: [domainOf(opts.domain)], kwargs: {} } };
    }
    case "read": {
      const { positional, opts } = options(rest, { "-f": "fields" });
      const m = model(positional[0], i18n.t("shell.usage.read"));
      if (positional.length > 2) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: positional[2] }));
      const fields = fieldsOf(opts.fields);
      return { kind: "call", display: "rows", call: { model: m, method: "read", args: [ids(positional[1])], kwargs: { fields: fields.includes("id") ? fields : ["id", ...fields] } } };
    }
    case "name": {
      const { positional, opts } = options(rest, { "-l": "limit" });
      const m = model(positional[0], i18n.t("shell.usage.name"));
      const limit = intOf(opts.limit, 20, "-l");
      // name_search has no offset: ask for one extra match to tell the user to refine.
      return { kind: "call", display: "names", page: { limit, offset: 0 }, call: { model: m, method: "name_search", args: [], kwargs: { name: unquote(positional.slice(1).join(" ")), limit: limit + 1 } } };
    }
    case "create": {
      const m = model(rest[0], i18n.t("shell.usage.create"));
      if (!rest[1]) throw new CommandError(i18n.t("shell.errors.missingValues"));
      return { kind: "call", display: "value", call: { model: m, method: "create", args: [literal(rest.slice(1).join(" "), "values")], kwargs: {} } };
    }
    case "write": {
      const m = model(rest[0], i18n.t("shell.usage.write"));
      if (!rest[2]) throw new CommandError(i18n.t("shell.errors.missingValues"));
      return { kind: "call", display: "value", call: { model: m, method: "write", args: [ids(rest[1]), literal(rest.slice(2).join(" "), "values")], kwargs: {} } };
    }
    case "unlink": {
      const m = model(rest[0], i18n.t("shell.usage.unlink"));
      if (rest.length > 2) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: rest[2] }));
      return { kind: "call", display: "value", call: { model: m, method: "unlink", args: [ids(rest[1])], kwargs: {} } };
    }
    case "call": {
      const m = model(rest[0], i18n.t("shell.usage.call"));
      const method = rest[1];
      if (!method || !/^[a-z][a-z0-9_]*$/i.test(method)) throw new CommandError(i18n.t("shell.errors.invalidMethod"));
      const args = rest[2] ? literal(rest[2], "args") : [];
      const kwargs = rest[3] ? literal(rest[3], "kwargs") : {};
      if (!Array.isArray(args)) throw new CommandError(i18n.t("shell.errors.argsList"));
      if (!kwargs || typeof kwargs !== "object" || Array.isArray(kwargs)) throw new CommandError(i18n.t("shell.errors.kwargsObject"));
      if (rest.length > 4) throw new CommandError(i18n.t("shell.errors.tooManyArguments"));
      return { kind: "call", display: "value", call: { model: m, method, args, kwargs: kwargs as Record<string, unknown> } };
    }
    default:
      throw new CommandError(i18n.t("shell.errors.unknownCommand", { command: head }));
  }
}
