import type { Environment } from "../ipc";
import { formatAppError, type AppError } from "../appError";
import { i18n } from "../i18n";
import { cellText, rowsTable, type Table } from "../table";
import type { OdooCall } from "./commands";
import type { Confirmation, ConnectionErrorPresentation, Entry, NewEntry } from "./sessions";

// Pure output of the Terminal: results as entries, entries as an agent reads them, and calls as Python.

/** What a call's result prints: a table, a value or a note, and the ids `$_` then stands for (create, search and reads). */
export function resultEntries(p: Confirmation, data: unknown, meta: string, expanded: boolean): { entries: NewEntry[]; previous?: { model?: string; ids: number[] } } {
  const t = i18n.t.bind(i18n);
  let previous: { model?: string; ids: number[] } | undefined;
  const done = (...entries: NewEntry[]) => ({ entries, previous });
  // create returns the new id (or ids): `$_` then means the new records, not an earlier result.
  const created = p.call.method === "create" && typeof data === "number" ? [data] : data;
  if (Array.isArray(created) && p.display !== "groups") {
    // name_search returns [id, name] pairs; reads return records; search and create return ids.
    const ids = created.map((r) => typeof r === "number" ? r : Array.isArray(r) ? r[0] : (r as { id?: unknown } | null)?.id).filter((id): id is number => typeof id === "number");
    previous = { model: p.call.model, ids: p.page ? ids.slice(0, p.page.limit) : ids };
  }
  const m = meta ? ` · ${meta}` : "";
  if (p.display === "fields" && data && typeof data === "object") {
    const q = p.filter?.toLowerCase();
    const rows = Object.entries(data as Record<string, Record<string, unknown>>)
      .filter(([name, f]) => !q || name.toLowerCase().includes(q) || String(f.string ?? "").toLowerCase().includes(q))
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, f]) => [name, f.type, f.relation ?? "", f.string, f.required ? t("shell.yes") : "", f.store === false ? t("shell.no") : ""]);
    return done({ kind: "table", expanded, table: { columns: [t("shell.field"), t("shell.type"), t("shell.relationship"), t("shell.label"), t("shell.required"), t("shell.stored")], rows, call: p.call }, footer: t("shell.count", { count: rows.length, item: t("shell.fields") }) + m });
  }
  if (p.display === "names" && Array.isArray(data)) {
    const more = !!p.page && data.length > p.page.limit;
    const rows = (p.page ? data.slice(0, p.page.limit) : data) as unknown[][];
    return done({ kind: "table", expanded, table: { columns: ["id", t("shell.name")], rows, model: p.call.model, call: p.call }, footer: t("shell.count", { count: rows.length, item: t("shell.results") }) + m + (more ? t("shell.moreRefine") : "") });
  }
  if (p.display === "groups" && Array.isArray(data) && data.length) {
    const groupby = p.call.kwargs.groupby as string[];
    const more = !!p.page && data.length > p.page.limit;
    const groups = (p.page ? data.slice(0, p.page.limit) : data) as Record<string, unknown>[];
    // Groupby columns first, then the count, then aggregates; drop Odoo's __domain/__range/__context.
    const rows = groups.map((g) => Object.fromEntries([
      ...groupby.map((key) => [key, g[key]]),
      ["count", g.__count],
      ...Object.entries(g).filter(([key]) => !key.startsWith("__") && !groupby.includes(key)),
    ]));
    return done({ kind: "table", expanded, table: { ...rowsTable(rows), model: p.call.model, call: p.call }, footer: t("shell.count", { count: rows.length, item: t("shell.groups") }) + m + (more ? t("shell.moreGroups") : "") });
  }
  if (Array.isArray(data) && data.length && data.every((r) => r && typeof r === "object" && !Array.isArray(r))) {
    let rows = data as Record<string, unknown>[];
    let more = false;
    if (p.page) { more = rows.length > p.page.limit; rows = rows.slice(0, p.page.limit); }
    const where = p.page ? t("shell.fromOffset", { offset: p.page.offset }) : "";
    // A single wide record reads better as a vertical list.
    const wide = rows.length === 1 && Object.keys(rows[0]).length > 12;
    return done({ kind: "table", expanded: expanded || wide, table: { ...rowsTable(rows), model: p.call.model, call: p.call }, footer: t("shell.row", { count: rows.length }) + where + m + (more ? t("shell.moreNext") : "") });
  }
  if (Array.isArray(data) && !data.length) return done({ kind: "info", text: `${t("shell.noResults")}${m}` });
  return done({ kind: "text", text: typeof data === "object" ? JSON.stringify(data, null, 2) : String(data) }, ...(meta ? [{ kind: "info" as const, text: meta }] : []));
}

