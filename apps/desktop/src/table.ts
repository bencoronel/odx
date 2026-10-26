import type { OdooCall } from "./shell/commands";

// Result tables: the data, its text forms and exports. Rendering lives in TableBlock.tsx.

/** `code`: column indexes containing commands to highlight. `model`: source of the rows, for clickable ids and relations. */
/** `call`: the single Odoo call behind the rows, when there is one, so it can be copied as code. */
export type Table = { columns: string[]; rows: unknown[][]; code?: number[]; model?: string; call?: OdooCall };

/** A value as one line of text: many2one pairs read as "name (id)". */
export function cellText(v: unknown): string {
  if (v === null || v === undefined) return "null";
  if (typeof v === "string") return v;
  if (Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string") return `${v[1]} (${v[0]})`;
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}
export const isM2o = (v: unknown): v is [number, string] => Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string";
export const isIds = (v: unknown): v is number[] => Array.isArray(v) && v.length > 0 && v.every((n) => Number.isInteger(n) && n > 0);
/** What a double click on a cell follows. */
export type Link = { model: string; column: string; value: unknown; row: Record<string, unknown> };

/** Records as a table: columns in first-seen order. */
export function rowsTable(rows: Record<string, unknown>[]): Table {
  const columns: string[] = [];
  for (const row of rows) for (const key of Object.keys(row)) if (!columns.includes(key)) columns.push(key);
  return { columns, rows: rows.map((row) => columns.map((c) => row[c])) };
}

/** Export file name: model and UTC time, e.g. odx-res.partner-20261026-091500.csv. */
export const exportName = (table: Table, ext: string) => `odx-${table.model ?? "table"}-${new Date().toISOString().slice(0, 19).replace(/[-:]/g, "").replace("T", "-")}.${ext}`;

export const toTsv = (table: Table) => [table.columns, ...table.rows.map((row) => row.map(cellText))]
  .map((cells) => cells.map((c) => String(c).replace(/[\t\n\r]/g, " ")).join("\t"))
  .join("\n");
/** Raw values keyed by column, as Odoo returned them. */
export const toJson = (table: Table) => JSON.stringify(table.rows.map((row) => Object.fromEntries(table.columns.map((c, j) => [c, row[j] ?? null]))), null, 2);
export const toMarkdown = (table: Table) => {
  const line = (cells: string[]) => `| ${cells.map((c) => c.replace(/\|/g, "\\|").replace(/[\n\r]+/g, " ")).join(" | ")} |`;
  return [line(table.columns), line(table.columns.map(() => "---")), ...table.rows.map((row) => line(row.map(cellText)))].join("\n");
};
/** RFC 4180 with a BOM so spreadsheets detect UTF-8. Odoo data can come from outside (web forms), so values
 * that a spreadsheet would run as a formula get a leading apostrophe. */
const csvCell = (value: string) => {
  const safe = /^[=+\-@\t\r]/.test(value) ? `'${value}` : value;
  return /[",\n\r]/.test(safe) ? `"${safe.replace(/"/g, '""')}"` : safe;
};
// Numbers are data, never formulas: -150.5 stays a number in the spreadsheet.
export const toCsv = (table: Table) => "\uFEFF" + [table.columns.map(csvCell), ...table.rows.map((row) => row.map((v) => typeof v === "number" ? String(v) : csvCell(cellText(v))))]
  .map((cells) => cells.join(","))
  .join("\r\n");
