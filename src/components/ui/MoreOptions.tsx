import React, { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown } from "lucide-react";

interface MoreOptionsProps {
  /** Rarely changed rows of the surrounding SettingsGroup. */
  children: React.ReactNode;
}

/**
 * The last row of a SettingsGroup: "More options" that unfolds the group's
 * rarely changed settings in place. Keeps every page down to the handful of
 * rows most people touch, without moving anything to another page.
 */
export const MoreOptions: React.FC<MoreOptionsProps> = ({ children }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const contentId = useId();
  if (React.Children.toArray(children).filter(Boolean).length === 0)
    return null;

  return (
    <div>
      {open && (
        <div id={contentId} className="divide-y divide-line">
          {children}
        </div>
      )}
      <button
        type="button"
        aria-expanded={open}
        aria-controls={contentId}
        onClick={() => setOpen((value) => !value)}
        className={`flex w-full cursor-pointer items-center gap-1.5 px-3.5 py-2 text-start text-xs font-medium text-sub transition-colors last:rounded-b-[10px] hover:bg-chip/60 hover:text-text focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50 ${
          open ? "border-t border-line" : ""
        }`}
      >
        <ChevronDown
          className={`h-3.5 w-3.5 shrink-0 transition-transform ${open ? "rotate-180" : ""}`}
          aria-hidden
        />
        {open ? t("settings.moreOptions.hide") : t("settings.moreOptions.show")}
      </button>
    </div>
  );
};
