import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BundleType, getBundleType, getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import { formatAppError, normalizeAppError } from "./appError";
import { ipc } from "./ipc";
import { flushShellSessions } from "./shellFlush";

type Found = { update: Update; installable: boolean };

/** `latest.json` lists only the AppImage; .deb/.rpm and dev builds get a link to the release page instead. */
async function find(): Promise<Found | null> {
  const update = await check();
  return update && { update, installable: await getBundleType().catch(() => null) === BundleType.AppImage };
}

let startup: Promise<Found | null> | null = null;
/** One quiet check per app start (offline is normal); lock/unlock remounts reuse it. */
const startupCheck = () => (startup ??= find().catch(() => null));

function UpdateAction({ found }: { found: Found }) {
  const { t } = useTranslation();
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState("");
  async function install() {
    setInstalling(true);
    setError("");
    try {
      await found.update.downloadAndInstall();
      await flushShellSessions();
      await ipc.restart();
    } catch {
      setError(t("updates.installFailed"));
      setInstalling(false);
    }
  }
  const open = () => { setError(""); invoke("updates_open_releases").catch((e) => setError(formatAppError(normalizeAppError(e)))); };
  return <>
    {error && <span className="text-danger" role="alert">{error}</span>}
    {found.installable
      ? <button type="button" className="btn sm primary" disabled={installing} onClick={() => void install()}><Icon name="refresh" />{installing ? t("updates.installing") : t("updates.install")}</button>
      : <button type="button" className="btn sm" onClick={open}><Icon name="external" />{t("updates.openReleases")}</button>}
  </>;
}

/** Startup notice, shown on the unlock screen and in the app alike. */
export function UpdateNotice() {
  const { t } = useTranslation();
  const [found, setFound] = useState<Found | null>(null);
  const [dismissed, setDismissed] = useState(false);
  useEffect(() => {
    let live = true;
    void startupCheck().then((f) => { if (live) setFound(f); });
    return () => { live = false; };
  }, []);
  if (!found || dismissed) return null;
  return <div className="desktop-notices"><div className="banner info page-banner" role="status">
    <Icon name="refresh" /><span className="grow">{t("updates.available", { version: found.update.version })}</span>
    <UpdateAction found={found} />
    <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={() => setDismissed(true)}><Icon name="x" /></button>
  </div></div>;
}

/** Settings panel: current version and a manual check. */
export function UpdatesPanel() {
  const { t } = useTranslation();
  const [version, setVersion] = useState("");
  const [result, setResult] = useState<Found | "checking" | "none" | "failed" | null>(null);
  useEffect(() => { getVersion().then(setVersion, () => undefined); }, []);
  const run = () => { setResult("checking"); find().then((f) => setResult(f ?? "none"), () => setResult("failed")); };
  const found = typeof result === "object" ? result : null;
  return <section className="panel" aria-labelledby="settings-updates">
    <header className="panel-header"><h3 id="settings-updates">{t("updates.title")}</h3></header>
    <div className="setting-row">
      <div><strong>{version && t("updates.version", { version })}</strong>
        {result === "none" && <p className="hint" role="status">{t("updates.upToDate")}</p>}
        {result === "failed" && <p className="hint text-danger" role="alert">{t("updates.checkFailed")}</p>}
        {found && <p className="hint" role="status">{t("updates.available", { version: found.update.version })}{!found.installable && ` ${t("updates.manual")}`}</p>}
      </div>
      <span className="row gap-2">
        {found && <UpdateAction found={found} />}
        <button type="button" className="btn" disabled={result === "checking"} onClick={run}>{result === "checking" ? t("updates.checking") : t("updates.check")}</button>
      </span>
    </div>
  </section>;
}
