import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import CopyButton from "./CopyButton";
import { highlight } from "./shell/highlight";
import type { Field } from "./shell/complete";
import type { OdooCall } from "./shell/commands";
import { formatDateTime, formatNumber } from "./i18n/format";
import { useDismiss } from "./ContextMenu";
import { cellText, isIds, isM2o, toJson, toMarkdown, toTsv, type Link, type Table } from "./table";

/** Cell clicks: follow a record or relation, or open a long value. */
/** What a table can do. In a session all of it; in a table window only reading: the optional ones need the session. */
export const CellActions = createContext<{ follow?: (link: Link, inOdoo?: boolean) => void; inspect?: (title: string, value: unknown) => void; exportTable?: (table: Table, format: "csv" | "json") => void;
  /** A model's fields for header tooltips, when loaded. */
  fields: (model: string) => Field[] | undefined;
  /** The call as a Python script against this tab's environment, without its API key. */
  callCode?: (call: OdooCall) => string | null;
  /** Put a command in the input, whole, to edit or run it. */
  insert?: (command: string) => void;
  /** Open the table in a window of its own. */
  popOut?: (table: Table, footer?: string) => void;
  /** Alias names, so commands in table cells highlight like typed ones. */
  aliases?: string[] } | null>(null);

/** `pos` ("row:column") and `active` support keyboard navigation; `pin` keeps the column visible when scrolling sideways. */
/** How a value reads once its field is known: selection labels, datetimes in local time, booleans as marks.
 * `tip` keeps the raw value; copying and exporting always use the raw value. */
function typedValue(value: unknown, field?: Field): { text: string; tip?: string; className?: string } | null {
  if (!field) return null;
  if (field.type === "boolean" && typeof value === "boolean") return { text: value ? "✓" : "✗", tip: String(value), className: value ? "bool yes" : "bool no" };
  if (field.type === "selection" && typeof value === "string") {
    const label = field.selection?.find(([key]) => key === value)?.[1];
    return label ? { text: label, tip: value } : null;
  }
  // Odoo sends datetimes in UTC without a zone.
  if (field.type === "datetime" && typeof value === "string" && /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(value)) {
    const at = new Date(`${value.replace(" ", "T")}Z`);
    return Number.isNaN(at.getTime()) ? null : { text: formatDateTime(at, { dateStyle: "short", timeStyle: "medium" }), tip: `${value} UTC` };
  }
  return null;
}

// One name lookup per model's field list, not a scan per cell.
const fieldMaps = new WeakMap<Field[], Map<string, Field>>();
function fieldNamed(fields: Field[] | undefined, name: string): Field | undefined {
  if (!fields) return undefined;
  let map = fieldMaps.get(fields);
  if (!map) fieldMaps.set(fields, map = new Map(fields.map((f) => [f.name, f])));
  return map.get(name);
}

