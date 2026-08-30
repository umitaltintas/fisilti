import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Cloud, HardDrive } from "lucide-react";
import type { ModelInfo } from "@/bindings";
import { LanguageSelector } from "@/components/settings/LanguageSelector";
import { TranslateToEnglish } from "@/components/settings/TranslateToEnglish";
import { Input } from "@/components/ui/Input";
import { SettingsGroup } from "@/components/ui/SettingsGroup";
import { useSettingsStore } from "@/stores/settingsStore";
import { getTranslatedModelName } from "@/lib/utils/modelTranslation";
import { cloudProviderOf } from "@/lib/utils/model";

interface ActiveModelPanelProps {
  model: ModelInfo | undefined;
}

/**
 * Everything about the model that is *currently* in use: which one it is, and
 * the handful of settings that only apply to it (recognition language,
 * translation, the custom slug). Credentials are deliberately NOT here — an
 * API key belongs to an account, not to whichever model happens to be active.
 */
export const ActiveModelPanel: React.FC<ActiveModelPanelProps> = ({
  model,
}) => {
  const { t } = useTranslation();
  const { settings, updateSetting } = useSettingsStore();

  const isCloud = cloudProviderOf(model?.engine_type) !== null;
  const isCustomCloud =
    model?.id === "openrouter-custom" || model?.id === "openrouter-asr-custom";

  const customModel = settings?.openrouter_custom_model ?? "";
  const [customModelLocal, setCustomModelLocal] = useState(customModel);
  useEffect(() => {
    setCustomModelLocal(customModel);
  }, [customModel]);

  if (!model) {
    return null;
  }

  return (
    <SettingsGroup title={t("settings.models.activeModel")}>
      <div className="flex items-center gap-3 px-4 py-3">
        <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-logo-primary/15 text-logo-primary">
          {isCloud ? (
            <Cloud className="h-4 w-4" />
          ) : (
            <HardDrive className="h-4 w-4" />
          )}
        </div>
        <div className="min-w-0">
          <p className="truncate text-sm font-semibold">
            {getTranslatedModelName(model, t)}
          </p>
          <p className="text-xs text-text/50">
            {isCloud
              ? t("settings.models.badges.cloud")
              : t("settings.models.badges.local")}
          </p>
        </div>
      </div>

      {model.supports_language_selection && (
        <LanguageSelector
          descriptionMode="tooltip"
          grouped={true}
          supportedLanguages={model.supported_languages}
        />
      )}
      {model.supports_translation && (
        <TranslateToEnglish descriptionMode="tooltip" grouped={true} />
      )}

      {/* Only the slug lives here: it identifies THIS model. The API key is
          account-level and lives in the credentials panel below. */}
      {isCustomCloud && (
        <div className="space-y-1 px-4 py-3">
          <label className="text-xs font-medium text-text/70">
            {t("settings.models.cloud.modelLabel")}
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <Input
              type="text"
              value={customModelLocal}
              onChange={(event) => setCustomModelLocal(event.target.value)}
              onBlur={() =>
                updateSetting("openrouter_custom_model", customModelLocal)
              }
              placeholder={t("settings.models.cloud.modelPlaceholder")}
              variant="compact"
              className="min-w-[320px] flex-1"
            />
            <button
              type="button"
              onClick={() => openUrl("https://openrouter.ai/models")}
              className="cursor-pointer text-xs text-logo-primary hover:underline"
            >
              {t("settings.models.cloud.browseModels")}
            </button>
          </div>
          {!customModel.trim() && (
            <p className="text-xs text-amber-500">
              {t("settings.models.cloud.modelRequired")}
            </p>
          )}
        </div>
      )}
    </SettingsGroup>
  );
};
