import { useEffect, useState } from "react";
import SettingsTable from "./SettingsTable";
import ProxyPanel from "./ProxyPanel";
import PasswordPage from "./PasswordPage";
import LanguageSelector from "./LanguageSelector";
import PanelTitle, { useCollapsed } from "./PanelTitle";
import { ApprovalDialog } from "@/components/ui/approval-dialog";
import { Icon } from "./Icon";
import { ipc, type Settings, type SettingsView } from "./ipc";
import { useTranslation } from "react-i18next";
import { i18n } from "./i18n";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { usePreventUnload } from "./windowState";

export type Theme = "light" | "dark";

function ThemeSelector({ theme, change }: { theme: Theme; change: (theme: Theme) => void }) {
  const { t } = useTranslation();
  return <div className="segmented" role="group" aria-label={t("settingsPage.themeLabel")}>
    <button type="button" aria-pressed={theme === "light"} onClick={() => change("light")}><Icon name="sun" />{t("settingsPage.light")}</button>
    <button type="button" aria-pressed={theme === "dark"} onClick={() => change("dark")}><Icon name="moon" />{t("settingsPage.dark")}</button>
  </div>;
}

export default function SettingsPage({ theme, setTheme, busy, pendingCount, onContextSaved, onHeadersSaved, lock, changePassword }: {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  busy: boolean;
  pendingCount: number;
  onContextSaved: () => Promise<void>;
  onHeadersSaved: () => Promise<void>;
  lock: () => void;
  changePassword: (current: string, next: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  return <div className="page-scroll"><div className="settings">
    <section className="settings-group" aria-labelledby="settings-general">
      <h2 id="settings-general" className="settings-group-title">{t("settingsPage.general")}</h2>
      <LanguageSelector />
      <section className="panel">
        <header className="panel-header"><h3>{t("settingsPage.appearance")}</h3></header>
        <div className="setting-row"><div><strong>{t("settingsPage.theme")}</strong><p className="hint">{t("settingsPage.remembered")}</p></div><ThemeSelector theme={theme} change={setTheme} /></div>
      </section>
    </section>
    <section className="settings-group" aria-labelledby="settings-connection">
      <h2 id="settings-connection" className="settings-group-title">{t("settingsPage.odooConnection")}</h2>
      <ProxyPanel pendingCount={pendingCount} onSaved={onHeadersSaved} />
      <SettingsTable kind="headers" pendingCount={pendingCount} onSaved={onHeadersSaved} />
      <SettingsTable kind="context" pendingCount={pendingCount} onSaved={onContextSaved} />
    </section>
    <section className="settings-group" aria-labelledby="settings-execution">
      <h2 id="settings-execution" className="settings-group-title">{t("settingsPage.executionStorage")}</h2>
      <LimitsPanel />
    </section>
    <section className="settings-group" aria-labelledby="settings-security">
      <h2 id="settings-security" className="settings-group-title">{t("settingsPage.security")}</h2>
      <section className="panel">
        <header className="panel-header"><h3>{t("unlock.vault")}</h3></header>
        <div className="setting-row"><div><strong>{t("settingsPage.lockNow")}</strong><p className="hint">{t("settingsPage.lockHint")}</p></div><button className="btn" disabled={busy} onClick={lock}><Icon name="lock" />{t("navigation.lock")}</button></div>
        <header className="panel-header panel-foot"><h3>{t("unlock.password")}</h3></header>
        <PasswordPage busy={busy} change={changePassword} />
      </section>
    </section>
    <FixedLimits />
  </div></div>;
}

type SettingsPageText = "settingsPage.groups.concurrency.title" | "settingsPage.groups.concurrency.hint" | "settingsPage.groups.total" | "settingsPage.groups.perEnvironment" | "settingsPage.groups.calls" | "settingsPage.groups.timeout.title" | "settingsPage.groups.timeout.hint" | "settingsPage.groups.perRequest" | "settingsPage.groups.seconds" | "settingsPage.groups.approval.title" | "settingsPage.groups.approval.hint" | "settingsPage.groups.validity" | "settingsPage.groups.minutes" | "settingsPage.groups.activity.title" | "settingsPage.groups.activity.hint" | "settingsPage.groups.age" | "settingsPage.groups.days" | "settingsPage.groups.size" | "settingsPage.groups.mib" | "settingsPage.groups.output.title" | "settingsPage.groups.output.hint" | "settingsPage.groups.totalSize" | "settingsPage.groups.hours";
type Field = { key: keyof Settings; label: SettingsPageText; unit: SettingsPageText };
const groups: { title: string; hint: string; fields: Field[] }[] = [
  { title: "settingsPage.groups.concurrency.title", hint: "settingsPage.groups.concurrency.hint", fields: [
    { key: "global_concurrency", label: "settingsPage.groups.total", unit: "settingsPage.groups.calls" },
    { key: "environment_concurrency", label: "settingsPage.groups.perEnvironment", unit: "settingsPage.groups.calls" },
  ] },
  { title: "settingsPage.groups.timeout.title", hint: "settingsPage.groups.timeout.hint", fields: [
    { key: "upstream_timeout_secs", label: "settingsPage.groups.perRequest", unit: "settingsPage.groups.seconds" },
  ] },
  { title: "settingsPage.groups.approval.title", hint: "settingsPage.groups.approval.hint", fields: [
    { key: "approval_minutes", label: "settingsPage.groups.validity", unit: "settingsPage.groups.minutes" },
  ] },
  { title: "settingsPage.groups.activity.title", hint: "settingsPage.groups.activity.hint", fields: [
    { key: "activity_days", label: "settingsPage.groups.age", unit: "settingsPage.groups.days" },
    { key: "activity_mib", label: "settingsPage.groups.size", unit: "settingsPage.groups.mib" },
  ] },
  { title: "settingsPage.groups.output.title", hint: "settingsPage.groups.output.hint", fields: [
    { key: "output_hours", label: "settingsPage.groups.age", unit: "settingsPage.groups.hours" },
    { key: "output_mib", label: "settingsPage.groups.totalSize", unit: "settingsPage.groups.mib" },
  ] },
];
const retentionKeys: (keyof Settings)[] = ["activity_days", "activity_mib", "output_hours", "output_mib"];
type SettingsMessage = { key: "settingsPage.saved" } | { error: AppError };

/** Keep form input as text; validate bounded integers on save. */
type Draft = Record<keyof Settings, string>;
const toDraft = (s: Settings) => Object.fromEntries(Object.entries(s).map(([k, v]) => [k, String(v)])) as Draft;

export function fieldError(view: SettingsView, draft: Draft, key: keyof Settings): string {
  const value = Number(draft[key]);
  const { min, max } = view.bounds[key];
  if (draft[key].trim() === "" || !Number.isSafeInteger(value) || value < min || value > max) return i18n.t("settingsPage.rangeError", { min, max });
  if (key === "environment_concurrency" && value > Number(draft.global_concurrency)) return i18n.t("settingsPage.concurrencyError");
  return "";
}

function LimitsPanel() {
  const { t } = useTranslation();
  const [view, setView] = useState<SettingsView | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [loadError, setLoadError] = useState<AppError | null>(null);
  const [message, setMessage] = useState<SettingsMessage | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  usePreventUnload(saving || confirming || (draft !== null && view !== null
    && JSON.stringify(draft) !== JSON.stringify(toDraft(view.settings))));
  useEffect(() => {
    ipc.settings().then((v) => { setView(v); setDraft(toDraft(v.settings)); }).catch((e) => setLoadError(normalizeAppError(e)));
  }, []);
  if (loadError) return <section className="panel"><header className="panel-header"><h3>{t("settingsPage.limits")}</h3></header>
    <p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(loadError)}</p></section>;
  if (!view || !draft) return <section className="panel" aria-busy="true"><header className="panel-header"><h3>{t("settingsPage.limits")}</h3></header></section>;

  const errors = Object.fromEntries(Object.keys(draft).map((k) => [k, fieldError(view, draft, k as keyof Settings)])) as Record<keyof Settings, string>;
  const valid = Object.values(errors).every((e) => !e);
  const next = valid ? Object.fromEntries(Object.entries(draft).map(([k, v]) => [k, Number(v)])) as Settings : null;
  const changed = next !== null && (Object.keys(next) as (keyof Settings)[]).some((k) => next[k] !== view.settings[k]);
  const shrinks = next !== null && retentionKeys.some((k) => next[k] < view.settings[k]);

  async function save() {
    if (!next) return;
    setSaving(true);
    setMessage(null);
    try {
      const saved = await ipc.saveSettings(next);
      setView(saved);
      setDraft(toDraft(saved.settings));
      setMessage({ key: "settingsPage.saved" });
    } catch (e) {
      setMessage({ error: normalizeAppError(e) });
      throw e;
    } finally { setSaving(false); }
  }

  return <form className="panel" aria-label={t("settingsPage.limits")} onSubmit={(event) => {
    event.preventDefault();
    if (!changed || saving) return;
    if (shrinks) setConfirming(true); else void save().catch(() => undefined);
  }}>
    <header className="panel-header"><h3>{t("settingsPage.limits")}</h3>
      <span className="row gap-2">
        <button type="button" className="btn sm" disabled={saving || !(Object.keys(draft) as (keyof Settings)[]).some((k) => draft[k] !== String(view.settings[k]))} onClick={() => { setDraft(toDraft(view.settings)); setMessage(null); }}>{t("common.discard")}</button>
        <button type="submit" className="btn sm primary" disabled={saving || !valid || !changed}>{saving ? t("common.saving") : t("common.save")}</button>
      </span>
    </header>
    <div className="panel-body">
      <p className="hint">{t("settingsPage.limitsHint")}</p>
      {groups.map((group) => <fieldset key={group.title} className="limits-group" disabled={saving}>
        <legend><strong>{t(group.title as SettingsPageText)}</strong></legend>
        <p className="hint">{t(group.hint as SettingsPageText)}</p>
        <div className="row gap-2">{group.fields.map((field) => {
          const { min, max } = view.bounds[field.key];
          const id = `setting-${field.key}`;
          return <label key={field.key} className="field" htmlFor={id}><span>{t(field.label)} ({t(field.unit)})</span>
            <input id={id} className="w-28" type="number" inputMode="numeric" min={min} max={max} step={1} value={draft[field.key]}
              aria-invalid={errors[field.key] ? true : undefined} aria-describedby={`${id}-hint`}
              onChange={(e) => { setDraft({ ...draft, [field.key]: e.target.value }); setMessage(null); }} />
            <small id={`${id}-hint`} className={`hint ${errors[field.key] ? "text-danger" : ""}`}>{errors[field.key] || t("settingsPage.defaultRange", { value: view.defaults[field.key], min, max })}</small>
          </label>;
        })}</div>
      </fieldset>)}
      {message && <p className={`banner ${"error" in message ? "danger" : "ok"}`} role={"error" in message ? "alert" : "status"}><Icon name={"error" in message ? "alert" : "check"} />{"error" in message ? formatAppError(message.error) : t(message.key)}</p>}
    </div>
    <ApprovalDialog
      open={confirming}
      busy={saving}
      destructive
      title={t("settingsPage.reduceRetention")}
      description={t("settingsPage.reduceRetentionWarning")}
      confirmLabel={t("settingsPage.reduceAndDelete")}
      onOpenChange={(open) => { if (!open && !saving) setConfirming(false); }}
      onConfirm={save}
    />
  </form>;
}

/** Fixed limits, for display only. */
const fixed = [
  "executionQueue", "pendingApprovals", "odooConnection", "queries", "apiInput", "odooResponse",
  "agentResponse", "binaryDownload", "exportedFile", "wait", "activity", "auditQueue",
] as const;

function FixedLimits() {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useCollapsed("settings-fixed-limits");
  return <section className="panel" aria-label={t("settingsPage.fixedLimits")}>
    <header className="panel-header"><PanelTitle title={t("settingsPage.fixedLimits")} collapsed={collapsed} toggle={() => setCollapsed(!collapsed)} /></header>
    {!collapsed && <div className="panel-body"><p className="hint">{t("settingsPage.fixedLimitsHint")}</p>
      <dl className="props compact">{fixed.map((key) => <div key={key}><dt>{t(`settingsPage.fixed.${key}`)}</dt><dd>{t(`settingsPage.fixedValues.${key}`)}</dd></div>)}</dl>
    </div>}
  </section>;
}