/** `edges`: the selection's outline on this cell (inset shadows on the sides that border unselected cells). */
function Cell({ value, column, model, row, pos, active, selected, edges, pin }: { value: unknown; column: string; model?: string; row: Record<string, unknown>; pos?: string; active?: boolean; selected?: boolean; edges?: string; pin?: boolean }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const typed = typedValue(value, model ? fieldNamed(actions?.fields(model), column) : undefined);
  const text = typed?.text ?? cellText(value);
  const dim = !typed && (value === false || value === null || value === undefined || (Array.isArray(value) && !value.length));
  const short = text.length > 80 ? `${text.slice(0, 80)}…` : text;
  const className = `${dim ? "dim" : ""} ${typeof value === "number" ? "num" : ""} ${typed?.className ?? ""} ${pin ? "pin" : ""}`;
  const cell = { "data-cell": pos, "data-active": active || undefined, "data-selected": selected || undefined, style: edges ? { boxShadow: edges } : undefined };
  // ir.model.data rows point at another model through res_id.
  const follow = actions?.follow;
  const linked = model && follow && (
    (column === "id" && typeof value === "number")
    || (model === "ir.model.data" && column === "res_id" && typeof value === "number" && typeof row.model === "string")
    || (column !== "id" && (isM2o(value) || isIds(value))));
  // A click selects the cell (see TableBlock); a double click acts on it.
  if (linked) return <td className={className} {...cell}><button type="button" className="term-link" tabIndex={-1} data-tip={t("shell.openRecord")}
    onDoubleClick={(event) => follow({ model, column, value, row }, event.ctrlKey || event.metaKey)}>{short}</button></td>;
  const inspect = actions?.inspect;
  if (inspect && (short !== text || (value !== null && typeof value === "object"))) return <td className={className} {...cell}>
    <button type="button" className="term-more" tabIndex={-1} data-tip={t("shell.inspectValue")}
      onDoubleClick={() => inspect(`${model ? `${model} ` : ""}${row.id !== undefined ? `${cellText(row.id)} · ` : ""}${column}`, value)}>{short}</button>
  </td>;
  return <td className={className} {...cell} data-tip={typed?.tip ?? (short !== text ? text : undefined)}>{short}</td>;
}

/** Rows a table draws in the output before "show all". */
const SHOWN_ROWS = 50;

/** Many2one values sort by name; empty values go last in both directions. */
function compareCells(a: unknown, b: unknown): number {
  const key = (v: unknown) => Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "string" ? v[1] : v;
  const x = key(a);
  const y = key(b);
  if (typeof x === "number" && typeof y === "number") return x - y;
  return cellText(x).localeCompare(cellText(y), undefined, { numeric: true, sensitivity: "base" });
}
const empty = (v: unknown) => v === false || v === null || v === undefined || v === "";

/** `bare`: only the table, for a window that brings its own controls. `headerEnd`: a control in the header's last cell,
 * kept at the right edge while scrolling sideways. */
