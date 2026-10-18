import { useEffect, useId, useRef, useState } from "react";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";

export default function CopyButton({ text, label, ariaLabel, className = "btn sm", tip }: {
  text: string | (() => string);
  label?: string;
  ariaLabel?: string;
  className?: string;
  tip?: string;
}) {
  const { t } = useTranslation();
  const [state, setState] = useState<"idle" | "copied" | "error">("idle");
  const timer = useRef(0);
  const request = useRef(0);
  const errorId = useId();
  useEffect(() => () => { window.clearTimeout(timer.current); request.current++; }, []);

  async function copy() {
    const current = ++request.current;
    window.clearTimeout(timer.current);
    setState("idle");
    try {
      await navigator.clipboard.writeText(typeof text === "function" ? text() : text);
      if (current !== request.current) return;
      setState("copied");
      timer.current = window.setTimeout(() => setState("idle"), 2000);
    } catch {
      if (current === request.current) setState("error");
    }
  }

  const buttonLabel = label ?? t("common.copy");
  return <>
    <button type="button" className={className} data-tip={tip} data-copy-state={state}
      aria-label={ariaLabel ?? (state === "copied" ? t("common.copied") : buttonLabel)} aria-describedby={state === "error" ? errorId : undefined}
      onClick={(event) => { event.stopPropagation(); void copy(); }}>
      <Icon name={state === "copied" ? "check" : state === "error" ? "alert" : "copy"} />
      {state === "copied" ? t("common.copied") : buttonLabel}
    </button>
    <span className="sr-only" role="status">{state === "copied" ? t("common.copiedToClipboard") : ""}</span>
    {state === "error" && <span id={errorId} className="copy-error" role="alert">{t("common.copyError")}</span>}
  </>;
}
