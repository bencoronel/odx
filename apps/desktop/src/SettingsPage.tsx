import { useEffect, useState, type ChangeEvent } from "react";
import SettingsTable from "./SettingsTable";
import ProxyPanel from "./ProxyPanel";
import PasswordPage from "./PasswordPage";
import { LanguageSelect } from "./LanguageSelector";
import { UpdatesPanel } from "./Updates";
import PanelTitle, { useCollapsed } from "./PanelTitle";
import { ConfirmDialog } from "./ConfirmDialog";
import { Icon, type IconName } from "./Icon";
import { ipc, type Settings, type SettingsView } from "./ipc";
import { useTranslation } from "react-i18next";
import { i18n } from "./i18n";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { usePreventUnload, type Theme } from "./windowState";
import { loadEnvironmentDefaults, saveEnvironmentDefaults, type EnvironmentDefaults } from "./EnvironmentForm";
import { Banner, Button, PanelHeader, Props, Segment, Segmented, navItem, panel, panelBody, settingRow } from "./ui";


function ThemeSelector({ theme, change }: { theme: Theme; change: (theme: Theme) => void }) {
  const { t } = useTranslation();
  return <Segmented role="group" aria-label={t("settingsPage.themeLabel")}>
    <Segment aria-pressed={theme === "light"} onClick={() => change("light")}><Icon name="sun" />{t("settingsPage.light")}</Segment>
    <Segment aria-pressed={theme === "dark"} onClick={() => change("dark")}><Icon name="moon" />{t("settingsPage.dark")}</Segment>
    <Segment aria-pressed={theme === "system"} onClick={() => change("system")}><Icon name="monitor" />{t("settingsPage.system")}</Segment>
  </Segmented>;
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
  const section = (id: SectionId) => ({ className: "grid gap-3.5", "aria-labelledby": `${id}-title`, hidden: active !== id });
  const title = (id: SectionId) => <header className="grid gap-[3px] mb-1">
    <h2 className="text-fg" id={`${id}-title`}>{t(sections.find(([key]) => key === id)![1])}</h2>
  </header>;
  return <div className="flex-1 min-h-0 grid grid-cols-[200px_minmax(0,1fr)] max-sm:grid-cols-1 max-sm:grid-rows-[auto_1fr]">
    <nav className="grid content-start gap-px px-2 py-3 border-r border-line bg-bg overflow-y-auto max-sm:grid-flow-col max-sm:auto-cols-max max-sm:overflow-x-auto max-sm:border-r-0 max-sm:border-b" aria-label={t("settingsPage.sections")}>
      {sections.map(([id, key, icon]) => <button key={id} type="button" className={navItem} aria-current={active === id ? "page" : undefined} onClick={() => setActive(id)}><Icon name={icon} /><span className="flex-1 max-md:hidden">{t(key)}</span></button>)}
    </nav>
    <div className="page-scroll flex-1 min-h-0 overflow-auto overscroll-contain px-7 pt-5 pb-8 max-sm:p-4"><div className="settings max-w-[760px]">
      <section {...section("general")}>
        {title("general")}
        <section className={panel}>
          <PanelHeader><h3>{t("settingsPage.appearance")}</h3></PanelHeader>
          <div className={settingRow}><strong>{t("settings.language.label")}</strong><LanguageSelect className="w-[260px] max-sm:w-full" /></div>
          <div className={settingRow}><strong>{t("settingsPage.theme")}</strong><ThemeSelector theme={theme} change={setTheme} /></div>
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
        <section className={panel}>
          <PanelHeader><h3>{t("unlock.vault")}</h3></PanelHeader>
          <div className={settingRow}><div><strong>{t("settingsPage.lockNow")}</strong>{pendingCount > 0 && <p className="text-fg-3 text-sm mt-px">{t("settingsPage.lockPending", { count: pendingCount })}</p>}</div><Button disabled={busy} onClick={lock}><Icon name="lock" />{t("navigation.lock")}</Button></div>
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
  return <section className={panel}>
    <PanelHeader><h3>{t("settingsPage.environmentDefaults")}</h3></PanelHeader>
    <label className={settingRow}><strong>{t("environmentForm.user")}</strong><input className="w-[260px] max-sm:w-full" autoComplete="off" value={defaults.login} onChange={set("login")} /></label>
    <label className={settingRow}><strong>{t("common.database")}</strong><input className="w-[260px] max-sm:w-full" autoComplete="off" value={defaults.database} onChange={set("database")} /></label>
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

function fieldError(view: SettingsView, draft: Draft, key: keyof Settings): string {
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
  if (loadError) return <section className={panel}><PanelHeader><h3>{t("settingsPage.limits")}</h3></PanelHeader>
    <Banner tone="danger" role="alert"><Icon name="alert" />{formatAppError(loadError)}</Banner></section>;
  if (!view || !draft) return <section className={panel} aria-busy="true"><PanelHeader><h3>{t("settingsPage.limits")}</h3></PanelHeader></section>;

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
    return <div key={field.key} className={settingRow}>
      <div className="gap-[3px]!"><label htmlFor={id}><strong>{text(field.label)}</strong></label>
        {/* The input enforces min and max; the error names the range when a value falls outside it. */}
        {error && <small id={`${id}-hint`} className="text-danger text-xs mt-px">{error}</small>}</div>
      <span className="inline-flex items-center gap-2">
        <input id={id} className="w-30" type="number" inputMode="numeric" min={min} max={max} step={1} value={draft[field.key]}
          aria-invalid={error ? true : undefined} aria-describedby={error ? `${id}-hint` : undefined}
          onChange={(e) => { setDraft({ ...draft, [field.key]: e.target.value }); setMessage(null); }} />
        <span className="min-w-[52px] text-fg-3 text-sm" aria-hidden="true">{text(field.unit)}</span>
      </span>
    </div>;
  };

  return <form className="grid gap-3.5" aria-label={t("settingsPage.limits")} onSubmit={(event) => {
    event.preventDefault();
    if (!changed || saving) return;
    if (shrinks) setConfirming(true); else void save().catch(() => undefined);
  }}>
    {cards.map((card) => <fieldset key={card.id} className={panel} disabled={saving} aria-labelledby={`limits-${card.id}`}>
      <PanelHeader><h3 id={`limits-${card.id}`}>{text(card.title)}</h3></PanelHeader>
      {card.groups.map((group, index) => group.title
        ? <fieldset key={index} className="border-t border-line" aria-labelledby={`limits-${card.id}-${index}`}>
          <div className="grid gap-0.5 px-3.5 pt-3 pb-0.5"><strong className="text-sm font-semibold text-fg-2" id={`limits-${card.id}-${index}`}>{text(group.title)}</strong></div>
          {group.fields.map(row)}
        </fieldset>
        : <div key={index}>{group.fields.map(row)}</div>)}
    </fieldset>)}
    {(dirty || message) && <div className="sticky bottom-0 z-2 flex items-center gap-2 px-3.5 py-2.5 border border-line-strong rounded-lg bg-surface shadow-pop [&_.icon]:size-3.5" role="group" aria-label={t("common.save")}>
      {message
        ? <p className={`grow flex items-center gap-1.5 text-sm ${"error" in message ? "text-danger" : "text-ok"}`} role={"error" in message ? "alert" : "status"}><Icon name={"error" in message ? "alert" : "check"} />{"error" in message ? formatAppError(message.error) : t(message.key)}</p>
        : <span className="grow text-fg-3 text-sm">{t("common.unsaved")}</span>}
      <Button size="sm" disabled={saving || !dirty} onClick={() => { setDraft(toDraft(view.settings)); setMessage(null); }}>{t("common.discard")}</Button>
      <Button type="submit" variant="primary" size="sm" disabled={saving || !valid || !changed}>{saving ? t("common.saving") : t("common.save")}</Button>
    </div>}
    <ConfirmDialog
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
  return <section className={panel} aria-label={t("settingsPage.fixedLimits")}>
    <PanelHeader><div className="flex-auto min-w-0"><PanelTitle title={t("settingsPage.fixedLimits")} collapsed={collapsed} toggle={() => setCollapsed(!collapsed)} /></div></PanelHeader>
    {!collapsed && <div className={panelBody}>
      <Props compact className="[&>div]:grid-cols-[170px_minmax(0,1fr)]!">{fixed.map((key) => <div key={key}><dt>{t(`settingsPage.fixed.${key}`)}</dt><dd>{t(`settingsPage.fixedValues.${key}`)}</dd></div>)}</Props>
    </div>}
  </section>;
}
