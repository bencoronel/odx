import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ipc, type RuleDraft, type SavedViewDraft, type Stage } from "./ipc";
import { Icon } from "./Icon";
import { StageBadge } from "./StageBadge";
import { isHexColor, setTagColor, TAG_COLORS, type TagColor } from "./tagColors";
import type { FormDraft, Protocol } from "./EnvironmentForm";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { Banner, Button, PanelHeader, cx, panel, panelBody, settingRow } from "./ui";

const SNIPPETS_KEY = "odx.shell.snippets";

export type BundleEnvironment = {
  name: string;
  url: string;
  database: string;
  protocol: Protocol;
  project?: string | null;
  stage?: Stage | null;
  tags: string[];
};

export type BundleView = {
  name: string;
  description: string;
  model: string;
  domain: unknown[];
  fields: string[];
  order?: string | null;
  limit: number;
};

export type BundleRule = Omit<RuleDraft, "id" | "environment_id" | "expires_at">;
export type BundleSnippet = { name: string; command: string };

export type TeamBundle = {
  version: 1;
  exported_at: string;
  /** Strictly no users (`login`), no API keys (`secret`), and no proxy credentials. */
  environments: BundleEnvironment[];
  views: BundleView[];
  global_rules: BundleRule[];
  snippets: BundleSnippet[];
  tag_colors: Record<string, string>;
};

function readSnippets(): { id: string; name: string; command: string }[] {
  try {
    const raw = JSON.parse(localStorage.getItem(SNIPPETS_KEY) ?? "[]");
    return Array.isArray(raw) ? raw.filter((s) => typeof s?.name === "string" && typeof s?.command === "string") : [];
  } catch { return []; }
}

function parseBundle(text: string): TeamBundle | null {
  try {
    const raw = JSON.parse(text);
    if (!raw || typeof raw !== "object" || raw.version !== 1) return null;
    const environments: BundleEnvironment[] = Array.isArray(raw.environments)
      ? raw.environments
          .filter((e: unknown): e is Record<string, unknown> => !!e && typeof e === "object")
          .map((e: Record<string, unknown>) => ({
            name: typeof e.name === "string" ? e.name.trim() : "",
            url: typeof e.url === "string" ? e.url.trim() : "",
            database: typeof e.database === "string" ? e.database.trim() : "",
            protocol: (["auto", "legacy_jsonrpc", "xmlrpc", "json2"].includes(String(e.protocol)) ? String(e.protocol) : "auto") as Protocol,
            project: typeof e.project === "string" && e.project.trim() ? e.project.trim() : null,
            stage: (["dev", "test", "pre", "pro"].includes(String(e.stage)) ? String(e.stage) : null) as Stage | null,
            tags: Array.isArray(e.tags) ? e.tags.filter((t): t is string => typeof t === "string" && !!t.trim()) : [],
          }))
          .filter((e: BundleEnvironment) => e.name && e.url && e.database)
      : [];
    const views: BundleView[] = Array.isArray(raw.views)
      ? raw.views
          .filter((v: unknown): v is Record<string, unknown> => !!v && typeof v === "object")
          .map((v: Record<string, unknown>) => ({
            name: typeof v.name === "string" ? v.name.trim() : "",
            description: typeof v.description === "string" ? v.description.trim() : "",
            model: typeof v.model === "string" ? v.model.trim() : "",
            domain: Array.isArray(v.domain) ? v.domain : [],
            fields: Array.isArray(v.fields) ? v.fields.filter((f): f is string => typeof f === "string") : [],
            order: typeof v.order === "string" && v.order.trim() ? v.order.trim() : null,
            limit: typeof v.limit === "number" && Number.isInteger(v.limit) && v.limit >= 1 && v.limit <= 500 ? v.limit : 50,
          }))
          .filter((v: BundleView) => v.name && v.model)
      : [];
    const global_rules: BundleRule[] = Array.isArray(raw.global_rules)
      ? raw.global_rules.filter((r: unknown): r is BundleRule => !!r && typeof r === "object" && typeof (r as Record<string, unknown>).method === "string")
      : [];
    const snippets: BundleSnippet[] = Array.isArray(raw.snippets)
      ? raw.snippets
          .filter((s: unknown): s is Record<string, unknown> => !!s && typeof s === "object" && typeof (s as Record<string, unknown>).name === "string" && typeof (s as Record<string, unknown>).command === "string")
          .map((s: Record<string, unknown>) => ({ name: String(s.name).trim(), command: String(s.command).trim() }))
          .filter((s: BundleSnippet) => s.name && s.command)
      : [];
    const tag_colors: Record<string, string> = raw.tag_colors && typeof raw.tag_colors === "object"
      ? Object.fromEntries(Object.entries(raw.tag_colors as Record<string, unknown>).filter(([, v]) => typeof v === "string") as [string, string][])
      : {};
    return { version: 1, exported_at: typeof raw.exported_at === "string" ? raw.exported_at : "", environments, views, global_rules, snippets, tag_colors };
  } catch { return null; }
}

