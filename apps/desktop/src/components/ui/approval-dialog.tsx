import * as AlertDialog from "@radix-ui/react-alert-dialog";
import { useEffect, useState, type ReactNode, type RefObject } from "react";
import { Icon } from "@/Icon";
import { useTranslation } from "react-i18next";
import { formatAppError, normalizeAppError, type ErrorState } from "@/appError";
import { usePreventUnload } from "@/windowState";

type ApprovalDialogProps = {
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

export function ApprovalDialog({
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
}: ApprovalDialogProps) {
  usePreventUnload(open || busy);
  const { t } = useTranslation();
  const [error, setError] = useState<ErrorState>("");
  useEffect(() => { if (open) setError(""); }, [open]);
  return (
    <AlertDialog.Root open={open} onOpenChange={onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay" />
        <AlertDialog.Content className="dialog-content" onOpenAutoFocus={initialFocus ? (event) => { event.preventDefault(); initialFocus.current?.focus(); } : undefined}>
          <header className="dialog-header">
            <span className={`tile tone-${tone}`}><Icon name={tone === "info" ? "shield" : "alert"} /></span>
            <div className="grow min-w-0">
              <AlertDialog.Title className="dialog-title">{title}</AlertDialog.Title>
              <AlertDialog.Description className="dialog-description">{description}</AlertDialog.Description>
            </div>
          </header>
          {children && <div className="dialog-body">{children}</div>}
          {error && <p className="banner danger" role="alert"><Icon name="alert" />{formatAppError(error)}</p>}
          <div className="dialog-actions">
            <AlertDialog.Cancel asChild>
              <button className="btn" disabled={busy}>{t("common.back")}</button>
            </AlertDialog.Cancel>
            <AlertDialog.Action asChild>
              <button
                className={`btn ${destructive ? "danger" : "primary"}`}
                disabled={busy || confirmDisabled}
                onClick={(event) => {
                  event.preventDefault();
                  setError("");
                  void onConfirm().then(() => onOpenChange(false)).catch((reason) => setError(normalizeAppError(reason)));
                }}
              >
                {busy ? t("common.processing") : confirmLabel}
              </button>
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
