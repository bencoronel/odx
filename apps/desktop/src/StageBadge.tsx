import type { Stage } from "./ipc";
import { useTranslation } from "react-i18next";
import { Segment, Segmented, cx, tones, type Tone } from "./ui";

/** Ordered by operational risk (PRO first). */
export const STAGES: Stage[] = ["pro", "pre", "test", "dev"];

export const stageTone: Record<Stage, Tone> = {
  pro: "danger",
  pre: "warn",
  test: "info",
  dev: "ok",
};

export const stageRank = (stage?: Stage | null) => {
  const index = stage ? STAGES.indexOf(stage) : -1;
  return index < 0 ? STAGES.length : index;
};

export const stageShortLabel: Record<Stage, string> = {
  pro: "PRO",
  pre: "PRE",
  test: "TEST",
  dev: "DEV",
};

export function StageBadge({ stage, compact = false, className }: { stage?: Stage | null; compact?: boolean; className?: string }) {
  const { t } = useTranslation();
  if (!stage) return null;
  const label = stageShortLabel[stage];
  return <span
    className={cx(
      "inline-flex items-center justify-center flex-none rounded-sm font-mono font-semibold tracking-[.03em] text-tone bg-tone-soft border border-tone/25",
      tones[stageTone[stage]],
      compact ? "h-4 px-1 text-3xs" : "h-5 px-1.5 text-2xs",
      className,
    )}
    data-tip={t(`stage.${stage}` as "stage.pro")}
  >{label}</span>;
}

/** None + DEV / TEST / PRE / PRO selector. */
export function StagePicker({ value, disabled, onChange }: {
  value: Stage | null;
  disabled?: boolean;
  onChange: (stage: Stage | null) => void;
}) {
  const { t } = useTranslation();
  const options: { value: Stage | null; label: string }[] = [
    { value: null, label: t("stage.none") },
    { value: "dev", label: "DEV" },
    { value: "test", label: "TEST" },
    { value: "pre", label: "PRE" },
    { value: "pro", label: "PRO" },
  ];
  return <Segmented className="justify-self-start" role="radiogroup" aria-label={t("stage.label")}>
    {options.map((option) => <Segment
      key={option.value ?? "none"}
      role="radio"
      disabled={disabled}
      aria-checked={value === option.value}
      aria-pressed={value === option.value}
      className={option.value && value === option.value ? cx(tones[stageTone[option.value]], "text-tone!") : undefined}
      onClick={() => onChange(option.value)}
    >{option.label}</Segment>)}
  </Segmented>;
}
