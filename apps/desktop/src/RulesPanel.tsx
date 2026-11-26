import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ipc, type Environment, type Pending, type Rule, type RuleAction, type RuleKind, type RuleSelector } from "./ipc";
import { Icon } from "./Icon";
import Select from "./Select";
import PanelTitle, { useCollapsed } from "./PanelTitle";
import { ConfirmDialog } from "./ConfirmDialog";
import { recordIds, writtenValues } from "./ApprovalDetails";
import { Json } from "./json";
import { formatDateTime } from "./i18n/format";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { Banner, Button, Chip, IconButton, PanelHeader, cx, field, panel, panelBody, panelSummary, valuesTable, valuesTables } from "./ui";

type Expiry = "keep" | "never" | "hour" | "day" | "week" | "month";
const EXPIRY_MS: Record<Exclude<Expiry, "keep" | "never">, number> = { hour: 3_600_000, day: 86_400_000, week: 7 * 86_400_000, month: 30 * 86_400_000 };
const KINDS: RuleKind[] = ["method", "fields", "values"];

/** A dialog's starting point: what the rule applies to (fixed) and its editable condition. `id` and `expiresAt` when editing. */
type Initial = {
  target: Pick<Pending, "environment_id" | "environment_name" | "model" | "method" | "context">;
  kind: RuleKind; fields: string; values: string; maxRecords: number; action: RuleAction; scope: "environment" | "global"; operations: string[]; regexFields: boolean; modelMode: "exact" | "all" | "regex"; methodMode: "exact" | "all"; name: string; selectors?: RuleSelector[]; id?: string; expiresAt?: string | null; expired?: boolean;
};

/** Same environment, model, method and effective context as the request; the condition starts from what it writes. */
export function fromApproval(item: Pending): Initial {
  const values = writtenValues(item);
  const payloadMethod = item.method === "create" || item.method === "write";
  const same = values && values.every((v) => JSON.stringify(v) === JSON.stringify(values[0]));
  return {
    target: item, kind: payloadMethod && values ? "fields" : "method", action: "approve", scope: "environment", operations: ["write"], regexFields: false, modelMode: "exact", methodMode: "exact", name: "",
    fields: [...new Set(values?.flatMap(Object.keys))].join(", "),
    values: same ? JSON.stringify(values[0], null, 2) : "{}",
    maxRecords: Math.max(1, recordIds(item)?.length ?? values?.length ?? 1),
  };
}

export function fromRule(rule: Rule, environmentName: string): Initial {
  const regexFields = rule.field_selectors?.some((s) => s.kind === "regex") ?? false;
  return {
    target: { ...rule, environment_name: environmentName }, kind: rule.kind, id: rule.id,
    fields: rule.field_selectors?.length ? rule.field_selectors.map((s) => s.kind === "regex" ? s.pattern : s.value).join(regexFields ? "\n" : ", ") : rule.fields?.join(", ") ?? "", values: JSON.stringify(rule.values ?? {}, null, 2),
    selectors: rule.field_selectors?.length ? rule.field_selectors : undefined,
    maxRecords: rule.max_records, expiresAt: rule.expired ? null : rule.expires_at, expired: rule.expired,
    action: rule.action ?? "approve", scope: rule.scope?.kind ?? "environment", operations: rule.operations ?? ["write"], regexFields,
    modelMode: rule.model_selector?.kind === "all" ? "all" : rule.model_selector?.kind === "regex" ? "regex" : "exact",
    methodMode: rule.method === "all" ? "all" : "exact",
    name: rule.name ?? "",
  };
}

