import * as AlertDialog from "@radix-ui/react-alert-dialog";
import { useEffect, useState, type ReactNode, type RefObject } from "react";
import { Icon } from "./Icon";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "./appError";
import { usePreventUnload } from "./windowState";
import { Banner, Button, tile } from "./ui";

/** Shared with the command palette. A transform, not translate utilities, because the `pop` keyframes animate it. */
export const dialogOverlay = "fixed inset-0 z-50 bg-overlay animate-[fade_.12s_ease-out]";
export const dialogContent = "fixed left-1/2 top-1/2 [transform:translate(-50%,-50%)] z-51 max-h-[calc(100dvh-48px)] overflow-auto grid rounded-xl bg-surface text-fg shadow-pop animate-[pop_.14s_ease-out]";

type ConfirmDialogProps = {
  open: boolean;
  busy: boolean;
  title: string;
  description: string;
  confirmLabel: string;
  destructive?: boolean;
  tone?: "info" | "warn" | "danger";
  children?: ReactNode;
  confirmDisabled?: boolean;
  /** Element to focus on open instead of the safe default (Cancel). */
  initialFocus?: RefObject<HTMLElement | null>;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => Promise<void>;
};

export function ConfirmDialog({
  open,
  busy,
  title,
  description,
  confirmLabel,
  destructive = false,
  tone = destructive ? "danger" : "info",
  children,
  confirmDisabled = false,
  initialFocus,
  onOpenChange,
  onConfirm,
}: ConfirmDialogProps) {
  usePreventUnload(open || busy);
  const { t } = useTranslation();
  const [error, setError] = useState<ErrorState>("");
  useEffect(() => { if (open) setError(""); }, [open]);
  return (
    <AlertDialog.Root open={open} onOpenChange={onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className={dialogOverlay} />
        <AlertDialog.Content className={`${dialogContent} w-[min(500px,calc(100%-32px))] gap-3.5 p-[18px]`} onOpenAutoFocus={initialFocus ? (event) => { event.preventDefault(); initialFocus.current?.focus(); } : undefined}>
          <header className="flex gap-3 items-start">
            <span className={tile(tone)}><Icon name={tone === "info" ? "shield" : "alert"} /></span>
            <div className="grow min-w-0">
              <AlertDialog.Title className="text-lg font-semibold">{title}</AlertDialog.Title>
              <AlertDialog.Description className="mt-[3px] text-fg-2 text-sm whitespace-pre-wrap wrap-anywhere">{description}</AlertDialog.Description>
            </div>
          </header>
          {children && <div className="grid gap-3">{children}</div>}
          {error && <Banner tone="danger" role="alert"><Icon name="alert" />{formatAppError(error)}</Banner>}
          <div className="flex justify-end gap-2 pt-1">
            <AlertDialog.Cancel asChild>
              <Button disabled={busy}>{t("common.back")}</Button>
            </AlertDialog.Cancel>
            <AlertDialog.Action asChild>
              <Button
                variant={destructive ? "danger" : "primary"}
                disabled={busy || confirmDisabled}
                onClick={(event) => {
                  event.preventDefault();
                  setError("");
                  void onConfirm().then(() => onOpenChange(false)).catch((reason) => setError(normalizeAppError(reason)));
                }}
              >
                {busy ? t("common.processing") : confirmLabel}
              </Button>
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
