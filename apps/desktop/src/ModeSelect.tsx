import { useRef, type KeyboardEvent } from "react";
import type { WriteMode } from "./ipc";
import { Icon, type IconName } from "./Icon";
import { useTranslation } from "react-i18next";

export const modeLabels: Record<WriteMode, string> = {
  read_only: "mode.readOnly",
  approval: "mode.approval",
  temporary: "mode.temporary",
  permanent: "mode.permanent",
};

export const modeShortLabels: Record<WriteMode, string> = {
  read_only: "mode.readOnlyShort",
  approval: "mode.approvalShort",
  temporary: "mode.temporaryShort",
  permanent: "mode.permanentShort",
};

/** Risk scale: green, blue, amber, red. */
export const modeTone: Record<WriteMode, "ok" | "info" | "warn" | "danger"> = {
  read_only: "ok",
  approval: "info",
  temporary: "warn",
  permanent: "danger",
};

export const modeIcons: Record<WriteMode, IconName> = {
  read_only: "eye",
  approval: "check",
  temporary: "clock",
  permanent: "infinity",
};

export const modeHelp: Record<WriteMode, string> = {
  read_only: "mode.readOnlyHelp",
  approval: "mode.approvalHelp",
  temporary: "mode.temporaryHelp",
  permanent: "mode.permanentHelp",
};

const modes = Object.keys(modeLabels) as WriteMode[];

export function ModeSelect({ id, value, disabled, onValueChange }: {
  id: string;
  value: WriteMode;
  disabled: boolean;
  onValueChange: (mode: WriteMode) => void;
}) {
  const { t } = useTranslation();
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  function onKeyDown(event: KeyboardEvent, index: number) {
    const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
    if (!step) return;
    event.preventDefault();
    refs.current[(index + step + modes.length) % modes.length]?.focus();
  }
  return <div id={id} className="mode-select" role="radiogroup" aria-label={t("mode.environmentMode")}>
    {modes.map((mode, index) => <button
      key={mode}
      ref={(node) => { refs.current[index] = node; }}
      type="button"
      role="radio"
      aria-checked={value === mode}
      tabIndex={value === mode ? 0 : -1}
      className={`tone-${modeTone[mode]}`}
      aria-label={t(modeLabels[mode] as "mode.readOnly")}
      aria-describedby={`${id}-${mode}-help`}
      disabled={disabled}
      onKeyDown={(event) => onKeyDown(event, index)}
      onClick={() => { if (mode !== value) onValueChange(mode); }}
    ><Icon name={modeIcons[mode]} /><span>{t(modeShortLabels[mode] as "mode.readOnlyShort")}</span>
      <span className="tooltip" role="tooltip" id={`${id}-${mode}-help`}><strong>{t(modeLabels[mode] as "mode.readOnly")}</strong>{t(modeHelp[mode] as "mode.readOnlyHelp")}</span>
    </button>)}
  </div>;
}
