import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ipc, type Environment, type Pending, type Rule, type RuleKind } from "./ipc";
import { Icon } from "./Icon";
import Select from "./Select";
import { ConfirmDialog } from "./ConfirmDialog";
import { recordIds, writtenValues } from "./ApprovalDetails";
import { Json } from "./json";
import { formatDateTime } from "./i18n/format";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { Banner, IconButton, PanelHeader, cx, field, panel, panelBody, valuesTable, valuesTables } from "./ui";

type Expiry = "keep" | "never" | "hour" | "day" | "week" | "month";
const EXPIRY_MS: Record<Exclude<Expiry, "keep" | "never">, number> = { hour: 3_600_000, day: 86_400_000, week: 7 * 86_400_000, month: 30 * 86_400_000 };
const KINDS: RuleKind[] = ["method", "fields", "values"];

/** A dialog's starting point: what the rule applies to (fixed) and its editable condition. `id` and `expiresAt` when editing. */
type Initial = {
  target: Pick<Pending, "environment_id" | "environment_name" | "model" | "method" | "context">;
  kind: RuleKind; fields: string; values: string; maxRecords: number; id?: string; expiresAt?: string | null; expired?: boolean;
};

/** Same environment, model, method and effective context as the request; the condition starts from what it writes. */
export function fromApproval(item: Pending): Initial {
  const values = writtenValues(item);
  const payloadMethod = item.method === "create" || item.method === "write";
  const same = values && values.every((v) => JSON.stringify(v) === JSON.stringify(values[0]));
  return {
    target: item, kind: payloadMethod && values ? "fields" : "method",
    fields: [...new Set(values?.flatMap(Object.keys))].join(", "),
    values: same ? JSON.stringify(values[0], null, 2) : "{}",
    maxRecords: Math.max(1, recordIds(item)?.length ?? values?.length ?? 1),
  };
}

export function fromRule(rule: Rule, environmentName: string): Initial {
  return {
    target: { ...rule, environment_name: environmentName }, kind: rule.kind, id: rule.id,
    fields: rule.fields?.join(", ") ?? "", values: JSON.stringify(rule.values ?? {}, null, 2),
    maxRecords: rule.max_records, expiresAt: rule.expired ? null : rule.expires_at, expired: rule.expired,
  };
}

/** Create or edit an auto-approval rule. `approve`: creating from a pending request can also approve that request. */
export function RuleDialog({ initial, open, onOpenChange, approve, onSaved }: {
  initial: Initial;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  approve?: () => void;
  onSaved?: () => void;
}) {
  const { t } = useTranslation();
  const { target } = initial;
  const editing = !!initial.id;
  const payloadMethod = target.method === "create" || target.method === "write";
  const [kind, setKind] = useState<RuleKind>(initial.kind);
  const [fields, setFields] = useState(initial.fields);
  const [valuesText, setValuesText] = useState(initial.values);
  const [maxRecords, setMaxRecords] = useState(initial.maxRecords);
  const [expiry, setExpiry] = useState<Expiry>(initial.expiresAt ? "keep" : initial.expired ? "day" : "never");
  const [approveToo, setApproveToo] = useState(true);
  const [busy, setBusy] = useState(false);
  let parsedValues: unknown = null;
  try { parsedValues = JSON.parse(valuesText); } catch { /* Shown as invalid below. */ }
  const valuesValid = typeof parsedValues === "object" && parsedValues !== null && !Array.isArray(parsedValues);
  const fieldList = fields.split(/[\s,]+/).filter(Boolean);
  const expiries: Expiry[] = [...(initial.expiresAt ? ["keep" as const] : []), "never", "hour", "day", "week", "month"];

  async function save() {
    setBusy(true);
    try {
      await ipc.saveRule({
        id: initial.id, environment_id: target.environment_id, model: target.model, method: target.method, kind,
        fields: kind === "fields" ? fieldList : undefined,
        values: kind === "values" ? parsedValues as Record<string, unknown> : undefined,
        context: target.context, max_records: maxRecords,
        expires_at: expiry === "keep" ? initial.expiresAt : expiry === "never" ? null : new Date(Date.now() + EXPIRY_MS[expiry]).toISOString(),
      });
      onSaved?.();
      if (approve && approveToo) approve();
    } finally { setBusy(false); }
  }

  return <ConfirmDialog open={open} busy={busy} onOpenChange={onOpenChange} onConfirm={save}
    title={t(editing ? "rules.editTitle" : "rules.createTitle", { method: target.method, model: target.model })}
    description={editing ? t("rules.editDescription") : t("rules.createDescription", { environment: target.environment_name })}
    confirmLabel={editing ? t("common.save") : t("rules.create")}
    confirmDisabled={maxRecords < 1 || (kind === "fields" && !fieldList.length) || (kind === "values" && !valuesValid)}>
    {payloadMethod && <label className={field}><span>{t("rules.kind")}</span>
      <Select label={t("rules.kind")} value={kind} onChange={setKind} options={KINDS.map((value) => ({ value, label: t(`rules.kinds.${value}`) }))} />
    </label>}
    {kind === "fields" && <label className={field}><span>{t("rules.fields")}</span>
      <input value={fields} onChange={(e) => setFields(e.target.value)} className="mono" />
      <small className="text-fg-3 text-xs">{t("rules.fieldsHint")}</small>
    </label>}
    {kind === "values" && <label className={field}><span>{t("rules.values")}</span>
      <textarea rows={6} value={valuesText} onChange={(e) => setValuesText(e.target.value)} />
      {!valuesValid && <small className="text-danger text-xs">{t("rules.valuesInvalid")}</small>}
    </label>}
    <div className="flex items-center gap-2">
      <label className={cx(field, "grow")}><span>{t("rules.maxRecords")}</span>
        <input type="number" min={1} max={1000} value={maxRecords} onChange={(e) => setMaxRecords(Number(e.target.value))} />
      </label>
      <label className={cx(field, "grow")}><span>{t("rules.expires")}</span>
        <Select label={t("rules.expires")} value={expiry} onChange={setExpiry}
          options={expiries.map((value) => ({ value, label: value === "keep" ? t("rules.until", { time: formatDateTime(new Date(initial.expiresAt!), { dateStyle: "short", timeStyle: "short" }) }) : t(`rules.expiry.${value}`) }))} />
      </label>
    </div>
    <div className={field}><span>{t("rules.context")}</span><code className="mono"><Json value={target.context} /></code></div>
    {approve && <label className="flex gap-2 items-start text-fg-2 text-sm cursor-pointer"><input type="checkbox" checked={approveToo} onChange={() => setApproveToo(!approveToo)} />{t("rules.approveToo")}</label>}
  </ConfirmDialog>;
}