export function TableBlock({ table, expanded, footer, onToggle, bare, headerEnd }: { table: Table; expanded: boolean; footer?: string; onToggle: () => void; bare?: boolean; headerEnd?: ReactNode }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const aliases = actions?.aliases;
  const [sort, setSort] = useState<{ column: number; desc: boolean } | null>(null);
  const described = table.model ? actions?.fields(table.model) : undefined;
  /** Label, type and relation of a technical column name, when the model's fields are known. */
  const about = (column: string) => {
    const field = fieldNamed(described, column);
    return field ? `${field.label}\n${field.type}${field.relation ? ` → ${field.relation}` : ""}` : undefined;
  };
  const [active, setActive] = useState<{ r: number; c: number } | null>(null);
  // Cells, columns and rows are selected like a spreadsheet's, not their text: `anchor` is where a range starts.
  const [selection, setSelection] = useState<{ cells: Set<string>; anchor: { r: number; c: number } } | null>(null);
  const dragging = useRef(false);
  // The cursor frame marks where the keys act: shown for a lone cell or while moving with the keyboard, not over a mouse selection.
  const [keyboard, setKeyboard] = useState(false);
  // A press anywhere outside the table lets go of its selection.
  useDismiss(!!selection, () => scroller.current, () => setSelection(null));
  const [copied, setCopied] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const rows = sort === null ? table.rows : [...table.rows].sort((a, b) => {
    const [x, y] = [a[sort.column], b[sort.column]];
    if (empty(x) || empty(y)) return Number(empty(x)) - Number(empty(y));
    return sort.desc ? compareCells(y, x) : compareCells(x, y);
  });
  // The output draws the first rows until asked for all: a long session keeps a small page to scroll. Sorting, totals,
  // copying and exporting use every row; the table window (bare) draws them all.
  // shortcut: find-in-output only sees drawn rows; "show all" makes the rest searchable.
  const [all, setAll] = useState(false);
  const shown = bare || all ? rows : rows.slice(0, SHOWN_ROWS);
  // A sum per numeric column (record ids aside), over the rows shown: \grep filters first. Empty values count as nothing.
  const totals = rows.length > 1 ? table.columns.map((column, j) => {
    if (column === "id" || !rows.every((row) => row[j] === null || row[j] === false || typeof row[j] === "number")) return null;
    const values = rows.map((row) => row[j]).filter((value): value is number => typeof value === "number");
    return values.length ? { sum: values.reduce((a, b) => a + b, 0), count: values.length } : null;
  }) : null;
  const totalsShown = totals?.some(Boolean) ? totals : null;
  const record = (row: unknown[]) => Object.fromEntries(table.columns.map((c, j) => [c, row[j]]));
  // Ascending, descending, then back to the original order.
  const cycle = (column: number) => setSort(sort?.column !== column ? { column, desc: false } : sort.desc ? null : { column, desc: true });
  // The id column stays visible while scrolling sideways.
  const pinned = table.columns[0] === "id" || table.columns[0] === "#";
  const cellKey = (r: number, c: number) => `${r}:${c}`;
  const box = (a: { r: number; c: number }, b: { r: number; c: number }) => {
    const cells = new Set<string>();
    for (let r = Math.min(a.r, b.r); r <= Math.max(a.r, b.r); r++) for (let c = Math.min(a.c, b.c); c <= Math.max(a.c, b.c); c++) cells.add(cellKey(r, c));
    return cells;
  };
  const lastRow = shown.length - 1;
  const lastColumn = table.columns.length - 1;
  /** Shift extends from the anchor, Ctrl adds or removes, otherwise the target replaces the selection. */
  const choose = (target: Set<string>, anchor: { r: number; c: number }, end: { r: number; c: number }, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => {
    if (event.shiftKey && selection) return setSelection({ ...selection, cells: box(selection.anchor, end) });
    if ((event.ctrlKey || event.metaKey) && selection) {
      const cells = new Set(selection.cells);
      const all = [...target].every((key) => cells.has(key));
      for (const key of target) { if (all) cells.delete(key); else cells.add(key); }
      return setSelection({ cells, anchor });
    }
    setSelection({ cells: target, anchor });
  };
  const selectCell = (r: number, c: number, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => { setActive({ r, c }); choose(new Set([cellKey(r, c)]), { r, c }, { r, c }, event); };
  // Whole columns and rows: Shift runs from the anchor's column (or row) to this one, across the table.
  const selectColumn = (c: number, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => {
    if (event.shiftKey && selection) return setSelection({ ...selection, cells: box({ r: 0, c: selection.anchor.c }, { r: lastRow, c }) });
    choose(box({ r: 0, c }, { r: lastRow, c }), { r: 0, c }, { r: lastRow, c }, event);
  };
  const selectRow = (r: number, event: { shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }) => {
    if (event.shiftKey && selection) return setSelection({ ...selection, cells: box({ r: selection.anchor.r, c: 0 }, { r, c: lastColumn }) });
    choose(box({ r, c: 0 }, { r, c: lastColumn }), { r, c: 0 }, { r, c: lastColumn }, event);
  };
  /** Outline the selected blocks like a spreadsheet: a side gets a line where the neighbour is not selected. */
  const edgesOf = (r: number, c: number) => {
    const has = (row: number, column: number) => !!selection?.cells.has(cellKey(row, column));
    if (!has(r, c)) return undefined;
    const line = "var(--accent)";
    return [!has(r - 1, c) && `inset 0 1px 0 ${line}`, !has(r, c + 1) && `inset -1px 0 0 ${line}`, !has(r + 1, c) && `inset 0 -1px 0 ${line}`, !has(r, c - 1) && `inset 1px 0 0 ${line}`]
      .filter(Boolean).join(", ") || undefined;
  };
  const selectedColumn = (c: number) => !!selection && shown.length > 0 && shown.every((_, r) => selection.cells.has(cellKey(r, c)));
  const selectedRow = (r: number) => !!selection && table.columns.every((_, c) => selection.cells.has(cellKey(r, c)));
  /** One cell copies as its value; more copy as TSV with the headers of their columns, blanks where a row skips one. */
  const selectionText = () => {
    const keys = selection?.cells.size ? [...selection.cells].map((key) => key.split(":").map(Number)) : active ? [[active.r, active.c]] : [];
    if (keys.length === 1) return cellText(rows[keys[0][0]][keys[0][1]]);
    const clean = (text: string) => text.replace(/[\t\n\r]/g, " ");
    const used = (index: 0 | 1) => [...new Set(keys.map((key) => key[index]))].sort((a, b) => a - b);
    const columns = used(1);
    return [columns.map((c) => clean(table.columns[c])).join("\t"),
      ...used(0).map((r) => columns.map((c) => selection!.cells.has(cellKey(r, c)) ? clean(cellText(rows[r][c])) : "").join("\t"))].join("\n");
  };
  const copySelection = () => void navigator.clipboard.writeText(selectionText()).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1200); }).catch(() => undefined);
  useEffect(() => {
    const stop = () => { dragging.current = false; };
    window.addEventListener("mouseup", stop);
    return () => window.removeEventListener("mouseup", stop);
  }, []);
  useLayoutEffect(() => {
    if (active) scroller.current?.querySelector(`[data-cell="${active.r}:${active.c}"]`)?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [active]);

  function onKey(event: KeyboardEvent<HTMLDivElement>) {
    if (expanded || !shown.length || event.altKey) return;
    const at = active ?? { r: 0, c: 0 };
    const last = { r: shown.length - 1, c: table.columns.length - 1 };
    const move = (r: number, c: number) => {
      event.preventDefault();
      const to = { r: Math.max(0, Math.min(last.r, r)), c: Math.max(0, Math.min(last.c, c)) };
      setActive(to);
      setKeyboard(true);
      // Shift+arrows grow the selection from its anchor; plain arrows move a one-cell selection.
      setSelection(event.shiftKey && selection ? { ...selection, cells: box(selection.anchor, to) } : { cells: new Set([cellKey(to.r, to.c)]), anchor: to });
    };
    const ctrl = event.ctrlKey || event.metaKey;
    if (ctrl && event.key.toLowerCase() === "a") { event.preventDefault(); setSelection({ cells: box({ r: 0, c: 0 }, last), anchor: { r: 0, c: 0 } }); return; }
    switch (event.key) {
      case "ArrowUp": if (!ctrl) move(at.r - 1, at.c); return;
      case "ArrowDown": if (!ctrl) move(at.r + 1, at.c); return;
      case "ArrowLeft": return move(at.r, at.c - 1);
      case "ArrowRight": return move(at.r, at.c + 1);
      case "Home": return ctrl ? move(0, 0) : move(at.r, 0);
      case "End": return ctrl ? move(last.r, last.c) : move(at.r, last.c);
      case "PageUp": return move(at.r - 10, at.c);
      case "PageDown": return move(at.r + 10, at.c);
      case "Enter": {
        event.preventDefault();
        const td = scroller.current?.querySelector(`[data-cell="${at.r}:${at.c}"]`);
        const button = td?.querySelector("button");
        if (button) button.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, ctrlKey: ctrl }));
        else actions?.inspect?.(`${table.model ? `${table.model} · ` : ""}${table.columns[at.c]}`, rows[at.r][at.c]);
        return;
      }
    }
    if (event.key.toLowerCase() === "c" && !event.shiftKey) {
      event.preventDefault();
      copySelection();
    }
  }

  return <div className={bare ? "term-block bare" : "term-block"}>
    <div className="term-scroll" ref={scroller} tabIndex={expanded ? undefined : 0} data-pinned={pinned || undefined}
      aria-label={t("shell.tableNavigation")} onKeyDown={onKey}
      onFocus={() => { if (!active) setActive({ r: 0, c: 0 }); }}
      onMouseDown={(event) => {
        if (event.button !== 0 || expanded) return;
        const cell = (event.target as HTMLElement).closest<HTMLElement>("[data-cell]")?.dataset.cell;
        if (!cell) return;
        const [r, c] = cell.split(":").map(Number);
        selectCell(r, c, event);
        setKeyboard(false);
        dragging.current = !event.shiftKey && !event.ctrlKey && !event.metaKey;
      }}
      onMouseOver={(event) => {
        if (!dragging.current || !selection) return;
        const cell = (event.target as HTMLElement).closest<HTMLElement>("[data-cell]")?.dataset.cell;
        if (cell) { const [r, c] = cell.split(":").map(Number); setSelection({ ...selection, cells: box(selection.anchor, { r, c }) }); }
      }}>
      {expanded ? shown.map((row, i) => { const values = record(row); return <table key={i} className="term-table expanded"><tbody>
        <tr><th colSpan={2}>{t("shell.record", { index: i + 1 })}</th></tr>
        {table.columns.map((c, j) => <tr key={c}><th data-tip={about(c)}>{c}</th><Cell value={row[j]} column={c} model={table.model} row={values} /></tr>)}
      </tbody></table>; })
        : <table className="term-table" data-copied={copied || undefined}><thead><tr>
          <th className="term-row-head" aria-label={t("shell.selectAll")} data-tip={t("shell.selectAll")} onMouseDown={(event) => { event.preventDefault(); setSelection({ cells: box({ r: 0, c: 0 }, { r: lastRow, c: lastColumn }), anchor: { r: 0, c: 0 } }); }} />
          {table.columns.map((c, j) => <th key={c} className={pinned && j === 0 ? "pin" : undefined} data-selected={selectedColumn(j) || undefined} data-tip={about(c)}
            aria-sort={sort?.column === j ? (sort.desc ? "descending" : "ascending") : undefined}
            onMouseDown={(event) => { if (event.button !== 0 || (event.target as HTMLElement).closest(".term-sort")) return; event.preventDefault(); selectColumn(j, event); }}>
          <span className="term-column">{c}</span>
          <button type="button" className="term-sort" tabIndex={-1} aria-label={t("shell.sortBy", { column: c })} onClick={() => cycle(j)}>
            <Icon name={sort?.column === j && sort.desc ? "caretDown" : "caretUp"} className={sort?.column === j ? "on" : undefined} />
          </button>
        </th>)}{headerEnd && <th className="term-header-end">{headerEnd}</th>}</tr></thead>
          <tbody>{shown.map((row, i) => { const values = record(row); return <tr key={i}>
            <td className="term-row-head" data-selected={selectedRow(i) || undefined} onMouseDown={(event) => { if (event.button !== 0) return; event.preventDefault(); selectRow(i, event); }}>{i + 1}</td>
            {row.map((v, j) => {
            const pos = `${i}:${j}`;
            const on = active?.r === i && active.c === j && (keyboard || (selection?.cells.size ?? 0) <= 1);
            const selected = selection?.cells.has(pos);
            const edges = edgesOf(i, j);
            return table.code?.includes(j)
              ? <td key={j} data-cell={pos} data-active={on || undefined} data-selected={selected || undefined} style={edges ? { boxShadow: edges } : undefined}>{actions?.insert
                ? <button type="button" className="term-link term-command" tabIndex={-1} data-tip={t("shell.editCommand")} onDoubleClick={() => actions.insert?.(String(v))}>{highlight(String(v), aliases)}</button>
                : highlight(String(v), aliases)}</td>
              : <Cell key={j} value={v} column={table.columns[j]} model={table.model} row={values} pos={pos} active={on} selected={selected} edges={edges} pin={pinned && j === 0} />;
          })}{headerEnd && <td className="term-header-end" />}</tr>; })}</tbody>
          {totalsShown && <tfoot><tr><td className="term-row-head" aria-label={t("shell.total")}>Σ</td>{totalsShown.map((total, j) => <td key={j} className={[pinned && j === 0 && "pin", total && "num"].filter(Boolean).join(" ") || undefined}>
            {total && <span data-tip={t("shell.average", { value: formatNumber(total.sum / total.count, { maximumFractionDigits: 2 }) })}>{formatNumber(total.sum, { maximumFractionDigits: 2 })}</span>}
          </td>)}{headerEnd && <td className="term-header-end" />}</tr></tfoot>}
        </table>}
    </div>
    {shown.length < rows.length && <button type="button" className="term-show-all" onClick={() => setAll(true)}>{t("shell.showAllRows", { shown: shown.length, count: rows.length })}</button>}
    {!bare && <div className="term-foot">
      <span className="term-foot-info">{footer ? `(${footer})` : ""}{copied ? ` · ${t("common.copied")}` : ""}</span>
      <span className="term-foot-actions">
        <button type="button" className="term-copy" aria-label={t(expanded ? "shell.collapseTable" : "shell.expandTable")} data-tip={t(expanded ? "shell.collapseTable" : "shell.expandTable")}
          aria-pressed={expanded} onClick={onToggle}><Icon name={expanded ? "table" : "rows"} /></button>
        {actions?.popOut && <button type="button" className="term-copy" aria-label={t("shell.openInWindow")} data-tip={t("shell.openInWindow")} onClick={() => actions.popOut?.(table, footer)}><Icon name="external" /></button>}
        <CopyTable table={{ ...table, rows }} />
      </span>
    </div>}
  </div>;
}

