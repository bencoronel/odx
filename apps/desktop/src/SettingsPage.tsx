import { useEffect, useState, type ChangeEvent } from "react";
import SettingsTable from "./SettingsTable";
import ProxyPanel from "./ProxyPanel";
import PasswordPage from "./PasswordPage";
import { LanguageSelect } from "./LanguageSelector";
import { UpdatesPanel } from "./Updates";
import PanelTitle, { useCollapsed } from "./PanelTitle";
import { ApprovalDialog } from "@/components/ui/approval-dialog";
import { Icon, type IconName } from "./Icon";
import { ipc, type Settings, type SettingsView } from "./ipc";
import { useTranslation } from "react-i18next";
import { i18n } from "./i18n";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { usePreventUnload } from "./windowState";
import { loadEnvironmentDefaults, saveEnvironmentDefaults, type EnvironmentDefaults } from "./EnvironmentForm";

export type Theme = "light" | "dark" | "system";

function ThemeSelector({ theme, change }: { theme: Theme; change: (theme: Theme) => void }) {
  const { t } = useTranslation();
  return <div className="segmented" role="group" aria-label={t("settingsPage.themeLabel")}>
    <button type="button" aria-pressed={theme === "light"} onClick={() => change("light")}><Icon name="sun" />{t("settingsPage.light")}</button>
    <button type="button" aria-pressed={theme === "dark"} onClick={() => change("dark")}><Icon name="moon" />{t("settingsPage.dark")}</button>
    <button type="button" aria-pressed={theme === "system"} onClick={() => change("system")}><Icon name="monitor" />{t("settingsPage.system")}</button>
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
  const [active, setActive] = useState<SectionId>("general");
  // Hidden, not unmounted, so unsaved drafts survive switching sections.
  const section = (id: SectionId) => ({ className: "settings-group", "aria-labelledby": `${id}-title`, hidden: active !== id });
  const title = (id: SectionId) => <header className="settings-head">
    <h2 id={`${id}-title`}>{t(sections.find(([key]) => key === id)![1])}</h2>
  </header>;
  return <div className="settings-layout">
    <nav className="settings-nav" aria-label={t("settingsPage.sections")}>
      {sections.map(([id, key, icon]) => <button key={id} type="button" className="nav-item" aria-current={active === id ? "page" : undefined} onClick={() => setActive(id)}><Icon name={icon} /><span>{t(key)}</span></button>)}
    </nav>
    <div className="page-scroll"><div className="settings">
      <section {...section("general")}>
        {title("general")}
        <section className="panel">
          <header className="panel-header"><h3>{t("settingsPage.appearance")}</h3></header>
          <div className="setting-row"><strong>{t("settings.language.label")}</strong><LanguageSelect className="setting-control" /></div>
          <div className="setting-row"><strong>{t("settingsPage.theme")}</strong><ThemeSelector theme={theme} change={setTheme} /></div>
        </section>
        <UpdatesPanel />
      </section>
      <section {...section("connection")}>
        {title("connection")}
        <EnvironmentDefaultsPanel />
        <ProxyPanel pendingCount={pendingCount} onSaved={onHeadersSaved} />
        <SettingsTable kind="headers" pendingCount={pendingCount} onSaved={onHeadersSaved} />
        <SettingsTable kind="context" pendingCount={pendingCount} onSaved={onContextSaved} />
      </section>
      <section {...section("execution")}>
        {title("execution")}
        <LimitsPanel />
        <FixedLimits />
      </section>
      <section {...section("security")}>
        {title("security")}
        <section className="panel">
          <header className="panel-header"><h3>{t("unlock.vault")}</h3></header>
          <div className="setting-row"><div><strong>{t("settingsPage.lockNow")}</strong>{pendingCount > 0 && <p className="hint">{t("settingsPage.lockPending", { count: pendingCount })}</p>}</div><button className="btn" disabled={busy} onClick={lock}><Icon name="lock" />{t("navigation.lock")}</button></div>
        </section>
        <PasswordPage busy={busy} change={changePassword} />
      </section>
    </div></div>
  </div>;
}

function EnvironmentDefaultsPanel() {
  const { t } = useTranslation();
  const [defaults, setDefaults] = useState(loadEnvironmentDefaults);
  const set = (key: keyof EnvironmentDefaults) => (event: ChangeEvent<HTMLInputElement>) => {
    const next = { ...defaults, [key]: event.target.value };
    setDefaults(next);
    saveEnvironmentDefaults(next);
  };
  return <section className="panel">
    <header className="panel-header"><h3>{t("settingsPage.environmentDefaults")}</h3></header>
    <label className="setting-row"><strong>{t("environmentForm.user")}</strong><input className="setting-control" autoComplete="off" value={defaults.login} onChange={set("login")} /></label>
    <label className="setting-row"><strong>{t("common.database")}</strong><input className="setting-control" autoComplete="off" value={defaults.database} onChange={set("database")} /></label>
  </section>;
}

