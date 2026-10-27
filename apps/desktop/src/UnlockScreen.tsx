import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatAppError, type ErrorState } from "./appError";
import { LanguageSelect } from "./LanguageSelector";
import { usePreventUnload } from "./windowState";
import { Banner, Button, IconButton, field } from "./ui";

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

  return <main className="flex-1 min-h-0 overflow-auto flex flex-col items-center justify-center gap-4 px-4 py-6 [background:radial-gradient(600px_300px_at_50%_0%,var(--accent-soft),transparent_70%),var(--bg)] [&_input::-webkit-caps-lock-indicator]:hidden [&_input::-webkit-caps-lock-indicator]:appearance-none [&_input::-webkit-caps-lock-indicator]:content-none">
    {/* unlock-panel: a hook for the visual tests. */}
    <section className="unlock-panel grid gap-[18px] w-[min(360px,100%)] p-[22px] border border-line rounded-xl bg-surface shadow-pop" aria-busy={busy || (loading && !error)}>
      {loading ? <div className="grid justify-items-center gap-2.5 py-3" role="status">
        <Icon name={error ? "alert" : "lock"} className={`size-[22px] ${error ? "text-danger" : "animate-pulse text-accent-text"}`} />
        <strong>{error ? t("unlock.openError") : t("unlock.opening")}</strong>
        {error && <Button disabled={busy} onClick={retry}><Icon name="refresh" />{t("common.retry")}</Button>}
      </div> : resetting && !creating ? <form className="grid gap-3.5" onSubmit={(e) => {
        e.preventDefault();
        setDismissedError("");
        void reset().then((ok) => { if (ok) { setResetting(false); setResetWord(""); } });
      }}>
        <div>
          <h2 className="text-base">{t("unlock.resetTitle")}</h2>
          <p className="text-fg-3 text-sm">{t("unlock.resetHint")}</p>
        </div>
        <label className={field}>
          <span>{t("unlock.resetConfirm", { word: t("unlock.resetWord") })}</span>
          <input autoFocus autoComplete="off" spellCheck={false} value={resetWord} onChange={(e) => edit(setResetWord)(e.target.value)} disabled={busy} required />
        </label>
        {formError && <Banner tone="danger" role="alert"><Icon name="alert" />{formError}</Banner>}
        <div className="flex items-center justify-end gap-2">
          <Button variant="ghost" disabled={busy} onClick={() => { setResetting(false); setResetWord(""); if (error) setDismissedError(error); }}>{t("common.cancel")}</Button>
          <Button type="submit" variant="danger" disabled={busy || resetWord.trim().toLocaleUpperCase() !== t("unlock.resetWord").toLocaleUpperCase()}>
            {busy && <Icon name="refresh" className="animate-spin" />}{t("unlock.resetAction")}
          </Button>
        </div>
      </form> : <form className="grid gap-3.5" onSubmit={(e) => {
        e.preventDefault();
        setDismissedError("");
        const pending = submit(password);
        // Keep a new password on failure so creating can be retried without retyping both fields.
        if (!creating) setPassword("");
        void pending.then((ok) => { if (ok) { setPassword(""); setRepeat(""); setConfirm(false); } });
      }}>
        <h2 className="text-base">{creating ? t("unlock.createTitle") : t("unlock.lockedTitle")}</h2>
        <label className={field}>
          <span id="unlock-password-label">{t("unlock.password")}</span>
          <div className="relative">
            <input className={capsLock ? "pr-[62px]" : "pr-[42px]"} ref={passwordRef} aria-labelledby="unlock-password-label" autoComplete={creating ? "new-password" : "current-password"} autoFocus type={showPassword ? "text" : "password"} minLength={8} value={password} onChange={(e) => edit(setPassword)(e.target.value)} onKeyDown={trackCapsLock} onKeyUp={trackCapsLock} disabled={busy} required aria-invalid={formError ? true : undefined} aria-describedby={described} />
            {capsLock && <span className="absolute top-1/2 right-10 -translate-y-1/2 flex text-warn" id="unlock-caps" role="img" aria-label={t("unlock.capsLock")} data-tip={t("unlock.capsLock")}><Icon name="alert" className="size-3.5" /></span>}
            <IconButton className="absolute top-1/2 right-1 -translate-y-1/2 enabled:hover:bg-transparent!" aria-label={t(showPassword ? "unlock.hidePassword" : "unlock.showPassword")} aria-pressed={showPassword} disabled={busy} onClick={() => setShowPassword((visible) => !visible)}>
              <Icon name={showPassword ? "eyeOff" : "eye"} />
            </IconButton>
          </div>
          {creating && password !== "" && <div className="grid grid-cols-4 gap-1" aria-hidden="true">
            {[0, 1, 2, 3].map((i) => <i key={i} className={`h-[3px] rounded-xs ${i >= Math.max(strength, 1) ? "bg-line" : strength < 2 ? "bg-danger" : strength < 3 ? "bg-warn" : "bg-ok"}`} />)}
          </div>}
          {creating && <small className="text-fg-3 text-xs" id="unlock-password-help">{password === "" ? t("unlock.minLength") : `${t("unlock.minLength")} ${t("unlock.strength", { level: t(`unlock.strengthLevels.${strengthLabels[strength]}`) })}`}</small>}
        </label>
        {creating && <label className={field}>
          <span id="unlock-repeat-label">{t("unlock.repeat")}</span>
          <input aria-labelledby="unlock-repeat-label" autoComplete="new-password" type={showPassword ? "text" : "password"} minLength={8} value={repeat} onChange={(e) => edit(setRepeat)(e.target.value)} onKeyDown={trackCapsLock} onKeyUp={trackCapsLock} disabled={busy} required aria-invalid={mismatch || undefined} aria-describedby={mismatch ? "unlock-mismatch" : undefined} />
          {mismatch && <small className="text-danger text-xs" id="unlock-mismatch">{t("unlock.mismatch")}</small>}
        </label>}
        {formError && <Banner tone="danger" id="unlock-error" role="alert"><Icon name="alert" />{formError}</Banner>}
        {creating && <label className="flex gap-2 items-start text-fg-2 text-sm cursor-pointer"><input type="checkbox" checked={confirm} disabled={busy} onChange={(e) => setConfirm(e.target.checked)} /><span>{t("unlock.lossWarning")}</span></label>}
        <Button type="submit" variant="primary" size="lg" full disabled={busy || password.length < 8 || (creating && (!confirm || repeat !== password))}>
          <Icon name={busy ? "refresh" : creating ? "key" : "unlock"} className={busy ? "animate-spin" : undefined} />{busy ? t("common.processing") : creating ? t("unlock.createTitle") : t("unlock.unlock")}
        </Button>
        {!creating && <Button variant="ghost" size="sm" className="justify-self-center" disabled={busy} onClick={() => { setResetting(true); setPassword(""); if (error) setDismissedError(error); }}>{t("unlock.forgot")}</Button>}
      </form>}
    </section>
    <div className="flex items-center gap-2 text-fg-3"><Icon name="globe" /><LanguageSelect className="min-w-[150px]" /></div>
  </main>;
}
