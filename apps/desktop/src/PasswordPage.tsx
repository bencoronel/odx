import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { usePreventUnload } from "./windowState";

type PasswordMessage = { translationKey: "passwordPage.mismatch" | "passwordPage.updated" } | AppError;

export default function PasswordPage({ change, busy }: { change: (current: string, next: string) => Promise<void>; busy: boolean }) {
  const { t } = useTranslation();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [repeat, setRepeat] = useState("");
  const [message, setMessage] = useState<PasswordMessage | "">("");
  const [failed, setFailed] = useState(false);
  usePreventUnload(busy || current !== "" || next !== "" || repeat !== "");

  return <form className="panel" aria-labelledby="password-title" onSubmit={async (event) => {
    event.preventDefault();
    setMessage("");
    setFailed(false);
    if (next !== repeat) { setFailed(true); setMessage({ translationKey: "passwordPage.mismatch" }); return; }
    try {
      const pending = change(current, next);
      setCurrent(""); setNext(""); setRepeat("");
      await pending;
      setMessage({ translationKey: "passwordPage.updated" });
    } catch (error) { setFailed(true); setMessage(normalizeAppError(error)); }
  }}>
    <header className="panel-header"><h3 id="password-title">{t("unlock.password")}</h3></header>
    <label className="setting-row"><strong>{t("passwordPage.current")}</strong><input id="current-password" className="setting-control" type="password" autoComplete="current-password" minLength={8} required disabled={busy} value={current} onChange={(e) => setCurrent(e.target.value)} /></label>
    <label className="setting-row"><strong>{t("passwordPage.new")}</strong><input id="new-password" className="setting-control" type="password" autoComplete="new-password" minLength={8} required disabled={busy} value={next} aria-describedby="new-password-help" onChange={(e) => setNext(e.target.value)} /></label>
    <label className="setting-row"><strong>{t("passwordPage.repeat")}</strong><input id="repeat-password" className="setting-control" type="password" autoComplete="new-password" minLength={8} required disabled={busy} value={repeat} onChange={(e) => setRepeat(e.target.value)} /></label>
    {/* Only while choosing one: the rule and the lack of recovery matter then, not on every visit. */}
    {next && <div className="panel-body"><p id="new-password-help" className="banner warn compact"><Icon name="alert" />{t("passwordPage.hint")}</p></div>}
    {message && <div className="panel-body"><p className={`banner ${failed ? "danger" : "ok"}`} role={failed ? "alert" : "status"}><Icon name={failed ? "alert" : "check"} />{"translationKey" in message ? t(message.translationKey) : formatAppError(message)}</p></div>}
    <footer className="setting-row"><button className="btn primary" disabled={busy || current.length < 8 || next.length < 8 || next !== repeat}>{busy ? t("common.saving") : t("passwordPage.change")}</button></footer>
  </form>;
}
