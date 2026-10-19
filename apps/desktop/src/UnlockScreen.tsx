import { useState } from "react";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatAppError, type ErrorState } from "./appError";
import { LanguageSelect } from "./LanguageSelector";
import { usePreventUnload } from "./windowState";

export default function UnlockScreen({ status, busy, error, retry, submit }: {
  status: string;
  busy: boolean;
  error: ErrorState;
  retry: () => void;
  submit: (password: string) => Promise<boolean>;
}) {
  const { t } = useTranslation();
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [confirm, setConfirm] = useState(false);
  usePreventUnload(busy || password !== "" || confirm);
  const creating = status === "absent";
  const loading = status === "loading";
  const errorMessage = error ? formatAppError(error) : "";

  return <main className="unlock">
    <section className="unlock-panel" aria-busy={busy || (loading && !error)}>
      {loading ? <div className="unlock-loading" role="status">
        <Icon name={error ? "alert" : "lock"} className={error ? "text-danger" : "spin-soft"} />
        <strong>{error ? t("unlock.openError") : t("unlock.opening")}</strong>
        {error && <button className="btn" disabled={busy} onClick={retry}><Icon name="refresh" />{t("common.retry")}</button>}
      </div> : <form className="stack" onSubmit={(e) => {
        e.preventDefault();
        const pending = submit(password);
        setPassword("");
        void pending.then((ok) => { if (ok) setConfirm(false); });
      }}>
        <div>
          <h2>{creating ? t("unlock.createTitle") : t("unlock.lockedTitle")}</h2>
          <p className="hint">{creating ? t("unlock.createHint") : t("unlock.enterHint")}</p>
        </div>
        <label className="field">
          <span>{t("unlock.password")}</span>
          <div className="unlock-password-control">
            <input autoComplete={creating ? "new-password" : "current-password"} autoFocus type={showPassword ? "text" : "password"} minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} disabled={busy} required />
            <button type="button" className="icon-btn unlock-password-toggle" aria-label={t(showPassword ? "unlock.hidePassword" : "unlock.showPassword")} aria-pressed={showPassword} disabled={busy} onClick={() => setShowPassword((visible) => !visible)}>
              <Icon name={showPassword ? "eyeOff" : "eye"} />
            </button>
          </div>
        </label>
        {creating && <label className="check"><input type="checkbox" checked={confirm} disabled={busy} onChange={(e) => setConfirm(e.target.checked)} /><span>{t("unlock.lossWarning")}</span></label>}
        <button className="btn primary full" disabled={busy || password.length < 8 || (creating && !confirm)}>
          <Icon name={creating ? "key" : "unlock"} />{busy ? t("common.processing") : creating ? t("unlock.createTitle") : t("unlock.unlock")}
        </button>
      </form>}
      {error && !loading && <p className="banner danger" role="alert"><Icon name="alert" />{errorMessage}</p>}
    </section>
    <div className="unlock-language"><Icon name="globe" /><LanguageSelect className="unlock-language-select" /></div>
  </main>;
}