export default function TeamBundlePanel({ onConfigureEnvironment }: { onConfigureEnvironment?: (draft: FormDraft) => void }) {
  const { t } = useTranslation();
  const fileInput = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorState>("");
  const [note, setNote] = useState("");
  const [preview, setPreview] = useState<TeamBundle | null>(null);
  const [pickViews, setPickViews] = useState(true);
  const [pickSnippets, setPickSnippets] = useState(true);
  const [pickRules, setPickRules] = useState(true);
  const [pickColors, setPickColors] = useState(true);
  const [pendingEnvs, setPendingEnvs] = useState<BundleEnvironment[]>([]);

  async function exportBundle() {
    setBusy(true);
    setError("");
    setNote("");
    try {
      const [envs, views, rules, tagColors] = await Promise.all([
        ipc.environments(),
        ipc.views().catch(() => []),
        ipc.rules().catch(() => []),
        ipc.tagColors().catch(() => ({})),
      ]);
      const bundle: TeamBundle = {
        version: 1,
        exported_at: new Date().toISOString(),
        // Never export login, secret, headers, context or proxies.
        environments: envs.map((e) => ({
          name: e.name,
          url: e.url,
          database: e.database,
          protocol: (["auto", "legacy_jsonrpc", "xmlrpc", "json2"].includes(e.protocol) ? e.protocol : "auto") as Protocol,
          project: e.project ?? null,
          stage: e.stage ?? null,
          tags: e.tags,
        })),
        views: views.map((v) => ({
          name: v.name,
          description: v.description,
          model: v.model,
          domain: v.domain,
          fields: v.fields,
          order: v.order ?? null,
          limit: v.limit,
        })),
        global_rules: rules
          .filter((r) => r.scope?.kind === "global")
          .map((r) => ({
            action: r.action,
            scope: { kind: "global" },
            name: r.name,
            model_selector: r.model_selector ?? null,
            operations: r.operations,
            field_selectors: r.field_selectors,
            model: r.model,
            method: r.method,
            kind: r.kind,
            fields: r.fields,
            values: r.values,
            context: {},
            max_records: r.max_records,
          })),
        snippets: readSnippets().map(({ name, command }) => ({ name, command })),
        tag_colors: tagColors,
      };
      const stamp = new Date().toISOString().slice(0, 19).replace(/[-:]/g, "").replace("T", "-");
      const path = await ipc.shellExport(`odx-team-bundle-${stamp}.json`, JSON.stringify(bundle, null, 2));
      setNote(t("settingsPage.bundle.exported", { path }));
    } catch (reason) {
      setError(normalizeAppError(reason));
    } finally { setBusy(false); }
  }

  async function onFileSelected(file: File | undefined) {
    if (!file) return;
    setError("");
    setNote("");
    try {
      const text = await file.text();
      const parsed = parseBundle(text);
      if (!parsed) {
        setError(t("settingsPage.bundle.invalid"));
        return;
      }
      setPreview(parsed);
      setPickViews(parsed.views.length > 0);
      setPickSnippets(parsed.snippets.length > 0);
      setPickRules(parsed.global_rules.length > 0);
      setPickColors(Object.keys(parsed.tag_colors).length > 0);
    } catch (reason) {
      setError(normalizeAppError(reason));
    } finally {
      if (fileInput.current) fileInput.current.value = "";
    }
  }

  async function applyImport() {
    if (!preview) return;
    setBusy(true);
    setError("");
    setNote("");
    try {
      let viewsAdded = 0;
      let snippetsAdded = 0;
      let rulesAdded = 0;
      if (pickViews && preview.views.length) {
        const existing = await ipc.views().catch(() => []);
        const existingNames = new Set(existing.map((v) => v.name.toLowerCase()));
        for (const v of preview.views) {
          if (existingNames.has(v.name.toLowerCase())) continue;
          const draft: SavedViewDraft = { ...v, schedule: null };
          await ipc.saveView(draft);
          viewsAdded++;
        }
      }
      if (pickSnippets && preview.snippets.length) {
        const current = readSnippets();
        const existingCmds = new Set(current.map((s) => s.command));
        const next = [...current];
        for (const s of preview.snippets) {
          if (existingCmds.has(s.command)) continue;
          next.unshift({ id: `u-${Date.now()}-${snippetsAdded}`, name: s.name, command: s.command });
          snippetsAdded++;
        }
        try { localStorage.setItem(SNIPPETS_KEY, JSON.stringify(next)); } catch { /* optional */ }
      }
      if (pickRules && preview.global_rules.length) {
        for (const r of preview.global_rules) {
          await ipc.saveRule({ ...r, scope: { kind: "global" }, environment_id: "", expires_at: null });
          rulesAdded++;
        }
      }
      if (pickColors) {
        for (const [tag, color] of Object.entries(preview.tag_colors)) {
          if ((TAG_COLORS as readonly string[]).includes(color) || isHexColor(color)) {
            setTagColor(tag, color as TagColor);
          }
        }
      }
      if (preview.environments.length) {
        const existing = await ipc.environments().catch(() => []);
        const existingTargets = new Set(existing.map((e) => `${e.url.toLowerCase()}|${e.database}`));
        setPendingEnvs(preview.environments.filter((e) => !existingTargets.has(`${e.url.toLowerCase()}|${e.database}`)));
      }
      setPreview(null);
      setNote(t("settingsPage.bundle.imported", { views: viewsAdded, snippets: snippetsAdded, rules: rulesAdded }));
    } catch (reason) {
      setError(normalizeAppError(reason));
    } finally { setBusy(false); }
  }

  return <section className={panel}>
    <PanelHeader><h3>{t("settingsPage.bundle.title")}</h3></PanelHeader>
    <div className={settingRow}>
      <div>
        <strong>{t("settingsPage.bundle.subtitle")}</strong>
        <p className="text-fg-3 text-xs mt-px">{t("settingsPage.bundle.noSecretsNote")}</p>
      </div>
      <span className="inline-flex items-center gap-2">
        <Button size="sm" disabled={busy} onClick={() => void exportBundle()}><Icon name="external" />{t("settingsPage.bundle.export")}</Button>
        <Button size="sm" disabled={busy} onClick={() => fileInput.current?.click()}><Icon name="plus" />{t("settingsPage.bundle.import")}</Button>
        <input ref={fileInput} type="file" accept=".json,application/json" className="hidden" onChange={(e) => void onFileSelected(e.target.files?.[0])} />
      </span>
    </div>
    {error && <div className={cx(panelBody, "border-t border-line")}><Banner tone="danger" compact role="alert"><Icon name="alert" />{typeof error === "string" ? error : formatAppError(error)}</Banner></div>}
    {note && <div className={cx(panelBody, "border-t border-line")}><Banner tone="ok" compact role="status"><Icon name="check" />{note}</Banner></div>}
    {preview && <div className={cx(panelBody, "grid gap-2.5 border-t border-line bg-surface-2")}>
      <strong className="text-sm text-fg">{t("settingsPage.bundle.previewTitle")}</strong>
      <div className="flex flex-wrap gap-3 text-xs text-fg-2">
        {preview.views.length > 0 && <label className="inline-flex items-center gap-1.5 cursor-pointer"><input type="checkbox" checked={pickViews} onChange={(e) => setPickViews(e.target.checked)} />{t("settingsPage.bundle.viewsCount", { count: preview.views.length })}</label>}
        {preview.snippets.length > 0 && <label className="inline-flex items-center gap-1.5 cursor-pointer"><input type="checkbox" checked={pickSnippets} onChange={(e) => setPickSnippets(e.target.checked)} />{t("settingsPage.bundle.snippetsCount", { count: preview.snippets.length })}</label>}
        {preview.global_rules.length > 0 && <label className="inline-flex items-center gap-1.5 cursor-pointer"><input type="checkbox" checked={pickRules} onChange={(e) => setPickRules(e.target.checked)} />{t("settingsPage.bundle.rulesCount", { count: preview.global_rules.length })}</label>}
        {Object.keys(preview.tag_colors).length > 0 && <label className="inline-flex items-center gap-1.5 cursor-pointer"><input type="checkbox" checked={pickColors} onChange={(e) => setPickColors(e.target.checked)} />{t("settingsPage.bundle.colorsCount", { count: Object.keys(preview.tag_colors).length })}</label>}
        {preview.environments.length > 0 && <span className="text-fg-3">{t("settingsPage.bundle.envsCount", { count: preview.environments.length })}</span>}
      </div>
      <div className="flex justify-end gap-2">
        <Button size="sm" disabled={busy} onClick={() => setPreview(null)}>{t("common.cancel")}</Button>
        <Button size="sm" variant="primary" disabled={busy} onClick={() => void applyImport()}>{t("settingsPage.bundle.importAction")}</Button>
      </div>
    </div>}
    {pendingEnvs.length > 0 && <div className={cx(panelBody, "grid gap-2 border-t border-line")}>
      <strong className="text-xs text-fg-2">{t("settingsPage.bundle.pendingEnvsTitle")}</strong>
      <ul className="grid gap-1.5">
        {pendingEnvs.map((env, idx) => <li key={`${env.url}-${env.database}-${idx}`} className="flex items-center justify-between gap-2 px-2.5 py-1.5 rounded-md border border-line bg-surface-2">
          <span className="flex items-center gap-1.5 min-w-0 text-xs">
            <StageBadge stage={env.stage} compact />
            <strong className="text-fg truncate">{env.name}</strong>
            <code className="text-fg-3 truncate">{env.url} · {env.database}</code>
          </span>
          {onConfigureEnvironment && <Button size="sm" onClick={() => {
            setPendingEnvs((cur) => cur.filter((_, i) => i !== idx));
            onConfigureEnvironment({
              values: { name: env.name, url: env.url, database: env.database, login: "", secret: "", protocol: env.protocol },
              tags: env.tags,
              project: env.project ?? "",
              stage: env.stage ?? null,
            });
          }}>{t("settingsPage.bundle.configureEnv")}</Button>}
        </li>)}
      </ul>
    </div>}
  </section>;
}