const sections = [
  ["general", "settingsPage.general", "settings"], ["connection", "settingsPage.odooConnection", "plug"],
  ["execution", "settingsPage.limitsTab", "activity"], ["security", "settingsPage.security", "shield"],
] as const satisfies readonly (readonly [string, string, IconName])[];
type SectionId = typeof sections[number][0];

type Field = { key: keyof Settings; label: string; unit: string };
type Group = { title?: string; fields: Field[] };
const age = (key: keyof Settings, unit: string): Field => ({ key, label: "settingsPage.fields.maxAge", unit });
const size = (key: keyof Settings): Field => ({ key, label: "settingsPage.fields.maxSize", unit: "settingsPage.units.mib" });
const cards: { id: string; title: string; groups: Group[] }[] = [
  { id: "execution", title: "settingsPage.execution.title", groups: [{ fields: [
    { key: "global_concurrency", label: "settingsPage.fields.globalConcurrency", unit: "settingsPage.units.calls" },
    { key: "environment_concurrency", label: "settingsPage.fields.environmentConcurrency", unit: "settingsPage.units.calls" },
    { key: "upstream_timeout_secs", label: "settingsPage.fields.timeout", unit: "settingsPage.units.seconds" },
    { key: "approval_minutes", label: "settingsPage.fields.approval", unit: "settingsPage.units.minutes" },
  ] }] },
  { id: "retention", title: "settingsPage.retention.title", groups: [
    { title: "settingsPage.retention.activity", fields: [age("activity_days", "settingsPage.units.days"), size("activity_mib")] },
    { title: "settingsPage.retention.output", fields: [age("output_hours", "settingsPage.units.hours"), size("output_mib")] },
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
  const dirty = (Object.keys(draft) as (keyof Settings)[]).some((k) => draft[k] !== String(view.settings[k]));

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

  // Keys come from the static `cards` table above.
  const text = (key: string) => String(t(key as never));
  const row = (field: Field) => {
    const { min, max } = view.bounds[field.key];
    const id = `setting-${field.key}`;
    const error = errors[field.key];
    return <div key={field.key} className="setting-row">
      <div><label htmlFor={id}><strong>{text(field.label)}</strong></label>
        {/* The input enforces min and max; the error names the range when a value falls outside it. */}
        {error && <small id={`${id}-hint`} className="hint text-danger">{error}</small>}</div>
      <span className="input-unit">
        <input id={id} className="setting-number" type="number" inputMode="numeric" min={min} max={max} step={1} value={draft[field.key]}
          aria-invalid={error ? true : undefined} aria-describedby={error ? `${id}-hint` : undefined}
          onChange={(e) => { setDraft({ ...draft, [field.key]: e.target.value }); setMessage(null); }} />
        <span aria-hidden="true">{text(field.unit)}</span>
      </span>
    </div>;
  };

  return <form className="limits-form" aria-label={t("settingsPage.limits")} onSubmit={(event) => {
    event.preventDefault();
    if (!changed || saving) return;
    if (shrinks) setConfirming(true); else void save().catch(() => undefined);
  }}>
    {cards.map((card) => <fieldset key={card.id} className="panel" disabled={saving} aria-labelledby={`limits-${card.id}`}>
      <header className="panel-header"><h3 id={`limits-${card.id}`}>{text(card.title)}</h3></header>
      {card.groups.map((group, index) => group.title
        ? <fieldset key={index} className="limits-group" aria-labelledby={`limits-${card.id}-${index}`}>
          <div className="limits-group-head"><strong id={`limits-${card.id}-${index}`}>{text(group.title)}</strong></div>
          {group.fields.map(row)}
        </fieldset>
        : <div key={index}>{group.fields.map(row)}</div>)}
    </fieldset>)}
    {(dirty || message) && <div className="limits-savebar" role="group" aria-label={t("common.save")}>
      {message
        ? <p className={`grow ${"error" in message ? "text-danger" : "text-ok"}`} role={"error" in message ? "alert" : "status"}><Icon name={"error" in message ? "alert" : "check"} />{"error" in message ? formatAppError(message.error) : t(message.key)}</p>
        : <span className="grow hint">{t("common.unsaved")}</span>}
      <button type="button" className="btn sm" disabled={saving || !dirty} onClick={() => { setDraft(toDraft(view.settings)); setMessage(null); }}>{t("common.discard")}</button>
      <button type="submit" className="btn sm primary" disabled={saving || !valid || !changed}>{saving ? t("common.saving") : t("common.save")}</button>
    </div>}
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
    <header className="panel-header"><div><PanelTitle title={t("settingsPage.fixedLimits")} collapsed={collapsed} toggle={() => setCollapsed(!collapsed)} /></div></header>
    {!collapsed && <div className="panel-body">
      <dl className="props compact">{fixed.map((key) => <div key={key}><dt>{t(`settingsPage.fixed.${key}`)}</dt><dd>{t(`settingsPage.fixedValues.${key}`)}</dd></div>)}</dl>
    </div>}
  </section>;
}
