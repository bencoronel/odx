import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatAppError, type ErrorState } from "./appError";
import { LanguageSelect } from "./LanguageSelector";
import { usePreventUnload } from "./windowState";

/** Rough 0–4 score from length and character variety; it only informs, it never blocks. */
export function passwordStrength(password: string) {
  return Math.min(4, [password.length >= 12, password.length >= 16, /[a-z]/.test(password) && /[A-Z]/.test(password), /\d/.test(password), /[^A-Za-z0-9]/.test(password)].filter(Boolean).length);
}
const strengthLabels = ["weak", "weak", "fair", "good", "strong"] as const;

export default function UnlockScreen({ status, busy, error, retry, submit, reset }: {
  status: string;
  busy: boolean;
  error: ErrorState;
  retry: () => void;
  submit: (password: string) => Promise<boolean>;
  reset: () => Promise<boolean>;
}) {
  const { t } = useTranslation();
  const [password, setPassword] = useState("");
  const [repeat, setRepeat] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [capsLock, setCapsLock] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [resetWord, setResetWord] = useState("");
  // Hides the last error once the user starts typing a new attempt.
  const [dismissedError, setDismissedError] = useState<ErrorState>("");
  const passwordRef = useRef<HTMLInputElement>(null);
  usePreventUnload(busy || password !== "" || repeat !== "" || confirm || resetWord !== "");
  const creating = status === "absent";
  const loading = status === "loading";
  const formError = error && error !== dismissedError && !loading ? formatAppError(error) : "";
  const mismatch = creating && repeat !== "" && repeat !== password;
  // The input is disabled while busy, which drops focus; give it back for the next attempt.
  useEffect(() => { if (!busy) passwordRef.current?.focus(); }, [busy]);
  const trackCapsLock = (e: KeyboardEvent) => setCapsLock(e.getModifierState("CapsLock"));
  const edit = (set: (value: string) => void) => (value: string) => { set(value); if (error) setDismissedError(error); };
  const strength = passwordStrength(password);
  const described = [creating && "unlock-password-help", capsLock && "unlock-caps", formError && "unlock-error"].filter(Boolean).join(" ") || undefined;

  return <main className="unlock">
    <section className="unlock-panel" aria-busy={busy || (loading && !error)}>
      {loading ? <div className="unlock-loading" role="status">
        <Icon name={error ? "alert" : "lock"} className={error ? "text-danger" : "spin-soft"} />
        <strong>{error ? t("unlock.openError") : t("unlock.opening")}</strong>
        {error && <button className="btn" disabled={busy} onClick={retry}><Icon name="refresh" />{t("common.retry")}</button>}
      </div> : resetting && !creating ? <form className="stack" onSubmit={(e) => {
        e.preventDefault();
        setDismissedError("");
        void reset().then((ok) => { if (ok) { setResetting(false); setResetWord(""); } });
      }}>
        <div>
          <h2>{t("unlock.resetTitle")}</h2>
          <p className="hint">{t("unlock.resetHint")}</p>
        </div>
        <label className="field">
          <span>{t("unlock.resetConfirm", { word: t("unlock.resetWord") })}</span>
          <input autoFocus autoComplete="off" spellCheck={false} value={resetWord} onChange={(e) => edit(setResetWord)(e.target.value)} disabled={busy} required />
        </label>
        {formError && <p className="banner danger" role="alert"><Icon name="alert" />{formError}</p>}
        <div className="row justify-end gap-2">
          <button type="button" className="btn ghost" disabled={busy} onClick={() => { setResetting(false); setResetWord(""); if (error) setDismissedError(error); }}>{t("common.cancel")}</button>
          <button className="btn danger" disabled={busy || resetWord.trim().toLocaleUpperCase() !== t("unlock.resetWord").toLocaleUpperCase()}>
            {busy && <Icon name="refresh" className="spin" />}{t("unlock.resetAction")}
          </button>
        </div>
      </form> : <form className="stack" onSubmit={(e) => {
        e.preventDefault();
        setDismissedError("");
        const pending = submit(password);
        // Keep a new password on failure so creating can be retried without retyping both fields.
        if (!creating) setPassword("");
        void pending.then((ok) => { if (ok) { setPassword(""); setRepeat(""); setConfirm(false); } });
      }}>
        <h2>{creating ? t("unlock.createTitle") : t("unlock.lockedTitle")}</h2>
        <label className="field">
          <span id="unlock-password-label">{t("unlock.password")}</span>
          <div className="unlock-password-control">
            <input ref={passwordRef} aria-labelledby="unlock-password-label" autoComplete={creating ? "new-password" : "current-password"} autoFocus type={showPassword ? "text" : "password"} minLength={8} value={password} onChange={(e) => edit(setPassword)(e.target.value)} onKeyDown={trackCapsLock} onKeyUp={trackCapsLock} disabled={busy} required aria-invalid={formError ? true : undefined} aria-describedby={described} />
            {capsLock && <span className="unlock-caps" id="unlock-caps" role="img" aria-label={t("unlock.capsLock")} data-tip={t("unlock.capsLock")}><Icon name="alert" /></span>}
            <button type="button" className="icon-btn unlock-password-toggle" aria-label={t(showPassword ? "unlock.hidePassword" : "unlock.showPassword")} aria-pressed={showPassword} disabled={busy} onClick={() => setShowPassword((visible) => !visible)}>
              <Icon name={showPassword ? "eyeOff" : "eye"} />
            </button>
          </div>
          {creating && password !== "" && <div className="unlock-strength" data-level={strength} aria-hidden="true"><i /><i /><i /><i /></div>}
          {creating && <small className="hint" id="unlock-password-help">{password === "" ? t("unlock.minLength") : `${t("unlock.minLength")} ${t("unlock.strength", { level: t(`unlock.strengthLevels.${strengthLabels[strength]}`) })}`}</small>}
        </label>
        {creating && <label className="field">
          <span id="unlock-repeat-label">{t("unlock.repeat")}</span>
          <input aria-labelledby="unlock-repeat-label" autoComplete="new-password" type={showPassword ? "text" : "password"} minLength={8} value={repeat} onChange={(e) => edit(setRepeat)(e.target.value)} onKeyDown={trackCapsLock} onKeyUp={trackCapsLock} disabled={busy} required aria-invalid={mismatch || undefined} aria-describedby={mismatch ? "unlock-mismatch" : undefined} />
          {mismatch && <small className="hint text-danger" id="unlock-mismatch">{t("unlock.mismatch")}</small>}
        </label>}
        {formError && <p className="banner danger" id="unlock-error" role="alert"><Icon name="alert" />{formError}</p>}
        {creating && <label className="check"><input type="checkbox" checked={confirm} disabled={busy} onChange={(e) => setConfirm(e.target.checked)} /><span>{t("unlock.lossWarning")}</span></label>}
        <button className="btn primary full" disabled={busy || password.length < 8 || (creating && (!confirm || repeat !== password))}>
          <Icon name={busy ? "refresh" : creating ? "key" : "unlock"} className={busy ? "spin" : undefined} />{busy ? t("common.processing") : creating ? t("unlock.createTitle") : t("unlock.unlock")}
        </button>
        {!creating && <button type="button" className="btn ghost sm unlock-forgot" disabled={busy} onClick={() => { setResetting(true); setPassword(""); if (error) setDismissedError(error); }}>{t("unlock.forgot")}</button>}
      </form>}
    </section>
    <div className="unlock-language"><Icon name="globe" /><LanguageSelect className="unlock-language-select" /></div>
  </main>;
}
