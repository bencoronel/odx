import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BundleType, getBundleType, getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { useTranslation } from "react-i18next";
import CopyButton from "./CopyButton";
import { Icon } from "./Icon";
import { formatAppError, normalizeAppError } from "./appError";
import { ipc } from "./ipc";
import { flushShellSessions } from "./shellFlush";

type Found = { update: Update; installable: boolean; bundle: BundleType | null };

/** `latest.json` lists only the AppImage; .deb/.rpm and dev builds get a link to the release page instead. */
async function find(): Promise<Found | null> {
  const update = await check();
  const bundle = update && await getBundleType().catch(() => null);
  return update && { update, installable: bundle === BundleType.AppImage, bundle };
}

/** Download, check against the release's .sha256 and install the package; null where no package fits. */
export function installCommand(bundle: BundleType | null, version: string) {
  const [file, install] = bundle === BundleType.Deb ? ["odx-linux-amd64.deb", "sudo apt install"]
    : bundle === BundleType.Rpm ? ["odx-linux-x86_64.rpm", "sudo dnf install"] : [];
  if (!file) return null;
  const base = `https://github.com/bencoronel/odx/releases/download/v${version}`;
  return `cd "$(mktemp -d)" && curl -fL -O ${base}/${file} -O ${base}/${file}.sha256 && sha256sum -c ${file}.sha256 && ${install} ./${file}`;
}

const DAY = 24 * 60 * 60 * 1000;
const RETRY = 60 * 60 * 1000;
let latest: { at: number; result: Promise<Found | null> } | null = null;
/** A quiet check (offline is normal) at most once a day, an hour after a failure; lock/unlock remounts reuse it. */
function dailyCheck() {
  if (!latest || Date.now() - latest.at >= DAY) {
    const current: { at: number; result: Promise<Found | null> } = { at: Date.now(), result: Promise.resolve(null) };
    current.result = find().catch(() => { current.at = Date.now() - DAY + RETRY; return null; });
    latest = current;
  }
  return latest.result;
}

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

/** Update notice, shown on the unlock screen and in the app alike. Odx can stay in the tray for days, so it
 * rechecks hourly and when the window gains focus; the daily limit keeps that to one request a day. */
export function UpdateNotice() {
  const { t } = useTranslation();
  const [found, setFound] = useState<Found | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    const run = () => void dailyCheck().then((f) => { if (live && f) setFound(f); });
    run();
    const timer = window.setInterval(run, RETRY);
    window.addEventListener("focus", run);
    return () => { live = false; window.clearInterval(timer); window.removeEventListener("focus", run); };
  }, []);
  // Dismissing hides this version only; a newer one shows again.
  if (!found || dismissed === found.update.version) return null;
  return <div className="desktop-notices"><div className="banner info page-banner" role="status">
    <Icon name="refresh" /><span className="grow">{t("updates.available", { version: found.update.version })}</span>
    <UpdateAction found={found} />
    <button type="button" className="icon-btn sm" aria-label={t("common.close")} onClick={() => setDismissed(found.update.version)}><Icon name="x" /></button>
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
  const command = found && !found.installable ? installCommand(found.bundle, found.update.version) : null;
  return <section className="panel" aria-labelledby="settings-updates">
    <header className="panel-header"><div><h3 id="settings-updates">{t("updates.title")}</h3></div></header>
    <div className="setting-row">
      <div><strong>{version && t("updates.version", { version })}</strong>
        {result === "none" && <p className="hint" role="status">{t("updates.upToDate")}</p>}
        {result === "failed" && <p className="hint text-danger" role="alert">{t("updates.checkFailed")}</p>}
        {found && <p className="hint" role="status">{t("updates.available", { version: found.update.version })}{!found.installable && ` ${t("updates.manual")}`}</p>}
        {command && <><p className="hint">{t("updates.command")}</p>
          <div className="row gap-2"><pre className="update-command grow">{command}</pre><CopyButton text={command} /></div></>}
      </div>
      <span className="row gap-2">
        {found && <UpdateAction found={found} />}
        <button type="button" className="btn" disabled={result === "checking"} onClick={run}>{result === "checking" ? t("updates.checking") : t("updates.check")}</button>
      </span>
    </div>
  </section>;
}
