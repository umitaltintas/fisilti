import React from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "../ui/Button";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";

interface PortalShortcutInputProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  shortcutId: string;
  disabled?: boolean;
  /** Trigger the desktop bound for this shortcut, if it has reported one. */
  trigger?: string;
  error: string | null;
  openSystemSettings: () => Promise<string | null>;
}

/**
 * Shortcut row for the XDG GlobalShortcuts portal backend (Wayland). The
 * desktop owns the key choice, so this shows what it bound and sends the user
 * to the system settings instead of recording keys in the app.
 */
export const PortalShortcutInput: React.FC<PortalShortcutInputProps> = ({
  descriptionMode = "inline",
  grouped = false,
  shortcutId,
  disabled = false,
  trigger,
  error,
  openSystemSettings,
}) => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const binding = getSetting("bindings")?.[shortcutId];

  const name = t(
    `settings.general.shortcut.bindings.${shortcutId}.name`,
    binding?.name ?? shortcutId,
  );
  const description = t(
    `settings.general.shortcut.bindings.${shortcutId}.description`,
    binding?.description ?? "",
  );

  const handleOpen = async () => {
    const failure = await openSystemSettings();
    if (failure) {
      toast.error(t("settings.general.shortcut.portal.openFailed"), {
        description: failure,
      });
    }
  };

  return (
    <SettingContainer
      title={name}
      description={`${description} ${t("settings.general.shortcut.portal.note")}`}
      descriptionMode={descriptionMode}
      grouped={grouped}
      disabled={disabled}
      layout="horizontal"
    >
      <div className="flex items-center space-x-2">
        <span
          className="px-2.5 py-1 text-[13px] bg-chip rounded-md font-medium"
          title={error ?? undefined}
        >
          {trigger || t("settings.general.shortcut.portal.notAssigned")}
        </span>
        <Button variant="secondary" size="sm" onClick={() => void handleOpen()}>
          {t("settings.general.shortcut.portal.configure")}
        </Button>
      </div>
    </SettingContainer>
  );
};
