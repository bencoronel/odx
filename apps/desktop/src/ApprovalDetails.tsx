import { Fragment } from "react";
import type { Pending } from "./ipc";
import { useTranslation } from "react-i18next";
import { Json } from "./json";
import { isRecord } from "./shell/commands";

export function methodTone(method: string) {
  return method === "unlink" ? "danger" : ["create", "write"].includes(method) ? "warn" : "info";
}

const isEmpty = (value: unknown) => value == null || (typeof value === "object" && Object.keys(value).length === 0);

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
    {(step.kind === "create" || step.kind === "update") && <code> <Json value={step.values} /></code>}
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
        <td>{relationSteps(value) ? <RelationSteps steps={relationSteps(value)!} /> : <code>{typeof value === "string" ? value : <Json value={value} />}</code>}</td>
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
    {rows.map(([key, value]) => <Fragment key={key}><dt>{t(`approvals.${key}`)}</dt><dd><code><Json value={value} /></code></dd></Fragment>)}
  </dl>;
}
