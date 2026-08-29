import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Cloud, HardDrive } from "lucide-react";
import type { ModelInfo } from "@/bindings";
import { ApiKeyField } from "@/components/settings/PostProcessingSettingsApi/ApiKeyField";
import { LanguageSelector } from "@/components/settings/LanguageSelector";
import { TranslateToEnglish } from "@/components/settings/TranslateToEnglish";
import { Input } from "@/components/ui/Input";
import { SettingsGroup } from "@/components/ui/SettingsGroup";
import { useSettingsStore } from "@/stores/settingsStore";
import { getTranslatedModelName } from "@/lib/utils/modelTranslation";
import { cloudProviderOf } from "@/lib/utils/model";
import { changeGeminiApiKey, getMeetingGeminiSettings } from "@/lib/meeting";

interface ActiveModelPanelProps {
  model: ModelInfo | undefined;
}

/**
 * Everything about the model that is *currently* in use: which one it is, and
 * the handful of settings that only apply to it (recognition language,
 * translation, cloud credentials). Previously these lived on three different
 * pages, so changing a model meant hunting for its settings elsewhere.
 */
export const ActiveModelPanel: React.FC<ActiveModelPanelProps> = ({
  model,
}) => {
  const { t } = useTranslation();
  const { settings, updatePostProcessApiKey, updateSetting } =
    useSettingsStore();

  const provider = cloudProviderOf(model?.engine_type);
  const isCloud = provider !== null;
  const isCustomCloud =
    model?.id === "openrouter-custom" || model?.id === "openrouter-asr-custom";

  const openrouterKey = settings?.post_process_api_keys?.["openrouter"] ?? "";
  const customModel = settings?.openrouter_custom_model ?? "";
  const [customModelLocal, setCustomModelLocal] = useState(customModel);
  useEffect(() => {
    setCustomModelLocal(customModel);
  }, [customModel]);

  // The stored Gemini key is write-only — the backend never hands it back — so
  // the field stays empty and only reports whether one exists.
  const [hasGeminiKey, setHasGeminiKey] = useState(false);
  const [geminiKeyDraft, setGeminiKeyDraft] = useState("");
  useEffect(() => {
    if (provider !== "gemini") return;
    let cancelled = false;
    void getMeetingGeminiSettings().then((s) => {
      if (!cancelled) setHasGeminiKey(s.hasApiKey);
    });
    return () => {
      cancelled = true;
    };
  }, [provider]);

  const commitGeminiKey = async (value: string) => {
    const key = value.trim();
    if (key.length === 0) return;
    await changeGeminiApiKey(key);
    setHasGeminiKey(true);
    setGeminiKeyDraft("");
  };

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

      {provider === "gemini" && (
        <div className="space-y-2 px-4 py-3">
          <p className="text-xs text-text/60">
            {t("settings.models.gemini.description")}
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <Input
              type="password"
              value={geminiKeyDraft}
              onChange={(event) => setGeminiKeyDraft(event.target.value)}
              onBlur={() => void commitGeminiKey(geminiKeyDraft)}
              placeholder={
                hasGeminiKey
                  ? t("settings.models.gemini.apiKeyStored")
                  : t("settings.models.gemini.apiKeyPlaceholder")
              }
              variant="compact"
              className="min-w-[320px] flex-1"
            />
            <button
              type="button"
              onClick={() => openUrl("https://aistudio.google.com/apikey")}
              className="text-xs text-logo-primary hover:underline cursor-pointer"
            >
              {t("settings.models.gemini.getKey")}
            </button>
          </div>
          {!hasGeminiKey && (
            <p className="text-xs text-amber-500">
              {t("settings.models.gemini.keyRequired")}
            </p>
          )}
        </div>
      )}

      {provider === "openrouter" && (
        <div className="space-y-2 px-4 py-3">
          <p className="text-xs text-text/60">
            {t("settings.models.cloud.description")}
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <ApiKeyField
              value={openrouterKey}
              onBlur={(value) => updatePostProcessApiKey("openrouter", value)}
              disabled={false}
              placeholder={t("settings.models.cloud.apiKeyPlaceholder")}
            />
            <button
              type="button"
              onClick={() => openUrl("https://openrouter.ai/keys")}
              className="text-xs text-logo-primary hover:underline cursor-pointer"
            >
              {t("settings.models.cloud.getKey")}
            </button>
          </div>
          {!openrouterKey.trim() && (
            <p className="text-xs text-amber-500">
              {t("settings.models.cloud.keyRequired")}
            </p>
          )}

          {isCustomCloud && (
            <div className="space-y-1 pt-1">
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
                  className="text-xs text-logo-primary hover:underline cursor-pointer"
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
        </div>
      )}
    </SettingsGroup>
  );
};