/** The environment's rules, newest first; a rule is created from a pending approval and can only be deleted here. */
export default function RulesPanel({ env }: { env: Environment }) {
  const { t } = useTranslation();
  const [rules, setRules] = useState<Rule[] | null>(null);
  const [error, setError] = useState<ErrorState>("");
  const load = () => ipc.rules(env.id).then((all) => setRules([...all].sort((a, b) => b.created_at.localeCompare(a.created_at))), (reason) => setError(normalizeAppError(reason)));
  useEffect(() => { setRules(null); setError(""); void load(); }, [env.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const remove = (id: string) => ipc.deleteRule(id).then(load, (reason) => setError(normalizeAppError(reason)));
  const [editing, setEditing] = useState<Rule | null>(null);
  const condition = (rule: Rule) => rule.kind === "fields" ? rule.fields?.join(", ") : rule.kind === "values" ? <Json value={rule.values} /> : t("rules.kinds.method");

  return <section className={panel} aria-label={t("rules.title")}>
    <PanelHeader><h3>{t("rules.title")}</h3>
      {env.write_mode !== "approval" && <span className="text-fg-3 text-xs">{t("rules.onlyApproval")}</span>}
    </PanelHeader>
    {error && <Banner tone="danger" compact role="alert"><Icon name="alert" />{formatAppError(error)}</Banner>}
    {rules && !rules.length && <p className={cx(panelBody, "text-fg-3 text-sm")}>{t("rules.empty")}</p>}
    {rules && rules.length > 0 && <div className={valuesTables}><table className={valuesTable}>
      <tbody>{rules.map((rule) => <tr key={rule.id} className={rule.expired ? "opacity-55" : undefined}>
        <th scope="row" className="mono">{rule.method} {rule.model}</th>
        <td><code>{condition(rule)}</code>
          <small className="text-fg-3 text-xs"> · {t("rules.records", { count: rule.max_records })} · {rule.expired ? t("rules.expired")
            : rule.expires_at ? t("rules.until", { time: formatDateTime(new Date(rule.expires_at), { dateStyle: "short", timeStyle: "short" }) }) : t("rules.expiry.never")}</small>
        </td>
        <td className="w-[1%] text-right!"><IconButton size="sm" aria-label={t("rules.edit")} data-tip={t("rules.edit")} onClick={() => setEditing(rule)}><Icon name="pencil" /></IconButton>
          <IconButton size="sm" aria-label={t("rules.delete")} data-tip={t("rules.delete")} onClick={() => void remove(rule.id)}><Icon name="trash" /></IconButton></td>
      </tr>)}</tbody>
    </table></div>}
    {editing && <RuleDialog key={editing.id} initial={fromRule(editing, env.name)} open onOpenChange={(open) => { if (!open) setEditing(null); }} onSaved={() => void load()} />}
  </section>;
}
