import { useEffect, useRef, useState, type ChangeEvent, type ReactNode } from "react";
import { ipc, type EnvironmentDraft, type EnvironmentProxyInput, type Health, type ProxiesView } from "./ipc";
import { i18n } from "./i18n";
import HeaderInput from "./HeaderInput";
import TagEditor from "./TagEditor";
import { Icon } from "./Icon";
import Select from "./Select";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { Banner, Button, PanelHeader, cx, detail, detailHeader, field, panel, panelBody, tile } from "./ui";

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

function ConnectionFields({ idPrefix, values, change, autoFocus, databases, loadDatabases, children }: {
  idPrefix: string;
  values: ConnectionValues;
  change: (values: ConnectionValues) => void;
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
  const section = (title: string, body: ReactNode) => <section className={panel}>
    <PanelHeader><h3>{title}</h3></PanelHeader>
    <div className={cx(panelBody, "grid grid-cols-2 gap-x-3.5 gap-y-3 max-sm:grid-cols-1 pt-3.5 pb-3.5")}>{body}</div>
  </section>;
  return <>
    {section(t("environmentForm.serverTitle"), <>
        <label className={cx(field, "col-span-2 max-sm:col-auto")}><span>{t("environmentForm.odooUrl")}</span><input id={`${idPrefix}-url`} autoFocus={autoFocus} type="url" required value={values.url} placeholder="https://odoo.example.com" onChange={set("url")} onBlur={suggestName} /></label>
        <div className={cx(field, "col-span-2 max-sm:col-auto")}><span id={`${idPrefix}-database-label`}>{t("common.database")}</span>
          <span className="flex items-center gap-2">
            <span className="grow min-w-0"><HeaderInput id={`${idPrefix}-database`} aria-labelledby={`${idPrefix}-database-label`} required value={values.database} suggestions={databases ?? []} showAll caseSensitive
              aria-describedby={databases?.length !== 0 ? `${idPrefix}-database-help` : undefined} onChange={(database) => change({ ...values, database })} onBlur={() => undefined} /></span>
            <Button iconOnly disabled={!!urlError(values.url)} onClick={loadDatabases} aria-label={t("environmentForm.loadDatabases")} data-tip={t("environmentForm.loadDatabases")}><Icon name="refresh" /></Button>
          </span>
          {databases?.length !== 0 && <small className="text-fg-3 text-xs" id={`${idPrefix}-database-help`}>{databases === null ? t("environmentForm.databasesHidden") : t("environmentForm.databasesFound", { count: databases.length })}</small>}</div>
        <details className="group/advanced col-span-2 max-sm:col-auto">
          <summary className="cursor-pointer text-fg-2 text-sm group-open/advanced:mb-2">{t("environmentForm.advancedOptions")}</summary>
          <label className={field}><span>{t("environmentForm.protocol")}</span><ProtocolSelect id={`${idPrefix}-protocol`} value={values.protocol} onChange={(protocol) => change({ ...values, protocol })} /></label>
        </details>
    </>)}
    {section(t("environmentForm.accessTitle"), <>
        <label className={field}><span>{t("environmentForm.user")}</span><input id={`${idPrefix}-login`} required autoComplete="off" value={values.login} onChange={set("login")} /></label>
        <label className={field}><span>{t("environmentForm.apiKey")}</span><input id={`${idPrefix}-secret`} type="password" required autoComplete="new-password" value={values.secret} onChange={set("secret")} /></label>
    </>)}
    {section(t("environmentForm.identityTitle"), <>
        <label className={field}><span>{t("environmentForm.name")}</span><input id={`${idPrefix}-name`} required maxLength={200} value={values.name} placeholder={t("environmentForm.namePlaceholder")} onChange={set("name")} /></label>
      {children}
    </>)}
  </>;
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

/** Select value for an environment's proxy: "inherit", "direct" or "p:" + the id of a named proxy. */
export function proxyChoice(value: string): EnvironmentProxyInput {
  return value.startsWith("p:") ? { mode: "profile", id: value.slice(2) } : { mode: value === "direct" ? "direct" : "inherit" };
}
export function proxyOptions(list: ProxiesView) {
  const fallback = list.proxies.find((p) => p.id === list.default)?.name;
  return [
    { value: "inherit", label: i18n.t("environments.inheritProxy", { proxy: fallback ?? i18n.t("environments.noDefaultProxy") }) },
    ...list.proxies.map((p) => ({ value: `p:${p.id}`, label: p.name })),
    { value: "direct", label: i18n.t("environments.directProxy") },
  ];
}

export type FormDraft = { values: ConnectionValues; tags: string[]; proxy?: string };

export default function EnvironmentForm({ busy, save, cancel, tagSuggestions, draftRef }: {
  busy: boolean;
  save: (draft: EnvironmentDraft, tags: string[], proxy: EnvironmentProxyInput) => Promise<void>;
  cancel: () => void;
  tagSuggestions: string[];
  /** Keeps what was typed while the form is unmounted, e.g. when switching pages. */
  draftRef?: { current: FormDraft | null };
}) {
  const { t } = useTranslation();
  const [values, setValues] = useState<ConnectionValues>(() => draftRef?.current?.values ?? { name: "", url: "", secret: "", protocol: "auto", ...loadEnvironmentDefaults() });
  const [tags, setTags] = useState<string[]>(() => draftRef?.current?.tags ?? []);
  const [proxy, setProxy] = useState(() => draftRef?.current?.proxy ?? "inherit");
  const [proxies, setProxies] = useState<ProxiesView | null>(null);
  useEffect(() => { ipc.proxies().then(setProxies, () => setProxies({ proxies: [], default: null })); }, []);
  useEffect(() => { if (draftRef) draftRef.current = { values, tags, proxy }; }, [draftRef, values, tags, proxy]);
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
  // What a test result is about: one that ends after an edit describes values no longer in the form and is dropped.
  const tested = JSON.stringify([{ ...draft(), name: "" }, proxyChoice(proxy)]);
  const latestTested = useRef(tested);
  latestTested.current = tested;
  async function testConnection() {
    setError("");
    setTest(null);
    const invalidUrl = urlError(values.url);
    if (invalidUrl) { setError(invalidUrl); return; }
    setTesting(true);
    const asked = tested;
    // The name is irrelevant to the connection, but the draft requires one.
    try {
      const report = await ipc.checkDraft({ ...draft(), name: draft().name || "draft" }, proxyChoice(proxy));
      if (latestTested.current !== asked) return;
      setTest(report);
      if (report.status === "ok") loadDatabases();
    }
    catch (reason) { if (latestTested.current === asked) setError(normalizeAppError(reason)); }
    finally { setTesting(false); }
  }

  return <form className={detail} aria-labelledby="create-environment-title" autoComplete="off" onSubmit={async (event) => {
    event.preventDefault();
    if (busy) return;
    setError("");
    const invalidUrl = urlError(values.url);
    if (invalidUrl) { setError(invalidUrl); return; }
    try {
      await save(draft(), tags, proxyChoice(proxy));
      setValues((current) => ({ ...current, secret: "" }));
    } catch (reason) { setError(normalizeAppError(reason)); }
  }}>
    <header className={detailHeader}>
      <span className={tile()}><Icon name="plus" /></span>
      <h2 id="create-environment-title" className="grow">{t("environmentForm.createTitle")}</h2>
    </header>
    <fieldset disabled={busy} className="grid gap-3 min-w-0 m-0 p-0 border-0">
      <ConnectionFields idPrefix="environment" values={values} change={change} autoFocus databases={databases} loadDatabases={loadDatabases}>
        <div className={field}><span id="environment-proxy-label">{t("proxy.title")}</span>
          {proxies ? <Select label={t("environments.environmentProxy")} value={proxy} options={proxyOptions(proxies)} onChange={(next) => { setProxy(next); setTest(null); }} /> : <span>…</span>}</div>
        <div className={cx(field, "col-span-2 max-sm:col-auto")}><span>{t("environments.tags")}</span><TagEditor tags={tags} suggestions={tagSuggestions} disabled={busy} onChange={setTags} /></div>
      </ConnectionFields>
    </fieldset>
    {error && <Banner tone="danger" role="alert"><Icon name="alert" />{formatAppError(error)}</Banner>}
    {test && (test.status === "ok"
      ? <Banner tone="ok" role="status"><Icon name="check" />{t("environmentForm.testOk", { latency: test.latency_ms, version: test.server_version ?? "?" })}</Banner>
      : <Banner tone="danger" role="alert"><Icon name="alert" />{test.error_code ? formatAppError({ code: test.error_code, params: test.error_params ?? undefined, detail: test.error ?? undefined }) : t("health.offline")}</Banner>)}
    <footer className="flex items-center gap-2 justify-end">
      <Button disabled={busy || testing || !canTest} onClick={() => void testConnection()} data-tip={canTest ? undefined : t("environmentForm.testNeeds")}>{testing ? <Icon name="refresh" className="animate-spin" /> : <Icon name="plug" />}{t("environmentForm.testConnection")}</Button>
      <span className="grow" />
      <Button disabled={busy} onClick={cancel}>{t("common.cancel")}</Button>
      <Button type="submit" variant="primary" disabled={busy}>{busy ? t("common.saving") : t("environmentForm.createAction")}</Button>
    </footer>
  </form>;
}
