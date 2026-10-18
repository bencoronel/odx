import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import { formatAppError, normalizeAppError, type AppError } from "./appError";

type PasswordMessage = { translationKey: "passwordPage.mismatch" | "passwordPage.updated" } | AppError;

export default function PasswordPage({ change, busy }: { change: (current: string, next: string) => Promise<void>; busy: boolean }) {
  const { t } = useTranslation();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [repeat, setRepeat] = useState("");
  const [message, setMessage] = useState<PasswordMessage | "">("");
  const [failed, setFailed] = useState(false);

  return <form className="panel-body form-grid" onSubmit={async (event) => {
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
    <label className="field span-2 half"><span>{t("passwordPage.current")}</span><input id="current-password" type="password" autoComplete="current-password" minLength={8} required disabled={busy} value={current} onChange={(e) => setCurrent(e.target.value)} /></label>
    <label className="field"><span>{t("passwordPage.new")}</span><input id="new-password" type="password" autoComplete="new-password" minLength={8} required disabled={busy} value={next} aria-describedby="new-password-help" onChange={(e) => setNext(e.target.value)} /></label>
    <label className="field"><span>{t("passwordPage.repeat")}</span><input id="repeat-password" type="password" autoComplete="new-password" minLength={8} required disabled={busy} value={repeat} onChange={(e) => setRepeat(e.target.value)} /></label>
    <p className="hint span-2" id="new-password-help">{t("passwordPage.hint")}</p>
    {message && <p className={`banner span-2 ${failed ? "danger" : "ok"}`} role={failed ? "alert" : "status"}><Icon name={failed ? "alert" : "check"} />{"translationKey" in message ? t(message.translationKey) : formatAppError(message)}</p>}
    <div className="row justify-end span-2"><button className="btn primary" disabled={busy || current.length < 8 || next.length < 8 || next !== repeat}>{busy ? t("common.saving") : t("passwordPage.change")}</button></div>
  </form>;
}
