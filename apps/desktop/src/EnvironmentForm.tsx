import { useState, type ChangeEvent } from "react";
import type { EnvironmentDraft } from "./ipc";
import { Icon } from "./Icon";
import Select from "./Select";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";

export type Protocol = EnvironmentDraft["protocol"];
export type ConnectionValues = { name: string; url: string; database: string; login: string; secret: string; protocol: Protocol };

/** Stable URL validation error, or null if valid. */
export function urlError(url: string): ErrorState | null {
  try {
    const parsed = new URL(url.trim());
    if (["http:", "https:"].includes(parsed.protocol) && !parsed.username && !parsed.password && !parsed.search && !parsed.hash) return null;
  } catch { /* Se informa abajo. */ }
  return { code: "validation.environment.url_invalid" };
}

export function connectionComplete(values: ConnectionValues, secretRequired: boolean) {
  return !!(values.name.trim() && values.url.trim() && values.database.trim() && values.login.trim() && (!secretRequired || values.secret));
}

const protocolLabelKeys = { auto: "environmentForm.protocols.auto", legacy_jsonrpc: "environmentForm.protocols.legacy_jsonrpc", xmlrpc: "environmentForm.protocols.xmlrpc", json2: "environmentForm.protocols.json2" } as const;
export const protocolLabels: Record<string, string> = protocolLabelKeys;
const protocolOptions = (Object.keys(protocolLabelKeys) as Protocol[]).map((protocol) => ({ value: protocol, label: protocolLabelKeys[protocol] }));

/** Changing the protocol invalidates authentication and pending approvals. */
export function ProtocolSelect({ id, className, value, onChange }: { id?: string; className?: string; value: Protocol; onChange: (protocol: Protocol) => void }) {
  const { t } = useTranslation();
  return <Select id={id} className={className} label={t("environmentForm.protocol")} value={value} options={protocolOptions.map((option) => ({ ...option, label: t(option.label) }))} onChange={onChange} />;
}

export function ConnectionFields({ idPrefix, values, change, editing, autoFocus }: {
  idPrefix: string;
  values: ConnectionValues;
  change: (values: ConnectionValues) => void;
  /** An empty credential preserves the saved value when editing. */
  editing: boolean;
  autoFocus?: boolean;
}) {
  const { t } = useTranslation();
  const set = (key: keyof ConnectionValues) => (event: ChangeEvent<HTMLInputElement>) => change({ ...values, [key]: event.target.value });
  return <div className="form-grid">
    <label className="field"><span>{t("environmentForm.name")}</span><input id={`${idPrefix}-name`} autoFocus={autoFocus} required maxLength={200} value={values.name} placeholder={t("environmentForm.namePlaceholder")} aria-describedby={`${idPrefix}-name-help`} onChange={set("name")} /><small className="hint" id={`${idPrefix}-name-help`}>{t("environmentForm.nameHelp")}</small></label>
    <label className="field"><span>{t("environmentForm.odooUrl")}</span><input id={`${idPrefix}-url`} type="url" required value={values.url} placeholder="https://odoo.example.com" onChange={set("url")} /></label>
    <label className="field"><span>{t("common.database")}</span><input id={`${idPrefix}-database`} required value={values.database} onChange={set("database")} /></label>
    <label className="field"><span>{t("environmentForm.user")}</span><input id={`${idPrefix}-login`} required autoComplete="off" value={values.login} onChange={set("login")} /></label>
    <label className="field"><span>{t("environmentForm.apiKey")}</span><input id={`${idPrefix}-secret`} type="password" required={!editing} autoComplete="new-password" value={values.secret} placeholder={editing ? t("environmentForm.unchanged") : undefined} aria-describedby={`${idPrefix}-secret-help`} onChange={set("secret")} /><small className="hint" id={`${idPrefix}-secret-help`}>{editing ? t("environmentForm.keepCredentialHint") : t("environmentForm.credentialHint")}</small></label>
    <details className="protocol-options span-2">
      <summary>{t("environmentForm.advancedOptions")}</summary>
      <label className="field"><span>{t("environmentForm.protocol")}</span><ProtocolSelect id={`${idPrefix}-protocol`} value={values.protocol} onChange={(protocol) => change({ ...values, protocol })} /></label>
    </details>
  </div>;
}

export default function EnvironmentForm({ busy, save, cancel }: {
  busy: boolean;
  save: (draft: EnvironmentDraft) => Promise<void>;
  cancel: () => void;
}) {
  const { t } = useTranslation();
  const [values, setValues] = useState<ConnectionValues>({ name: "", url: "", database: "", login: "", secret: "", protocol: "auto" });
  const [error, setError] = useState<ErrorState>("");

  return <form className="detail" aria-labelledby="create-environment-title" autoComplete="off" onSubmit={async (event) => {
    event.preventDefault();
    if (busy) return;
    setError("");
    const invalidUrl = urlError(values.url);
    if (invalidUrl) { setError(invalidUrl); return; }
    try {
      await save({ name: values.name.trim(), url: values.url.trim(), database: values.database.trim(), login: values.login.trim(), secret: values.secret, protocol: values.protocol, context: {} });
      setValues((current) => ({ ...current, secret: "" }));
    } catch (reason) { setError(normalizeAppError(reason)); }
  }}>
    <header className="detail-header">
      <span className="tile"><Icon name="plus" /></span>
      <div className="grow"><h2 id="create-environment-title">{t("environmentForm.createTitle")}</h2><p className="hint">{t("environmentForm.readOnlyHint")}</p></div>
    </header>
    <fieldset disabled={busy} className="panel">
      <header className="panel-header"><h3>{t("environmentForm.connection")}</h3></header>
      <div className="panel-body"><ConnectionFields idPrefix="environment" values={values} change={setValues} editing={false} autoFocus /></div>
    </fieldset>
    {error && <p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p>}
    <footer className="row gap-2 justify-end">
      <button type="button" className="btn" disabled={busy} onClick={cancel}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={busy || !connectionComplete(values, true)}>{busy ? t("common.saving") : t("environmentForm.createAction")}</button>
    </footer>
  </form>;
}
