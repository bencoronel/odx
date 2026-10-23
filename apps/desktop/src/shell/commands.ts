import { LiteralError, parseIds, parseLiteral } from "./literal";
import { i18n } from "../i18n";

/** Parsed Odoo call; the session supplies the environment. */
export type OdooCall = { model: string; method: string; args: unknown[]; kwargs: Record<string, unknown> };

export type Command =
  | { kind: "help" }
  | { kind: "list" }
  | { kind: "expanded" }
  | { kind: "timing" }
  | { kind: "history"; q?: string; clear?: boolean }
  | { kind: "clear" }
  | { kind: "next" }
  | { kind: "prev" }
  /** `remove`: forget the variable; without a name, list them. */
  | { kind: "set"; name?: string; remove?: boolean }
  /** `command` null removes the alias; without a name, list them. */
  | { kind: "alias"; name?: string; command?: string | null }
  | { kind: "grep"; q: string }
  /** `hide`: drop the listed columns instead of keeping them in that order. */
  | { kind: "cols"; columns: string[]; hide: boolean }
  | { kind: "context"; value?: Record<string, unknown> | null }
  | { kind: "whoami" }
  | { kind: "metadata"; model: string; ids: number[] }
  | { kind: "access"; model: string }
  | { kind: "has_group"; group: string }
  | { kind: "call"; call: OdooCall; display: "rows" | "fields" | "models" | "names" | "groups" | "value"; page?: { limit: number; offset: number }; filter?: string };

const ODOO_VERBS = new Set(["search", "count", "read", "name", "read_group", "ref", "metadata", "whoami", "cam", "uhg", "create", "write", "unlink", "call", "\\d", "\\dm", "\\next", "\\prev"]);
/** Whether a line calls Odoo, judged by its first word so an incomplete command counts too; the rest work on the
 * session alone (help, history, aliases, context, the screen). Expand aliases first. */
export const needsOdoo = (line: string) => ODOO_VERBS.has(line.trim().split(/\s+/)[0]?.toLowerCase() ?? "");

export const VERBS = ["search", "count", "read", "read_group", "name", "ref", "metadata", "whoami", "cam", "uhg", "create", "write", "unlink", "call", "help"];
export const META = ["\\?", "\\l", "\\x", "\\timing", "\\history", "\\s", "\\d", "\\dm", "\\next", "\\prev", "\\set", "\\alias", "\\grep", "\\cols", "\\context", "\\clear"];
/** Verbs whose second token is a model, for autocomplete. */
export const MODEL_VERBS = ["search", "count", "read", "read_group", "name", "metadata", "cam", "create", "write", "unlink", "call", "\\d"];

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

/** The line ends inside a string or bracket, so it is still being typed. */
export function unfinished(line: string): boolean {
  let depth = 0;
  let quote: string | null = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote) {
      if (c === "\\") i++;
      else if (c === quote) quote = null;
      continue;
    }
    if (c === "'" || c === '"') quote = c;
    else if ("[({".includes(c)) depth++;
    else if ("])}".includes(c)) depth = Math.max(0, depth - 1);
  }
  return quote !== null || depth > 0;
}

