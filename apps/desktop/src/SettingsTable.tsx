import { contextKeys, formatContextValue, parseContextValue } from "./contextValues";
import PanelTitle, { useCollapsed } from "./PanelTitle";
import HeaderInput, { headerNames, headerValues, reservedHeaderNames } from "./HeaderInput";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { ipc, type HeaderEntry, type ContextEntry } from "./ipc";
import { Icon } from "./Icon";
import { ApprovalDialog } from "@/components/ui/approval-dialog";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { preventUnloadUntil, usePreventUnload } from "./windowState";

/** Returning the backend placeholder preserves sensitive header values. */
export const HEADER_MASK = "••••••••••••";

const emptyHeader = (): HeaderEntry => ({ name: "", value: "", enabled: true });
const defaults: HeaderEntry[] = [
  { name: "Accept", value: "*/*", enabled: true },
  { name: "Content-Type", value: "application/json", enabled: true },
];
function combine(base: HeaderEntry[], overrides: HeaderEntry[]) {
  return [...base.map((h) => overrides.find((o) => o.name.toLowerCase() === h.name.toLowerCase()) ?? h),
    ...overrides.filter((h) => !base.some((b) => b.name.toLowerCase() === h.name.toLowerCase()))];
}

type TableSettings = { entries: HeaderEntry[]; inherited: HeaderEntry[] };
const contextRows = (entries: ContextEntry[]): HeaderEntry[] => entries.map((entry) => ({ ...entry, value: formatContextValue(entry.value) }));

