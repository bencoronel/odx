import { Fragment } from "react";
import type { Pending } from "./ipc";
import { useTranslation } from "react-i18next";

export function methodTone(method: string) {
  return method === "unlink" ? "danger" : ["create", "write"].includes(method) ? "warn" : "info";
}

const isEmpty = (value: unknown) => value == null || (typeof value === "object" && Object.keys(value).length === 0);
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
/** One line when short; indented when it would wrap. */
const show = (value: unknown) => { const line = JSON.stringify(value); return line.length > 80 ? JSON.stringify(value, null, 2) : line; };

/** Positional argument `index`, or its Odoo keyword name. */
function param(item: Pick<Pending, "args" | "kwargs">, index: number, name: string): unknown {
  return Array.isArray(item.args) && index < item.args.length ? item.args[index] : isRecord(item.kwargs) ? item.kwargs[name] : undefined;
}

/** Target record ids (first argument or kwargs.ids, Odoo's convention); null for create or unknown shapes. */
export function recordIds(item: Pick<Pending, "method" | "args" | "kwargs">): number[] | null {
  if (item.method === "create") return null;
  const ids = param(item, 0, "ids");
  return Array.isArray(ids) && ids.length > 0 && ids.every(Number.isInteger) ? ids as number[] : null;
}

/** Field values a create or write sends: one map per record. */
export function writtenValues(item: Pick<Pending, "method" | "args" | "kwargs">): Record<string, unknown>[] | null {
  const values = item.method === "write" ? param(item, 1, "vals") : item.method === "create" ? param(item, 0, "vals_list") : undefined;
  const list = Array.isArray(values) ? values : [values];
  return list.length > 0 && list.every(isRecord) ? list : null;
}

/** Records shown as tables; the raw call keeps the rest. */
const SHOWN_RECORDS = 20;

type RelationText = "create" | "update" | "delete" | "unlink" | "link" | "clear" | "set";
const RELATION_TEXT: RelationText[] = ["create", "update", "delete", "unlink", "link", "clear", "set"];
/** As in odx-core's relational_delete_count: only create, update and link never remove records. */
const DANGEROUS = new Set<RelationText>(["delete", "unlink", "clear", "set"]);

/** Odoo x2many commands ([[6, 0, [3, 4]]] and so on) as plain steps; null when the value is not a command list. */
export function relationSteps(value: unknown): { kind: RelationText; id?: unknown; values?: unknown }[] | null {
  if (!Array.isArray(value) || !value.length) return null;
  if (!value.every((c) => Array.isArray(c) && Number.isInteger(c[0]) && c[0] >= 0 && c[0] <= 6)) return null;
  return (value as unknown[][]).map(([code, id, values]) => {
    const kind = RELATION_TEXT[code as number];
    return kind === "set" ? { kind, values: id === 0 || id === false ? values : id } : kind === "create" ? { kind, values } : { kind, id, values };
  });
}

function RelationSteps({ steps }: { steps: NonNullable<ReturnType<typeof relationSteps>> }) {
  const { t } = useTranslation();
  return <ul className="relation-steps">{steps.map((step, index) => <li key={index} className={DANGEROUS.has(step.kind) ? "tone-danger" : undefined}>
    {t(`approvals.relation.${step.kind}`, { id: step.id, ids: Array.isArray(step.values) ? step.values.join(", ") : "" })}
    {(step.kind === "create" || step.kind === "update") && <code> {show(step.values)}</code>}
  </li>)}</ul>;
}

export function ValuesTable({ values }: { values: Record<string, unknown>[] }) {
  const { t } = useTranslation();
  return <div className="values-tables">
    {values.slice(0, SHOWN_RECORDS).map((record, index) => <table key={index} className="values-table">
      {values.length > 1 && <caption>{t("approvals.recordNumber", { number: index + 1 })}</caption>}
      <thead><tr><th scope="col">{t("approvals.field")}</th><th scope="col">{t("approvals.value")}</th></tr></thead>
      <tbody>{Object.entries(record).map(([field, value]) => <tr key={field}>
        <th scope="row" className="mono">{field}</th>
        <td>{relationSteps(value) ? <RelationSteps steps={relationSteps(value)!} /> : <code>{typeof value === "string" ? value : show(value)}</code>}</td>
      </tr>)}</tbody>
    </table>)}
    {values.length > SHOWN_RECORDS && <p className="hint">{t("approvals.moreRecords", { count: values.length - SHOWN_RECORDS })}</p>}
  </div>;
}

export function CallDetails({ item }: { item: Pick<Pending, "args" | "kwargs" | "context"> }) {
  const { t } = useTranslation();
  const rows = ([["arguments", item.args], ["namedArguments", item.kwargs], ["context", item.context]] as const).filter(([, value]) => !isEmpty(value));
  if (!rows.length) return null;
  return <dl className="call">
    {rows.map(([key, value]) => <Fragment key={key}><dt>{t(`approvals.${key}`)}</dt><dd><code>{show(value)}</code></dd></Fragment>)}
  </dl>;
}

/** The request as a Terminal command, to repeat or adjust it by hand. The call context is left out: the Terminal sets it per session (\\context). */
export function shellCommand(item: Pick<Pending, "model" | "method" | "args" | "kwargs">): string {
  const { context: _context, ...kwargs } = isRecord(item.kwargs) ? item.kwargs : {};
  const args = Array.isArray(item.args) ? item.args : [];
  const json = JSON.stringify;
  if (!Object.keys(kwargs).length) {
    const ids = recordIds({ ...item, kwargs: {} });
    if (item.method === "write" && ids && args.length === 2 && isRecord(args[1])) return `write ${item.model} ${ids.join(",")} ${json(args[1])}`;
    if (item.method === "unlink" && ids && args.length === 1) return `unlink ${item.model} ${ids.join(",")}`;
    if (item.method === "create" && args.length === 1) return `create ${item.model} ${json(args[0])}`;
  }
  return `call ${item.model} ${item.method} ${json(args)} ${json(kwargs)}`;
}
