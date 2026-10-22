import { useEffect, useRef, useState, type ChangeEvent, type ReactNode } from "react";
import { ipc, type EnvironmentDraft, type Health } from "./ipc";
import HeaderInput from "./HeaderInput";
import TagEditor from "./TagEditor";
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

export function ConnectionFields({ idPrefix, values, change, editing, autoFocus, databases, loadDatabases, children }: {
  idPrefix: string;
  values: ConnectionValues;
  change: (values: ConnectionValues) => void;
  /** An empty credential preserves the saved value when editing. */
  editing: boolean;
  autoFocus?: boolean;
  /** Databases the server lists; `null` once it refused to list them. */
  databases: string[] | null;
  loadDatabases: () => void;
  /** Extra fields before the advanced options. */
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  const set = (key: keyof ConnectionValues) => (event: ChangeEvent<HTMLInputElement>) => change({ ...values, [key]: event.target.value });
  // The URL comes first because the database list depends on it; the host is a sensible default name.
  const suggestName = () => { if (!values.name.trim() && !urlError(values.url)) change({ ...values, name: new URL(values.url.trim()).host }); };
  return <div className="form-grid">
    <label className="field span-2"><span>{t("environmentForm.odooUrl")}</span><input id={`${idPrefix}-url`} autoFocus={autoFocus} type="url" required value={values.url} placeholder="https://odoo.example.com" onChange={set("url")} onBlur={suggestName} /></label>
    <div className="field"><span id={`${idPrefix}-database-label`}>{t("common.database")}</span>
      <span className="row gap-2">
        <span className="grow min-w-0"><HeaderInput id={`${idPrefix}-database`} aria-labelledby={`${idPrefix}-database-label`} required value={values.database} suggestions={databases ?? []} showAll caseSensitive
          aria-describedby={`${idPrefix}-database-help`} onChange={(database) => change({ ...values, database })} onBlur={() => undefined} /></span>
        <button type="button" className="btn icon-only" disabled={!!urlError(values.url)} onClick={loadDatabases} aria-label={t("environmentForm.loadDatabases")} data-tip={t("environmentForm.loadDatabases")}><Icon name="refresh" /></button>
      </span>
      <small className="hint" id={`${idPrefix}-database-help`}>{databases === null ? t("environmentForm.databasesHidden") : databases.length ? t("environmentForm.databasesFound", { count: databases.length }) : t("environmentForm.databasesHint")}</small></div>
    <label className="field"><span>{t("environmentForm.user")}</span><input id={`${idPrefix}-login`} required autoComplete="off" value={values.login} onChange={set("login")} /></label>
    <label className="field"><span>{t("environmentForm.apiKey")}</span><input id={`${idPrefix}-secret`} type="password" required={!editing} autoComplete="new-password" value={values.secret} placeholder={editing ? t("environmentForm.unchanged") : undefined} aria-describedby={`${idPrefix}-secret-help`} onChange={set("secret")} /><small className="hint" id={`${idPrefix}-secret-help`}>{editing ? t("environmentForm.keepCredentialHint") : t("environmentForm.credentialHint")}</small></label>
    <label className="field"><span>{t("environmentForm.name")}</span><input id={`${idPrefix}-name`} required maxLength={200} value={values.name} placeholder={t("environmentForm.namePlaceholder")} aria-describedby={`${idPrefix}-name-help`} onChange={set("name")} /><small className="hint" id={`${idPrefix}-name-help`}>{t("environmentForm.nameHelp")}</small></label>
    {children}
    <details className="protocol-options span-2">
      <summary>{t("environmentForm.advancedOptions")}</summary>
      <label className="field"><span>{t("environmentForm.protocol")}</span><ProtocolSelect id={`${idPrefix}-protocol`} value={values.protocol} onChange={(protocol) => change({ ...values, protocol })} /></label>
    </details>
  </div>;
}

/** Prefilled login and database for new environments, saved per device. */
export type EnvironmentDefaults = { login: string; database: string };
const DEFAULTS_KEY = "odx.environmentDefaults";
export function loadEnvironmentDefaults(): EnvironmentDefaults {
  try {
    const stored = JSON.parse(localStorage.getItem(DEFAULTS_KEY) ?? "{}");
    return { login: typeof stored.login === "string" ? stored.login : "", database: typeof stored.database === "string" ? stored.database : "" };
  } catch { return { login: "", database: "" }; }
}
export function saveEnvironmentDefaults(defaults: EnvironmentDefaults) {
  try { localStorage.setItem(DEFAULTS_KEY, JSON.stringify(defaults)); } catch { /* Optional preference. */ }
}

export type FormDraft = { values: ConnectionValues; tags: string[] };

export default function EnvironmentForm({ busy, save, cancel, tagSuggestions, draftRef }: {
  busy: boolean;
  save: (draft: EnvironmentDraft, tags: string[]) => Promise<void>;
  cancel: () => void;
  tagSuggestions: string[];
  /** Keeps what was typed while the form is unmounted, e.g. when switching pages. */
  draftRef?: { current: FormDraft | null };
}) {
  const { t } = useTranslation();
  const [values, setValues] = useState<ConnectionValues>(() => draftRef?.current?.values ?? { name: "", url: "", secret: "", protocol: "auto", ...loadEnvironmentDefaults() });
  const [tags, setTags] = useState<string[]>(() => draftRef?.current?.tags ?? []);
  useEffect(() => { if (draftRef) draftRef.current = { values, tags }; }, [draftRef, values, tags]);
  const [error, setError] = useState<ErrorState>("");
  const [testing, setTesting] = useState(false);
  const [test, setTest] = useState<Health | null>(null);
  const [databases, setDatabases] = useState<string[] | null>([]);
  const latestUrl = useRef("");
  latestUrl.current = values.url.trim();
  const draft = (): EnvironmentDraft => ({ name: values.name.trim(), url: values.url.trim(), database: values.database.trim(), login: values.login.trim(), secret: values.secret, protocol: values.protocol, context: {} });
  const change = (next: ConnectionValues) => {
    setValues(next);
    setTest(null);
    if (next.url.trim() !== values.url.trim()) setDatabases([]);
  };
  // Only on request: it sends the global headers to the typed URL. Late answers for an older URL are dropped.
  function loadDatabases() {
    const url = values.url.trim();
    ipc.databases(url).then((names) => { if (latestUrl.current === url) setDatabases(names); }, () => { if (latestUrl.current === url) setDatabases(null); });
  }
  const canTest = !!(values.url.trim() && values.database.trim() && values.login.trim() && values.secret);
  async function testConnection() {
    setError("");
    setTest(null);
    const invalidUrl = urlError(values.url);
    if (invalidUrl) { setError(invalidUrl); return; }
    setTesting(true);
    // The name is irrelevant to the connection, but the draft requires one.
    try {
      const report = await ipc.checkDraft({ ...draft(), name: draft().name || "draft" });
      setTest(report);
      if (report.status === "ok") loadDatabases();
    }
    catch (reason) { setError(normalizeAppError(reason)); }
    finally { setTesting(false); }
  }

  return <form className="detail" aria-labelledby="create-environment-title" autoComplete="off" onSubmit={async (event) => {
    event.preventDefault();
    if (busy) return;
    setError("");
    const invalidUrl = urlError(values.url);
    if (invalidUrl) { setError(invalidUrl); return; }
    try {
      await save(draft(), tags);
      setValues((current) => ({ ...current, secret: "" }));
    } catch (reason) { setError(normalizeAppError(reason)); }
  }}>
    <header className="detail-header">
      <span className="tile"><Icon name="plus" /></span>
      <div className="grow"><h2 id="create-environment-title">{t("environmentForm.createTitle")}</h2><p className="hint">{t("environmentForm.readOnlyHint")}</p></div>
    </header>
    <fieldset disabled={busy} className="panel">
      <header className="panel-header"><h3>{t("environmentForm.connection")}</h3></header>
      <div className="panel-body"><ConnectionFields idPrefix="environment" values={values} change={change} editing={false} autoFocus databases={databases} loadDatabases={loadDatabases}>
        <div className="field span-2"><span>{t("environments.tags")}</span><TagEditor tags={tags} suggestions={tagSuggestions} disabled={busy} onChange={setTags} /></div>
      </ConnectionFields></div>
    </fieldset>
    {error && <p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p>}
    {test && (test.status === "ok"
      ? <p className="banner ok" role="status"><Icon name="check" />{t("environmentForm.testOk", { latency: test.latency_ms, version: test.server_version ?? "?" })}</p>
      : <p className="banner danger" role="alert"><Icon name="alert" />{test.error_code ? formatAppError({ code: test.error_code, params: test.error_params ?? undefined, detail: test.error ?? undefined }) : t("health.offline")}</p>)}
    <footer className="row gap-2 justify-end">
      <button type="button" className="btn" disabled={busy || testing || !canTest} onClick={() => void testConnection()} data-tip={canTest ? undefined : t("environmentForm.testNeeds")}>{testing ? <Icon name="refresh" className="spin" /> : <Icon name="plug" />}{t("environmentForm.testConnection")}</button>
      <span className="grow" />
      <button type="button" className="btn" disabled={busy} onClick={cancel}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={busy}>{busy ? t("common.saving") : t("environmentForm.createAction")}</button>
    </footer>
  </form>;
}