export default function SettingsTable({ kind, environmentId, pendingCount = 0, onSaved }: { kind: "headers" | "context"; environmentId?: string; pendingCount?: number; onSaved?: () => void | Promise<void> }) {
  const { t } = useTranslation();
  const headerMode = kind === "headers";
  const title = headerMode ? (environmentId ? t("settingsTable.headers") : t("settingsTable.globalHeaders")) : (environmentId ? t("settingsTable.context") : t("settingsTable.globalContext"));
  const sameName = (a: string, b: string) => headerMode ? a.toLowerCase() === b.toLowerCase() : a === b;
  const [settings, setSettings] = useState<TableSettings | null>(null);
  const [rows, setRows] = useState<HeaderEntry[] | null>(null);
  const pending = useRef(Promise.resolve());
  const lastQueued = useRef<string | null>(null);
  const version = useRef(0);
  const mounted = useRef(true);
  const [saveStatus, setSaveStatus] = useState<"common.saving" | "">("");
  const [showHidden, setShowHidden] = useState(false);
  const [collapsed, setCollapsed] = useCollapsed(kind);
  const folded = Boolean(environmentId && collapsed);
  const [error, setError] = useState<ErrorState>("");
  const [confirm, setConfirm] = useState<{ headers: HeaderEntry[]; signature: string } | null>(null);
  const confirmed = useRef(false);
  useEffect(() => {
    let active = true;
    mounted.current = true;
    const load: Promise<TableSettings> = headerMode
      ? ipc.headers(environmentId).then((value) => ({ entries: value.headers, inherited: value.inherited }))
      : ipc.context(environmentId).then((value) => ({ entries: contextRows(value.entries), inherited: contextRows(Object.entries(value.inherited).map(([name, value]) => ({ name, value, enabled: true }))) }));
    void load.then((value) => { if (active) setSettings(value); }).catch((reason) => { if (active) setError(normalizeAppError(reason)); });
    return () => { active = false; mounted.current = false; };
  }, [environmentId, headerMode]);
  const inherited = headerMode ? combine(defaults, settings?.inherited ?? []) : settings?.inherited ?? [];
  const baseline = !environmentId && headerMode ? combine(inherited, settings?.entries ?? []) : settings?.entries ?? [];
  const displayed = rows ?? baseline;
  /** Rows as stored: global headers omit unchanged defaults. */
  const stored = (edited: HeaderEntry[], entries: HeaderEntry[]) => edited.map((h) => ({ ...h, name: h.name.trim() }))
    .filter((h) => h.name !== "" || h.value !== "").filter((h) => {
      if (environmentId || !headerMode) return true;
      const base = inherited.find((b) => b.name.toLowerCase() === h.name.toLowerCase());
      return entries.some((s) => s.name.toLowerCase() === h.name.toLowerCase()) || !base || h.value !== base.value || h.enabled !== base.enabled;
    });
  usePreventUnload(saveStatus !== "" || confirm !== null || (rows !== null && settings !== null
    && JSON.stringify(stored(rows, settings.entries)) !== JSON.stringify(settings.entries)));
  const hiddenCount = (headerMode ? 2 : 0) + inherited.length;
  const last = displayed.at(-1);
  const tableRows = !last || last.name.trim() ? [...displayed, emptyHeader()] : displayed;
  const newRow = useRef<HTMLTableRowElement>(null);
  const previousCount = useRef(tableRows.length);
  const editing = rows !== null;
  useLayoutEffect(() => {
    if (editing && tableRows.length > previousCount.current) newRow.current?.scrollIntoView?.({ block: "nearest" });
    previousCount.current = tableRows.length;
  }, [tableRows.length, editing]);
  const change = (index: number, changes: Partial<HeaderEntry>) => {
    const next = displayed.map((h) => ({ ...h }));
    next[index] = { ...(next[index] ?? emptyHeader()), ...changes };
    version.current += 1;
    setRows(next); setSaveStatus(""); setError("");
    return next;
  };
  const save = (edited: HeaderEntry[]) => {
    if (!settings) return;
    const next = edited.map((h) => ({ ...h, name: h.name.trim() }));
    if (next.some((h, i) => h.name !== edited[i].name)) setRows(next);
    if (next.some((h) => !h.name && h.value)) {
      setError({ code: headerMode ? "validation.headers.name_required" : "validation.context.key_required" });
      return;
    }
    const headers = stored(next, settings.entries);
    const signature = JSON.stringify(headers);
    if (signature === lastQueued.current || (lastQueued.current === null && signature === JSON.stringify(settings.entries))) return;
    if (pendingCount > 0) { setConfirm({ headers, signature }); return; }
    commit(headers, signature);
  };
  const commit = (headers: HeaderEntry[], signature: string) => {
    lastQueued.current = signature;
    const revision = version.current;
    setSaveStatus("common.saving"); setError("");
    pending.current = preventUnloadUntil(pending.current.then(async () => {
      try {
        if (headerMode) await ipc.saveHeaders(headers, environmentId);
        else await ipc.saveContext(headers.map((entry) => ({ ...entry, value: parseContextValue(entry.value) })), environmentId);
        if (mounted.current) {
          setSettings((current) => current && { ...current, entries: headers });
          if (revision === version.current) setSaveStatus("");
        }
        await onSaved?.();
      } catch (reason) {
        if (lastQueued.current === signature) lastQueued.current = null;
        if (mounted.current && revision === version.current) { setError(normalizeAppError(reason)); setSaveStatus(""); }
      }
    }));
  };
  return <form className="panel" aria-label={headerMode ? (environmentId ? t("settingsTable.environmentHeaders") : t("settingsTable.globalHeaders")) : (environmentId ? t("settingsTable.environmentContext") : t("settingsTable.globalContext"))} autoComplete="off" onSubmit={(event) => { event.preventDefault(); save(displayed); }}>
    <header className="panel-header"><span className="row gap-2">{environmentId ? <PanelTitle title={title} collapsed={folded} toggle={() => setCollapsed(!collapsed)}>{!folded && <RevokeNotice environmentId={environmentId} />}</PanelTitle> : <><h3>{title}</h3><RevokeNotice environmentId={environmentId} /></>}
      {environmentId && !folded && hiddenCount > 0 && <button type="button" className="btn sm ghost" disabled={!settings} aria-expanded={showHidden} onClick={() => setShowHidden(!showHidden)}><Icon name="eye" />{t("settingsTable.hidden", { count: hiddenCount })}</button>}</span>
      {saveStatus && <span className="hint" role="status">{t(saveStatus as never)}</span>}
    </header>
    <ApprovalDialog open={confirm !== null} busy={false} tone="warn"
      title={headerMode ? t("settingsTable.saveHeaders") : t("settingsTable.saveContext")}
      description={t("settingsTable.pendingWarning", { count: pendingCount, scope: environmentId ? t("common.inThisEnvironment") : "" })}
      confirmLabel={t("settingsTable.saveAndRevoke")}
      onOpenChange={(open) => {
        if (open) return;
        if (!confirmed.current) { version.current += 1; setRows(null); setSaveStatus(""); setError(""); }
        confirmed.current = false;
        setConfirm(null);
      }}
      onConfirm={async () => { if (confirm) { confirmed.current = true; commit(confirm.headers, confirm.signature); } }} />
    {!folded && <div className="panel-body headers-body">
      {!environmentId && <p className="hint">{t(headerMode ? "settingsTable.globalHeadersHint" : "settingsTable.globalContextHint")}</p>}
      {settings && <fieldset className="props-fieldset">
        <table className="headers-table"><thead><tr><th><span className="sr-only">{t("settingsTable.enabled")}</span></th><th>{t("settingsTable.key")}</th><th>{t("settingsTable.value")}</th><th><span className="sr-only">{t("settingsTable.actions")}</span></th></tr></thead><tbody>
          {headerMode && (!environmentId || showHidden) && ["Host", "Content-Length"].map((name) => <tr key={name}>
            <td><input type="checkbox" checked disabled aria-label={t("settingsTable.automaticHeader", { name })} /></td>
            <td><input className="inline-input mono" value={name} readOnly aria-label={t("settingsTable.automaticHeaderName", { name })} /></td>
            <td><input className="inline-input mono" value={t("settingsTable.automaticValue")} readOnly aria-label={t("settingsTable.automaticHeaderValue", { name })} /></td><td />
          </tr>)}
          {showHidden && environmentId && inherited.map((h) => <tr key={`inherited-${h.name}`}>
            <td><input type="checkbox" checked={h.enabled && !displayed.some((local) => local.enabled && sameName(local.name, h.name))} disabled aria-label={t(headerMode ? "settingsTable.inheritedHeader" : "settingsTable.inheritedContext", { name: h.name })} /></td>
            <td><input className="inline-input mono" value={h.name} readOnly aria-label={t(headerMode ? "settingsTable.inheritedHeaderName" : "settingsTable.inheritedContextKey", { name: h.name })} /></td>
            <td><input className="inline-input mono" value={h.value} readOnly aria-label={t(headerMode ? "settingsTable.inheritedHeaderValue" : "settingsTable.inheritedContextValue", { name: h.name })} /></td><td />
          </tr>)}
          {tableRows.map((h, index) => {
            const blank = index === displayed.length;
            const names = headerMode ? headerNames.filter((name) => !["host", "content-length"].includes(name.toLowerCase())) : contextKeys;
            const suggestions = names.filter((name) => !displayed.some((row, i) => i !== index && sameName(row.name, name)));
            return <tr key={index} ref={blank ? newRow : undefined} className={blank ? "headers-new-row" : undefined}>
              <td>{!blank && <input type="checkbox" aria-label={t(headerMode ? "settingsTable.enableHeader" : "settingsTable.enableContext", { index: index + 1 })} checked={h.enabled} onChange={(e) => save(change(index, { enabled: e.target.checked }))} />}</td>
              <td><HeaderInput className="inline-input mono" aria-label={blank ? (headerMode ? t("settingsTable.newHeaderName") : t("settingsTable.newContextKey")) : t(headerMode ? "settingsTable.headerName" : "settingsTable.contextKey", { index: index + 1 })} value={h.name} required={!blank && h.value !== ""} placeholder={blank ? t("settingsTable.key") : undefined} spellCheck={false} suggestions={suggestions} caseSensitive={!headerMode} disabledSuggestions={headerMode ? reservedHeaderNames : []} onBlur={() => save(displayed)} onChange={(name) => change(index, { name })} /></td>
              <td><HeaderInput className={`inline-input mono ${headerMode && h.value === HEADER_MASK ? "secret-mask" : ""}`} aria-label={blank ? (headerMode ? t("settingsTable.newHeaderValue") : t("settingsTable.newContextValue")) : t(headerMode ? "settingsTable.headerValue" : "settingsTable.contextValue", { index: index + 1 })} value={h.value} placeholder={blank ? t("settingsTable.value") : undefined} data-tip={headerMode ? (h.value === HEADER_MASK ? t("settingsTable.savedSecret") : undefined) : t("settingsTable.contextValueHelp")} spellCheck={false} suggestions={headerMode ? headerValues.get(h.name.toLowerCase()) ?? [] : []} onBlur={() => save(displayed)} onChange={(value) => change(index, { value: headerMode && h.value === HEADER_MASK ? value.replaceAll("•", "") : value, enabled: true })} /></td>
              <td>{!blank && <button type="button" className="icon-btn sm" aria-label={t(headerMode ? "settingsTable.deleteHeader" : "settingsTable.deleteContext", { index: index + 1 })} onClick={() => { const next = displayed.filter((_, i) => i !== index); version.current += 1; setRows(next); save(next); }}><Icon name="trash" /></button>}</td>
            </tr>;
          })}
        </tbody></table>
      </fieldset>}
      {error && <p className="banner danger" role="alert">{formatAppError(error)}</p>}
    </div>}
  </form>;
}

/** Show a notice beside the title: saving revokes pending approvals. */
function RevokeNotice({ environmentId }: { environmentId?: string }) {
  const { t } = useTranslation();
  return <span className="title-warn tone-warn" tabIndex={0} aria-label={t("settingsTable.saveWarning")}>
    <Icon name="alert" />
    <span className="tooltip start" role="tooltip"><strong>{t("settingsTable.onSave")}</strong>{environmentId
      ? t("settingsTable.revokeEnvironmentRequests")
      : t("settingsTable.revokeAllRequests")}</span>
  </span>;
}
