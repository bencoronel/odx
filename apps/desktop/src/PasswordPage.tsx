import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Icon } from "./Icon";
import { formatAppError, normalizeAppError, type AppError } from "./appError";
import { usePreventUnload } from "./windowState";
import { Banner, Button, PanelHeader, cx, panel, panelBody, settingRow } from "./ui";

type PasswordMessage = { translationKey: "passwordPage.mismatch" | "passwordPage.updated" } | AppError;

export default function PasswordPage({ change, busy }: { change: (current: string, next: string) => Promise<void>; busy: boolean }) {
  const { t } = useTranslation();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [repeat, setRepeat] = useState("");
  const [message, setMessage] = useState<PasswordMessage | "">("");
  const [failed, setFailed] = useState(false);
  usePreventUnload(busy || current !== "" || next !== "" || repeat !== "");

  return <form className={panel} aria-labelledby="password-title" onSubmit={async (event) => {
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
    <PanelHeader><h3 id="password-title">{t("unlock.password")}</h3></PanelHeader>
    <label className={settingRow}><strong>{t("passwordPage.current")}</strong><input id="current-password" className="w-[260px] max-sm:w-full" type="password" autoComplete="current-password" minLength={8} required disabled={busy} value={current} onChange={(e) => setCurrent(e.target.value)} /></label>
    <label className={settingRow}><strong>{t("passwordPage.new")}</strong><input id="new-password" className="w-[260px] max-sm:w-full" type="password" autoComplete="new-password" minLength={8} required disabled={busy} value={next} aria-describedby="new-password-help" onChange={(e) => setNext(e.target.value)} /></label>
    <label className={settingRow}><strong>{t("passwordPage.repeat")}</strong><input id="repeat-password" className="w-[260px] max-sm:w-full" type="password" autoComplete="new-password" minLength={8} required disabled={busy} value={repeat} onChange={(e) => setRepeat(e.target.value)} /></label>
    {/* Only while choosing one: the rule and the lack of recovery matter then, not on every visit. */}
    {next && <div className={panelBody}><Banner id="new-password-help" tone="warn" compact><Icon name="alert" />{t("passwordPage.hint")}</Banner></div>}
    {message && <div className={cx(panelBody, "[.panel-body+&]:border-t")}><Banner tone={failed ? "danger" : "ok"} role={failed ? "alert" : "status"}><Icon name={failed ? "alert" : "check"} />{"translationKey" in message ? t(message.translationKey) : formatAppError(message)}</Banner></div>}
    <footer className={settingRow}><Button type="submit" variant="primary" disabled={busy || current.length < 8 || next.length < 8 || next !== repeat}>{busy ? t("common.saving") : t("passwordPage.change")}</Button></footer>
  </form>;
}
