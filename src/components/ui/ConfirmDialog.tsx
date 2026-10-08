import React, { useCallback, useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { AlertTriangle } from "lucide-react";
import { Button } from "./Button";

export interface ConfirmOptions {
  title: string;
  /** What will happen, in plain words. */
  description?: string;
  /** Defaults to "Confirm" / "Delete" depending on `destructive`. */
  confirmLabel?: string;
  cancelLabel?: string;
  /** Red confirm button and a warning icon. Use for anything that loses
   * data. */
  destructive?: boolean;
}

interface ConfirmDialogProps extends ConfirmOptions {
  open: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * Modal "are you sure?" dialog. The one confirmation primitive: every
 * destructive action (delete, discard, replace, leave with unsaved edits)
 * asks through this, so they all read and behave the same — Escape or a click
 * outside cancels, focus starts on Cancel and stays inside the dialog.
 */
export const ConfirmDialog: React.FC<ConfirmDialogProps> = ({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel,
  destructive = false,
  onConfirm,
  onCancel,
}) => {
  const { t } = useTranslation();
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  const onCancelRef = useRef(onCancel);
  onCancelRef.current = onCancel;

  useEffect(() => {
    if (!open) return;
    const previouslyFocused = document.activeElement as HTMLElement | null;
    cancelRef.current?.focus();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onCancelRef.current();
        return;
      }
      if (event.key === "Tab") trapFocus(event);
    };
    document.addEventListener("keydown", handleKeyDown, true);
    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
      previouslyFocused?.focus?.();
    };
  }, [open]);

  if (!open) return null;

  function trapFocus(event: KeyboardEvent) {
    // Keep focus inside the dialog.
    const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(
      "button:not([disabled])",
    );
    if (!focusable || focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
      role="presentation"
    >
      <div
        ref={dialogRef}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        className="w-full max-w-sm rounded-lg border border-mid-gray/20 bg-background p-5 shadow-xl space-y-4"
      >
        <div className="flex items-start gap-3">
          {destructive && (
            <div className="shrink-0 rounded-full bg-red-500/15 p-2 text-red-400">
              <AlertTriangle width={16} height={16} aria-hidden />
            </div>
          )}
          <div className="min-w-0 space-y-1">
            <h2 id={titleId} className="text-sm font-semibold text-text">
              {title}
            </h2>
            {description && (
              <p
                id={descriptionId}
                className="text-sm text-text/70 whitespace-pre-wrap break-words"
              >
                {description}
              </p>
            )}
          </div>
        </div>
        <div className="flex justify-end gap-2">
          <Button
            ref={cancelRef}
            variant="secondary"
            size="md"
            onClick={onCancel}
          >
            {cancelLabel ?? t("common.cancel")}
          </Button>
          <Button
            variant={destructive ? "danger" : "primary"}
            size="md"
            onClick={onConfirm}
          >
            {confirmLabel ??
              (destructive ? t("common.delete") : t("common.confirm"))}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
};

/**
 * Promise-based confirmation: `if (await confirm({...})) doIt();`. Render the
 * returned `dialog` once anywhere in the component.
 */
export function useConfirm(): {
  confirm: (options: ConfirmOptions) => Promise<boolean>;
  dialog: React.ReactNode;
} {
  const [pending, setPending] = useState<{
    options: ConfirmOptions;
    resolve: (value: boolean) => void;
  } | null>(null);
  const pendingRef = useRef(pending);
  pendingRef.current = pending;

  // An unmount with a question still open answers "no".
  useEffect(() => () => pendingRef.current?.resolve(false), []);

  const confirm = useCallback(
    (options: ConfirmOptions) =>
      new Promise<boolean>((resolve) => {
        pendingRef.current?.resolve(false);
        setPending({ options, resolve });
      }),
    [],
  );

  const settle = (value: boolean) => {
    pending?.resolve(value);
    setPending(null);
  };

  const dialog = pending ? (
    <ConfirmDialog
      open
      {...pending.options}
      onConfirm={() => settle(true)}
      onCancel={() => settle(false)}
    />
  ) : null;

  return { confirm, dialog };
}
