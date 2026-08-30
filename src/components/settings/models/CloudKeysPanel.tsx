import React from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { KeyRound } from "lucide-react";
import { SettingContainer } from "@/components/ui/SettingContainer";
import { SettingsGroup } from "@/components/ui/SettingsGroup";
import { ApiKeyField } from "../PostProcessingSettingsApi/ApiKeyField";
import { useSettingsStore } from "@/stores/settingsStore";

interface ApiKeyRowProps {
  title: string;
  description: string;
  value: string;
  onCommit: (value: string) => void;
  disabled: boolean;
  consoleUrl: string;
}

/**
 * One credential. Every key row on every page is this component, so a key never
 * looks or behaves differently depending on where you found it — the panel used
 * to mix a bound password field with a write-only one whose state lived in its
 * placeholder, which read as two unrelated controls.
 */
const ApiKeyRow: React.FC<ApiKeyRowProps> = ({
  title,
  description,
  value,
  onCommit,
  disabled,
  consoleUrl,
}) => {
  const { t } = useTranslation();

  return (
    <SettingContainer
      title={title}
      description={description}
      descriptionMode="tooltip"
      layout="horizontal"
      grouped
    >
      <div className="flex items-center gap-2">
        <ApiKeyField
          value={value}
          onBlur={onCommit}
          disabled={disabled}
          placeholder={t("settings.models.keys.placeholder")}
        />
        <button
          type="button"
          onClick={() => openUrl(consoleUrl)}
          className="cursor-pointer whitespace-nowrap text-xs text-logo-primary hover:underline"
        >
          {t("settings.models.keys.getKey")}
        </button>
      </div>
    </SettingContainer>
  );
};

/**
 * Every cloud credential the app uses, in one always-visible place.
 *
 * Keys used to be attached to whichever model happened to be active, which
 * meant the Gemini field only existed while a Gemini model was the *dictation*
 * model — so anyone running local dictation with a Gemini meeting model had
 * nowhere to paste it. A key is account-level, not model-level, so it belongs
 * here rather than inside a model's panel, and the AI-editing tab links here
 * instead of growing a second field for the same account.
 */
export const CloudKeysPanel: React.FC = () => {
  const { t } = useTranslation();
  const { settings, updateSetting, updatePostProcessApiKey, isUpdatingKey } =
    useSettingsStore();

  return (
    <SettingsGroup title={t("settings.models.keys.title")}>
      <div className="flex items-center gap-3 px-4 py-3">
        <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-logo-primary/15 text-logo-primary">
          <KeyRound className="h-4 w-4" />
        </div>
        <p className="text-xs text-text/60">
          {t("settings.models.keys.description")}
        </p>
      </div>

      <ApiKeyRow
        title={t("settings.models.keys.openrouter")}
        description={t("settings.models.keys.openrouterHint")}
        value={settings?.post_process_api_keys?.["openrouter"] ?? ""}
        onCommit={(value) => void updatePostProcessApiKey("openrouter", value)}
        disabled={isUpdatingKey("post_process_api_key:openrouter")}
        consoleUrl="https://openrouter.ai/keys"
      />

      <ApiKeyRow
        title={t("settings.models.keys.gemini")}
        description={t("settings.models.keys.geminiHint")}
        value={settings?.gemini_api_key ?? ""}
        onCommit={(value) => void updateSetting("gemini_api_key", value)}
        disabled={isUpdatingKey("gemini_api_key")}
        consoleUrl="https://aistudio.google.com/apikey"
      />
    </SettingsGroup>
  );
};