/** Strip outer quotes (`'Ada'` → `Ada`). */
function unquote(token: string) {
  if (!/^(['"]).*\1$/.test(token)) return token;
  try { return String(parseLiteral(token)); } catch (e) { throw new CommandError((e as LiteralError).message); }
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
/** `module.name`, as ir.model.data stores it. */
function xmlid(token: string | undefined, usage: string): [string, string] {
  const m = token === undefined ? null : /^([A-Za-z0-9_]+)\.([A-Za-z0-9_.-]+)$/.exec(unquote(token));
  if (!m) throw new CommandError(i18n.t("shell.errors.xmlid", { usage }));
  return [m[1], m[2]];
}
function intOf(raw: string | undefined, fallback: number, name: string, max = 1000) {
  if (raw === undefined) return fallback;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 0 || n > max) throw new CommandError(i18n.t("shell.errors.integerRange", { name, max }));
  return n;
}

/** `fields` undefined reads every field (`-f *`). */
export function searchCall(m: string, domain: unknown[], fields: string[] | undefined, limit: number, offset: number, order?: string): OdooCall {
  const kwargs: Record<string, unknown> = { domain, limit: limit + 1, offset, order: order ?? "id asc" };
  if (fields) kwargs.fields = fields.includes("id") ? fields : ["id", ...fields];
  return { model: m, method: "search_read", args: [], kwargs };
}

/** Replace `$_` (previous result) and `$name` (saved with \\set) outside strings with their ids. */
export function substitute(line: string, previous: number[] | undefined, named: Record<string, { ids: number[] }> = {}): string {
  let out = "";
  let quote: string | null = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (quote) {
      out += c;
      if (c === "\\" && i + 1 < line.length) { out += line[++i]; continue; }
      if (c === quote) quote = null;
      continue;
    }
    if (c === "'" || c === '"') { quote = c; out += c; continue; }
    const name = c === "$" ? /^\$([A-Za-z_]\w*)/.exec(line.slice(i))?.[1] : undefined;
    if (name === "_") {
      if (!previous) throw new CommandError(i18n.t("shell.errors.noPreviousIds"));
      if (!previous.length) throw new CommandError(i18n.t("shell.errors.emptyPreviousIds"));
      out += `[${previous.join(",")}]`;
      i++;
      continue;
    }
    if (name) {
      if (!Object.hasOwn(named, name)) throw new CommandError(i18n.t("shell.errors.unknownVariable", { name }));
      out += `[${named[name].ids.join(",")}]`;
      i += name.length;
      continue;
    }
    out += c;
  }
  return out;
}

/** `pc -l 5` runs the command saved as `pc` followed by ` -l 5`. */
export function expandAlias(line: string, aliases: Record<string, string>): string {
  const m = /^\s*(\S+)([\s\S]*)$/.exec(line);
  return m && Object.hasOwn(aliases, m[1]) ? aliases[m[1]] + m[2] : line;
}
const NAME = /^[A-Za-z][A-Za-z0-9_]*$/;

export function parse(line: string): Command | null {
  const tokens = tokenize(line.trim());
  if (!tokens.length) return null;
  const [head, ...rest] = tokens;
  switch (head) {
    case "\\?": case "help": return { kind: "help" };
    case "\\l": return { kind: "list" };
    case "\\x": return { kind: "expanded" };
    case "\\timing": return { kind: "timing" };
    case "\\history": case "\\s": {
      const q = rest.join(" ");
      return q === "clear" ? { kind: "history", clear: true } : { kind: "history", q: q || undefined };
    }
    case "\\clear": return { kind: "clear" };
    case "\\next": return { kind: "next" };
    case "\\prev": return { kind: "prev" };
    case "\\set": {
      if (!rest.length) return { kind: "set" };
      if (!NAME.test(rest[0]) || rest.length > 2 || (rest[1] !== undefined && rest[1] !== "-")) throw new CommandError(i18n.t("shell.errors.setUsage"));
      return { kind: "set", name: rest[0], remove: rest[1] === "-" };
    }
    case "\\alias": {
      if (!rest.length) return { kind: "alias" };
      const name = rest[0];
      if (!NAME.test(name) || VERBS.includes(name)) throw new CommandError(i18n.t("shell.errors.aliasName", { name }));
      if (rest.length === 2 && rest[1] === "-") return { kind: "alias", name, command: null };
      // Keep the command as typed; it may end in an option the alias user completes.
      const command = /^\s*\S+\s+\S+\s+([\s\S]+)$/.exec(line)?.[1].trim();
      const verb = rest[1];
      if (!command || !verb || !(VERBS.includes(verb) || META.includes(verb))) throw new CommandError(i18n.t("shell.errors.aliasUsage"));
      return { kind: "alias", name, command };
    }
    case "\\grep": {
      if (!rest.length) throw new CommandError(i18n.t("shell.errors.grepUsage"));
      return { kind: "grep", q: rest.length === 1 ? unquote(rest[0]) : rest.join(" ") };
    }
    case "\\cols": {
      const raw = rest.join(" ");
      const hide = raw.startsWith("-");
      const columns = (hide ? raw.slice(1) : raw).split(",").map((c) => unquote(c.trim())).filter(Boolean);
      if (!columns.length || new Set(columns).size !== columns.length) throw new CommandError(i18n.t("shell.errors.colsUsage"));
      return { kind: "cols", columns, hide };
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
      const fields = opts.fields === "*" ? undefined : fieldsOf(opts.fields);
      return { kind: "call", display: "rows", page: { limit, offset }, call: searchCall(m, domainOf(opts.domain), fields, limit, offset, opts.order === undefined ? undefined : unquote(opts.order)) };
    }
    case "count": {
      const { positional, opts } = options(rest, { "-d": "domain" });
      return { kind: "call", display: "value", call: { model: model(positional[0], i18n.t("shell.usage.count")), method: "search_count", args: [domainOf(opts.domain)], kwargs: {} } };
    }
    case "read": {
      const { positional, opts } = options(rest, { "-f": "fields" });
      const m = model(positional[0], i18n.t("shell.usage.read"));
      if (positional.length > 2) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: positional[2] }));
      // -f * omits fields so Odoo returns every readable field.
      if (opts.fields === "*") return { kind: "call", display: "rows", call: { model: m, method: "read", args: [ids(positional[1])], kwargs: {} } };
      const fields = fieldsOf(opts.fields);
      return { kind: "call", display: "rows", call: { model: m, method: "read", args: [ids(positional[1])], kwargs: { fields: fields.includes("id") ? fields : ["id", ...fields] } } };
    }
    case "read_group": {
      const { positional, opts } = options(rest, { "-d": "domain", "-f": "fields", "-l": "limit", "--order": "order" });
      const m = model(positional[0], i18n.t("shell.usage.readGroup"));
      if (!positional[1]) throw new CommandError(i18n.t("shell.errors.missingGroupby", { usage: i18n.t("shell.usage.readGroup") }));
      if (positional.length > 2) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: positional[2] }));
      const groupby = positional[1].split(",").map((g) => g.trim()).filter(Boolean);
      const aggregates = opts.fields === undefined ? [] : fieldsOf(opts.fields);
      const limit = intOf(opts.limit, 80, "-l");
      // lazy=false groups by every field at once and reports __count.
      const kwargs: Record<string, unknown> = { domain: domainOf(opts.domain), fields: aggregates, groupby, lazy: false, limit: limit + 1 };
      if (opts.order !== undefined) kwargs.orderby = unquote(opts.order);
      return { kind: "call", display: "groups", page: { limit, offset: 0 }, call: { model: m, method: "read_group", args: [], kwargs } };
    }
    case "ref": {
      if (rest.length > 1) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: rest[1] }));
      const [module, name] = xmlid(rest[0], i18n.t("shell.usage.ref"));
      return { kind: "call", display: "rows", call: { model: "ir.model.data", method: "search_read", args: [], kwargs: { domain: [["module", "=", module], ["name", "=", name]], fields: ["module", "name", "model", "res_id"], limit: 2, order: "id asc" } } };
    }
    case "whoami":
      if (rest.length) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: rest[0] }));
      return { kind: "whoami" };
    case "metadata": {
      const m = model(rest[0], i18n.t("shell.usage.metadata"));
      if (rest.length > 2) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: rest[2] }));
      return { kind: "metadata", model: m, ids: ids(rest[1]) };
    }
    case "cam": {
      const m = model(rest[0], i18n.t("shell.usage.access"));
      if (rest.length > 1) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: rest[1] }));
      return { kind: "access", model: m };
    }
    case "uhg": {
      if (rest.length > 1) throw new CommandError(i18n.t("shell.errors.unexpectedArgument", { argument: rest[1] }));
      return { kind: "has_group", group: xmlid(rest[0], i18n.t("shell.usage.hasGroup")).join(".") };
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