/** Create or edit an approval or block rule. Only approval rules may also approve a pending request. */
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
  const [kind, setKind] = useState<RuleKind>(initial.kind);
  const [fields, setFields] = useState(initial.fields);
  const [valuesText, setValuesText] = useState(initial.values);
  const [maxRecords, setMaxRecords] = useState(initial.maxRecords);
  const [action, setAction] = useState<RuleAction>(initial.action);
  const [scope, setScope] = useState<"environment" | "global">(initial.scope);
  const [operations, setOperations] = useState<string[]>(initial.operations);
  const [regexFields, setRegexFields] = useState(initial.regexFields);
  const [modelMode, setModelMode] = useState<"exact" | "all" | "regex">(initial.modelMode);
  const [modelValue, setModelValue] = useState(target.model);
  const [methodMode, setMethodMode] = useState<"exact" | "all">(initial.methodMode);
  const [methodValue, setMethodValue] = useState(target.method === "all" ? "" : target.method);
  const [ruleName, setRuleName] = useState(initial.name);
  const payloadMethod = methodMode === "exact" && (methodValue === "create" || methodValue === "write");
  const [expiry, setExpiry] = useState<Expiry>(initial.expiresAt ? "keep" : initial.expired ? "day" : "never");
  const [approveToo, setApproveToo] = useState(true);
  const [busy, setBusy] = useState(false);
  let parsedValues: unknown = null;
  try { parsedValues = JSON.parse(valuesText); } catch { /* Shown as invalid below. */ }
  const valuesValid = typeof parsedValues === "object" && parsedValues !== null && !Array.isArray(parsedValues);
  const fieldList = regexFields
    ? fields.split(/\r?\n/).map((line) => line.trim()).filter(Boolean)
    : fields.split(/[\s,]+/).filter(Boolean);
  const selectors = fields === initial.fields && regexFields === initial.regexFields && initial.selectors
    ? initial.selectors : fieldList.map((value): RuleSelector => regexFields ? { kind: "regex", pattern: value } : { kind: "exact", value });
  const expiries: Expiry[] = [...(initial.expiresAt ? ["keep" as const] : []), "never", "hour", "day", "week", "month"];

  async function save() {
    setBusy(true);
    try {
      await ipc.saveRule({
        id: initial.id, environment_id: target.environment_id, model: modelMode === "all" ? "" : modelValue, model_selector: modelMode === "all" ? { kind: "all" } : modelMode === "regex" ? { kind: "regex", pattern: modelValue } : { kind: "exact", value: modelValue },
        method: methodMode === "all" ? "all" : methodValue, kind, action, scope: { kind: scope }, name: ruleName, operations: action === "block" ? operations : ["write"],
        fields: kind === "fields" && selectors.some((s) => s.kind === "exact") ? selectors.filter((s) => s.kind === "exact").map((s) => s.value) : undefined,
        field_selectors: kind === "fields" ? selectors : [],
        values: kind === "values" ? parsedValues as Record<string, unknown> : undefined,
        context: target.context, max_records: maxRecords,
        expires_at: expiry === "keep" ? initial.expiresAt : expiry === "never" ? null : new Date(Date.now() + EXPIRY_MS[expiry]).toISOString(),
      });
      onSaved?.();
      if (approve && approveToo && action === "approve") approve();
    } finally { setBusy(false); }
  }

  const row2 = "grid grid-cols-2 gap-3 max-sm:grid-cols-1";
  return <ConfirmDialog open={open} busy={busy} wide onOpenChange={onOpenChange} onConfirm={save}
    title={t(editing ? "rules.editTitle" : "rules.createTitle", { method: target.method, model: target.model })}
    description={editing ? t("rules.editDescription") : scope === "global" ? t("rules.createGlobalDescription") : t("rules.createDescription", { environment: target.environment_name })}
    confirmLabel={editing ? t("common.save") : t("rules.create")}
    confirmDisabled={(action === "approve" && maxRecords < 1) || (action === "block" && !operations.length) || (kind === "fields" && !fieldList.length) || (kind === "values" && !valuesValid) || (methodMode === "exact" && !methodValue.trim()) || (modelMode !== "all" && !modelValue.trim())}>
    <div className={row2}>
      <label className={field}><span>{t("rules.action")}</span><Select label={t("rules.action")} value={action} disabled={editing} onChange={(value) => { setAction(value as RuleAction); if (value === "approve") { setRegexFields(false); if (modelMode === "regex") setModelMode("exact"); if (!payloadMethod && kind !== "method") setKind("method"); } }} options={[{ value: "approve", label: t("rules.actions.approve") }, { value: "block", label: t("rules.actions.block") }]} /></label>
      <label className={field}><span>{t("rules.scope")}</span><Select label={t("rules.scope")} value={scope} disabled={editing} onChange={(value) => setScope(value as "environment" | "global")} options={[{ value: "environment", label: t("rules.scopes.environment") }, { value: "global", label: t("rules.scopes.global") }]} /></label>
    </div>
    <label className={field}><span>{t("rules.name")}</span><input value={ruleName} placeholder={t("rules.namePlaceholder")} onChange={(e) => setRuleName(e.target.value)} maxLength={120} /></label>
    <div className={modelMode !== "all" ? row2 : undefined}>
      <label className={field}><span>{t("rules.modelSelector")}</span><Select label={t("rules.modelSelector")} value={modelMode} onChange={(value) => setModelMode(value as "exact" | "regex" | "all")} options={[{ value: "exact", label: t("rules.selectors.exact") }, ...(action === "block" ? [{ value: "regex" as const, label: t("rules.selectors.regex") }] : []), { value: "all", label: t("rules.selectors.all") }]} /></label>
      {modelMode !== "all" && <label className={field}><span>{t("rules.modelPattern")}</span><input value={modelValue} onChange={(e) => setModelValue(e.target.value)} className="mono" /></label>}
    </div>
    <div className={methodMode === "exact" ? row2 : undefined}>
      <label className={field}><span>{t("rules.method")}</span><Select label={t("rules.method")} value={methodMode} onChange={(v) => setMethodMode(v as "exact" | "all")} options={[{ value: "exact", label: t("rules.selectors.exact") }, { value: "all", label: t("rules.methodAny") }]} /></label>
      {methodMode === "exact" && <label className={field}><span>{t("rules.methodName")}</span><input value={methodValue} onChange={(e) => setMethodValue(e.target.value)} className="mono" /></label>}
    </div>
    {(payloadMethod || action === "block") && <label className={field}><span>{t("rules.kind")}</span>
      <Select label={t("rules.kind")} value={kind} onChange={setKind} options={(action === "block" ? ["method", "fields"] as RuleKind[] : KINDS).map((value) => ({ value, label: t(`rules.kinds.${value}`) }))} />
    </label>}
    {action === "block" && <fieldset className={field}><legend className="text-sm font-medium text-fg-2">{t("rules.operations")}</legend>
      <div className="flex flex-wrap items-center gap-4 pt-0.5">
        {([{ key: "read", label: t("rules.operationsList.read") }, { key: "write", label: t("rules.operationsList.write") }, { key: "delete", label: t("rules.operationsList.delete") }]).map(({ key: op, label }) =>
          <label key={op} className="inline-flex items-center gap-1.5 text-sm text-fg cursor-pointer"><input type="checkbox" checked={operations.includes(op)} onChange={() => setOperations((old) => old.includes(op) ? old.filter((v) => v !== op) : [...old, op])} />{label}</label>)}
      </div>
    </fieldset>}
    {kind === "fields" && <label className={field}><span>{t("rules.fields")}</span>
      {regexFields
        ? <textarea rows={3} value={fields} onChange={(e) => setFields(e.target.value)} className="mono" />
        : <input value={fields} onChange={(e) => setFields(e.target.value)} className="mono" />}
      <small className="text-fg-3 text-xs">{t(regexFields ? "rules.regexExample" : "rules.fieldsHint")}</small>
      {action === "block" && <label className="inline-flex items-center gap-1.5 text-sm text-fg-2 cursor-pointer mt-0.5"><input type="checkbox" checked={regexFields} onChange={() => setRegexFields(!regexFields)} />{t("rules.regexFields")}</label>}
    </label>}
    {kind === "values" && <label className={field}><span>{t("rules.values")}</span>
      <textarea rows={5} value={valuesText} onChange={(e) => setValuesText(e.target.value)} />
      {!valuesValid && <small className="text-danger text-xs">{t("rules.valuesInvalid")}</small>}
    </label>}
    <div className={action === "approve" ? row2 : undefined}>
      {action === "approve" && <label className={field}><span>{t("rules.maxRecords")}</span>
        <input type="number" min={1} max={1000} value={maxRecords} onChange={(e) => setMaxRecords(Number(e.target.value))} />
      </label>}
      <label className={field}><span>{t("rules.expires")}</span>
        <Select label={t("rules.expires")} value={expiry} onChange={setExpiry}
          options={expiries.map((value) => ({ value, label: value === "keep" ? t("rules.until", { time: formatDateTime(new Date(initial.expiresAt!), { dateStyle: "short", timeStyle: "short" }) }) : t(`rules.expiry.${value}`) }))} />
      </label>
    </div>
    {action === "approve" && <div className={field}><span>{t("rules.context")}</span><code className="mono"><Json value={target.context} /></code></div>}
    {approve && action === "approve" && <label className="flex gap-2 items-start text-fg-2 text-sm cursor-pointer"><input type="checkbox" checked={approveToo} onChange={() => setApproveToo(!approveToo)} />{t("rules.approveToo")}</label>}
  </ConfirmDialog>;
}