/** TSV for spreadsheets one click away; other formats behind a small menu. */
function CopyTable({ table }: { table: Table }) {
  const { t } = useTranslation();
  const actions = useContext(CellActions);
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const menu = useRef<HTMLSpanElement>(null);
  // Not on blur: the terminal refocuses its editor on mouseup, which would unmount the menu before the click.
  useDismiss(open, () => menu.current, () => setOpen(false));
  const code = table.call ? actions?.callCode?.(table.call) : null;
  const copy = (label: string, text: string) => {
    void navigator.clipboard.writeText(text).then(() => { setCopied(label); setTimeout(() => setCopied(null), 1500); }).catch(() => undefined);
    setOpen(false);
  };
  return <>
    <CopyButton text={() => toTsv(table)} className="term-copy" tip={t("shell.copyTsvTip")} ariaLabel={t("shell.copyTsvLabel")} label="TSV" />
    <span className="term-table-menu" ref={menu} onMouseDown={(e) => e.preventDefault()}
      onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); setOpen(false); } }}>
      <button type="button" className="term-copy" aria-haspopup="menu" aria-expanded={open} aria-label={t("shell.moreTableActions")} data-tip={copied ? t("shell.copiedAs", { format: copied }) : t("shell.moreTableActions")}
        onClick={() => setOpen(!open)}>{copied ? <Icon name="check" /> : "⋯"}</button>
      {open && <div className="term-menu term-table-actions" role="menu">
        <button type="button" role="menuitem" onClick={() => copy("JSON", toJson(table))}>{t("shell.copyAs", { format: "JSON" })}</button>
        <button type="button" role="menuitem" onClick={() => copy("Markdown", toMarkdown(table))}>{t("shell.copyAs", { format: "Markdown" })}</button>
        {code && <button type="button" role="menuitem" onClick={() => copy("Python", code)}>{t("shell.copyAs", { format: "Python" })}</button>}
        {actions?.exportTable && <>
          <button type="button" role="menuitem" onClick={() => { setOpen(false); actions.exportTable?.(table, "csv"); }}>{t("shell.saveAs", { format: "CSV" })}</button>
          <button type="button" role="menuitem" onClick={() => { setOpen(false); actions.exportTable?.(table, "json"); }}>{t("shell.saveAs", { format: "JSON" })}</button>
        </>}
      </div>}
    </span>
  </>;
}