/** `\grep` keeps the rows containing `q`; `\cols` keeps (or with `hide` drops) the named columns. */
export function reshapeTable(table: Table, command: { kind: "grep"; q: string } | { kind: "cols"; columns: string[]; hide?: boolean }): Table | { missing: string[] } {
  if (command.kind === "grep") {
    const q = command.q.toLowerCase();
    return { ...table, rows: table.rows.filter((row) => row.some((v) => cellText(v).toLowerCase().includes(q))) };
  }
  const missing = command.columns.filter((c) => !table.columns.includes(c));
  if (missing.length) return { missing };
  const keep = command.hide ? table.columns.filter((c) => !command.columns.includes(c)) : command.columns;
  const at = keep.map((c) => table.columns.indexOf(c));
  return { ...table, columns: keep, rows: table.rows.map((row) => at.map((j) => row[j])), code: table.code?.map((j) => at.indexOf(j)).filter((j) => j >= 0) };
}

export function connectionProblem(problem: ConnectionErrorPresentation | AppError): string {
  if ("code" in problem) return formatAppError(problem);
  return i18n.t("shell.connectionError", { name: problem.name, reason: i18n.t(problem.reason === "credential" ? "shell.credentialRejected" : "shell.offline"), error: problem.detail ? ` (${formatAppError(problem.detail)})` : "" });
}

/** An entry as an agent reads it: text as shown, errors in words, tables as columns and (capped) rows. */
export function plainEntry(entry: Entry): unknown {
  if (entry.kind === "table") return { columns: entry.table.columns, rows: entry.table.rows.slice(0, 200), total_rows: entry.table.rows.length, model: entry.table.model, footer: entry.footer };
  if (entry.kind === "input") return { kind: "input", text: entry.text };
  const text = entry.connectionError ? connectionProblem(entry.connectionError) : entry.appError ? formatAppError(entry.appError) : entry.translationKey ? i18n.t(entry.translationKey) : entry.text;
  return { kind: entry.kind, text };
}

/** A command block's mark in the margin: errors first, then writes (and their confirmations), then plain reads. */
export function outcomeOf(entries: Entry[]): "error" | "write" | "read" | undefined {
  const [first] = entries;
  if (first?.kind !== "input" || !first.text.trim()) return undefined;
  if (entries.some((entry) => entry.kind === "error")) return "error";
  const verb = first.text.trim().split(/\s+/)[0];
  // A confirmation answer is its own block, under the write's question.
  return ["create", "write", "unlink"].includes(verb) || first.prompt.endsWith("]") ? "write" : "read";
}

/** JSON as a Python literal: the same structure with None, True and False. */
function pythonLiteral(value: unknown): string {
  if (value === null || value === undefined) return "None";
  if (typeof value === "boolean") return value ? "True" : "False";
  if (typeof value === "number") return String(value);
  if (typeof value === "string") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(pythonLiteral).join(", ")}]`;
  return `{${Object.entries(value as Record<string, unknown>).map(([k, v]) => `${JSON.stringify(k)}: ${pythonLiteral(v)}`).join(", ")}}`;
}

/** An Odoo call as a standalone XML-RPC script; the API key stays a placeholder, Odx never copies secrets. */
export function pythonCall(env: Pick<Environment, "url" | "database" | "login">, call: OdooCall, context: Record<string, unknown>): string {
  const kwargs = Object.keys(context).length ? { ...call.kwargs, context: { ...context, ...(call.kwargs.context as object | undefined) } } : call.kwargs;
  return [
    "import xmlrpc.client",
    "",
    `url, db, login = ${JSON.stringify(env.url.replace(/\/+$/, ""))}, ${JSON.stringify(env.database)}, ${JSON.stringify(env.login)}`,
    'key = "<API key>"',
    'uid = xmlrpc.client.ServerProxy(f"{url}/xmlrpc/2/common").authenticate(db, login, key, {})',
    'models = xmlrpc.client.ServerProxy(f"{url}/xmlrpc/2/object")',
    `result = models.execute_kw(db, uid, key, ${JSON.stringify(call.model)}, ${JSON.stringify(call.method)}, ${pythonLiteral(call.args)}, ${pythonLiteral(kwargs)})`,
    "print(result)",
    "",
  ].join("\n");
}
