import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertCircle, Check, Loader2 } from "lucide-react";
import { Input } from "../../ui/Input";

interface ApiKeyFieldProps {
  value: string;
  /** Persist the key. Resolve `false` when it was not saved (the caller's
   * store already reported why). */
  onCommit: (value: string) => Promise<boolean> | boolean | void;
  disabled: boolean;
  placeholder?: string;
  className?: string;
  /** Accessible name when no visible `<label>` points at the field. */
  ariaLabel?: string;
}

type CommitState = "idle" | "saving" | "saved" | "failed";

const SAVED_FEEDBACK_MS = 2000;

/**
 * The one API-key input: a password field edited as a draft and committed on
 * blur (or Enter). It commits only when the key actually changed — every
 * commit clears the provider's fetched model list, so committing on every
 * blur used to wipe it just for tabbing through — and says whether the save
 * worked.
 */
export const ApiKeyField: React.FC<ApiKeyFieldProps> = React.memo(
  ({ value, onCommit, disabled, placeholder, className = "", ariaLabel }) => {
    const { t } = useTranslation();
    const [localValue, setLocalValue] = useState(value);
    const [state, setState] = useState<CommitState>("idle");
    const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

    // Sync with the persisted value when it changes elsewhere.
    useEffect(() => {
      setLocalValue(value);
    }, [value]);

    useEffect(
      () => () => {
        if (timerRef.current) clearTimeout(timerRef.current);
      },
      [],
    );

    const commit = async () => {
      const next = localValue.trim();
      if (next === value.trim()) {
        // Nothing changed; just normalize stray whitespace in the box.
        if (localValue !== value) setLocalValue(value);
        return;
      }
      if (timerRef.current) clearTimeout(timerRef.current);
      setState("saving");
      const saved = await onCommit(next);
      if (saved === false) {
        setState("failed");
        return;
      }
      setState("saved");
      timerRef.current = setTimeout(() => setState("idle"), SAVED_FEEDBACK_MS);
    };

    return (
      <div className={`flex flex-1 items-center gap-2 ${className}`}>
        <Input
          type="password"
          value={localValue}
          onChange={(event) => {
            setLocalValue(event.target.value);
            if (state === "failed") setState("idle");
          }}
          onBlur={() => void commit()}
          onKeyDown={(event) => {
            if (event.key === "Enter") void commit();
          }}
          placeholder={placeholder}
          aria-label={ariaLabel}
          aria-invalid={state === "failed" || undefined}
          variant="compact"
          disabled={disabled}
          className="flex-1 min-w-[320px]"
        />
        <span
          className="flex w-4 shrink-0 items-center justify-center"
          role="status"
        >
          {state === "saving" && (
            <Loader2
              className="h-3.5 w-3.5 animate-spin text-text/50"
              aria-label={t("settings.apiKey.saving")}
            />
          )}
          {state === "saved" && (
            <Check
              className="h-3.5 w-3.5 text-emerald-500"
              aria-label={t("settings.apiKey.saved")}
            />
          )}
          {state === "failed" && (
            <AlertCircle
              className="h-3.5 w-3.5 text-red-400"
              aria-label={t("settings.apiKey.failed")}
            />
          )}
        </span>
      </div>
    );
  },
);

ApiKeyField.displayName = "ApiKeyField";
