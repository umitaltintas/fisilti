import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { errorMessage } from "@/lib/utils/errors";

const COPIED_FEEDBACK_MS = 2000;

/**
 * Copy text and report whether it actually worked.
 *
 * `copied` only turns true after the clipboard write resolved, so a failed
 * copy never shows a checkmark; failures get a localized toast instead. The
 * feedback timer is cleared on re-copy and on unmount.
 */
export function useCopyToClipboard(): {
  copied: boolean;
  copy: (text: string) => Promise<boolean>;
} {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    },
    [],
  );

  const copy = useCallback(
    async (text: string) => {
      if (timerRef.current) clearTimeout(timerRef.current);
      try {
        await navigator.clipboard.writeText(text);
      } catch (error) {
        setCopied(false);
        toast.error(t("common.copyFailed"), {
          description: errorMessage(error),
        });
        return false;
      }
      setCopied(true);
      timerRef.current = setTimeout(() => {
        setCopied(false);
        timerRef.current = null;
      }, COPIED_FEEDBACK_MS);
      return true;
    },
    [t],
  );

  return { copied, copy };
}