/** Rules for an environment, or global rules only when `env` is omitted (Settings → Security). */
export default function RulesPanel({ env, onCount }: { env?: Environment; onCount?: (count: number) => void }) {
  const { t } = useTranslation();
  const globalOnly = !env;
  const [rules, setRules] = useState<Rule[] | null>(null);
  const [error, setError] = useState<ErrorState>("");
  const [collapsed, setCollapsed] = useCollapsed(globalOnly ? "global-rules" : "rules", false);
  const load = () => ipc.rules(env?.id).then((all) => {
    const filtered = globalOnly ? all.filter((r) => r.scope?.kind === "global") : all;
    setRules([...filtered].sort((a, b) => b.created_at.localeCompare(a.created_at)));
    onCount?.(filtered.length);
  }, (reason) => setError(normalizeAppError(reason)));
  useEffect(() => { setRules(null); setError(""); void load(); }, [env?.id]); // eslint-disable-line react-hooks/exhaustive-deps
  const [editing, setEditing] = useState<Rule | null>(null);
  const [deleting, setDeleting] = useState<Rule | null>(null);
  const [busyDelete, setBusyDelete] = useState(false);
  const [creating, setCreating] = useState(false);
  const newRule: Initial = { target: { environment_id: env?.id ?? "", environment_name: env?.name ?? "", model: "", method: "all", context: {} }, kind: "method", fields: "", values: "{}", maxRecords: 1, action: "block", scope: "global", operations: ["read"], regexFields: true, modelMode: "all", methodMode: "all", name: "" };
  const condition = (rule: Rule) => rule.kind === "fields"
    ? (rule.field_selectors?.length
      ? rule.field_selectors.map((s) => s.kind === "regex" ? `/${s.pattern}/` : s.value).join(", ")
      : rule.fields?.join(", "))
    : rule.kind === "values" ? <Json value={rule.values} /> : t("rules.kinds.method");
  const title = (rule: Rule) => {
    if (rule.name) return rule.name;
    const method = rule.method === "all" ? t("rules.methodAny") : rule.method;
    const model = rule.model_selector?.kind === "all" ? t("rules.selectors.all")
      : rule.model_selector?.kind === "regex" ? `/${rule.model_selector.pattern}/`
      : rule.model_selector?.kind === "exact" ? rule.model_selector.value
      : rule.model;
    return `${method} ${model}`;
  };
  const heading = t(globalOnly ? "rules.globalTitle" : "rules.title");
  const emptyText = t(globalOnly ? "rules.globalEmpty" : "rules.empty");

  return <section className={panel} aria-label={heading}>
    <PanelHeader className={collapsed && rules ? "flex-wrap gap-y-1" : undefined}>
      <div className="flex-auto min-w-0"><PanelTitle title={heading} collapsed={collapsed} toggle={() => setCollapsed(!collapsed)} /></div>
      {collapsed && rules && <span className={panelSummary}>{rules.length ? t("rules.count", { count: rules.length }) : emptyText}</span>}
      {!collapsed && <Button size="sm" onClick={() => setCreating(true)}><Icon name="plus" />{t("rules.create")}</Button>}
    </PanelHeader>
    {!collapsed && <>
      {error && <div className={panelBody}><Banner tone="danger" compact role="alert"><Icon name="alert" />{formatAppError(error)}</Banner></div>}
      {rules && !rules.length && <p className={cx(panelBody, "text-fg-3 text-sm")}>{emptyText}</p>}
      {rules && rules.length > 0 && <div className={valuesTables}><table className={valuesTable}>
        <tbody>{rules.map((rule) => {
          const isBlock = (rule.action ?? "approve") === "block";
          const isGlobal = (rule.scope?.kind ?? "environment") === "global";
          return <tr key={rule.id} className={rule.expired ? "opacity-55" : undefined}>
            <th scope="row" className="mono">{title(rule)}</th>
            <td>
              <div className="flex flex-wrap items-center gap-1.5">
                <Chip tone={isBlock ? "danger" : "ok"}>{t(isBlock ? "rules.badges.block" : "rules.badges.approve")}</Chip>
                {!globalOnly && <Chip tone={isGlobal ? "info" : undefined}>{t(`rules.scopes.${isGlobal ? "global" : "environment"}`)}</Chip>}
                <code>{condition(rule)}</code>
              </div>
              <small className="block text-fg-3 text-xs mt-1">
                {!isBlock ? `${t("rules.records", { count: rule.max_records })} · ` : ""}
                {rule.expired ? t("rules.expired")
                  : rule.expires_at ? t("rules.until", { time: formatDateTime(new Date(rule.expires_at), { dateStyle: "short", timeStyle: "short" }) }) : t("rules.expiry.never")}
              </small>
            </td>
            <td className="w-[1%] whitespace-nowrap text-right!"><span className="inline-flex items-center gap-1">
              <IconButton size="sm" aria-label={t("rules.edit")} data-tip={t("rules.edit")} onClick={() => setEditing(rule)}><Icon name="pencil" /></IconButton>
              <IconButton size="sm" aria-label={t("rules.delete")} data-tip={t("rules.delete")} onClick={() => setDeleting(rule)}><Icon name="trash" /></IconButton>
            </span></td>
          </tr>;
        })}</tbody>
      </table></div>}
    </>}
    {editing && <RuleDialog key={editing.id} initial={fromRule(editing, env?.name ?? "")} open onOpenChange={(open) => { if (!open) setEditing(null); }} onSaved={() => void load()} />}
    {creating && <RuleDialog initial={newRule} open onOpenChange={(open) => { if (!open) setCreating(false); }} onSaved={() => { setCreating(false); void load(); }} />}
    <ConfirmDialog
      open={deleting !== null}
      busy={busyDelete}
      destructive
      title={t("rules.deleteTitle")}
      description={deleting ? t("rules.deleteDescription", { name: title(deleting) }) : ""}
      confirmLabel={t("rules.delete")}
      onOpenChange={(open) => { if (!open && !busyDelete) setDeleting(null); }}
      onConfirm={async () => {
        if (!deleting) return;
        setBusyDelete(true);
        try { await ipc.deleteRule(deleting.id); await load(); setDeleting(null); }
        finally { setBusyDelete(false); }
      }}
    />
  </section>;
}
