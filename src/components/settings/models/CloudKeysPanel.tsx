import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { KeyRound } from "lucide-react";
import { Input } from "@/components/ui/Input";
import { SettingContainer } from "@/components/ui/SettingContainer";
import { SettingsGroup } from "@/components/ui/SettingsGroup";
import { changeGeminiApiKey, getMeetingGeminiSettings } from "@/lib/meeting";
import { useSettingsStore } from "@/stores/settingsStore";

/**
 * Every cloud credential the app uses, in one always-visible place.
 *
 * Keys used to be attached to whichever model happened to be active, which
 * meant the Gemini field only existed while a Gemini model was the *dictation*
 * model — so anyone running local dictation with a Gemini meeting model had
 * nowhere to paste it. A key is account-level, not model-level, so it belongs
 * here rather than inside a model's panel.
 */
export const CloudKeysPanel: React.FC = () => {
  const { t } = useTranslation();
  const { settings, updatePostProcessApiKey } = useSettingsStore();

  const openrouterKey = settings?.post_process_api_keys?.["openrouter"] ?? "";

  // The Gemini key is write-only — the backend reports only whether one is
  // stored — so the field stays empty and shows its state in the placeholder.
  const [hasGeminiKey, setHasGeminiKey] = useState(false);
  const [geminiDraft, setGeminiDraft] = useState("");
  useEffect(() => {
    let cancelled = false;
    void getMeetingGeminiSettings().then((s) => {
      if (!cancelled) setHasGeminiKey(s.hasApiKey);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const commitGemini = async (value: string) => {
    const key = value.trim();
    if (key.length === 0) return;
    try {
      await changeGeminiApiKey(key);
      setHasGeminiKey(true);
      setGeminiDraft("");
    } catch (error) {
      console.error("Failed to save the Gemini API key:", error);
    }
  };

  const clearGemini = async () => {
    try {
      await changeGeminiApiKey("");
      setHasGeminiKey(false);
      setGeminiDraft("");
    } catch (error) {
      console.error("Failed to clear the Gemini API key:", error);
    }
  };

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

      <SettingContainer
        title={t("settings.models.keys.openrouter")}
        description={t("settings.models.keys.openrouterHint")}
        descriptionMode="tooltip"
        grouped
      >
        <div className="flex items-center gap-2">
          <Input
            type="password"
            value={openrouterKey}
            onChange={(event) =>
              updatePostProcessApiKey("openrouter", event.target.value)
            }
            placeholder={t("settings.models.cloud.apiKeyPlaceholder")}
            variant="compact"
            className="min-w-[280px]"
          />
          <button
            type="button"
            onClick={() => openUrl("https://openrouter.ai/keys")}
            className="cursor-pointer whitespace-nowrap text-xs text-logo-primary hover:underline"
          >
            {t("settings.models.cloud.getKey")}
          </button>
        </div>
      </SettingContainer>

      <SettingContainer
        title={t("settings.models.keys.gemini")}
        description={t("settings.models.keys.geminiHint")}
        descriptionMode="tooltip"
        grouped
      >
        <div className="flex items-center gap-2">
          <Input
            type="password"
            value={geminiDraft}
            onChange={(event) => setGeminiDraft(event.target.value)}
            onBlur={() => void commitGemini(geminiDraft)}
            placeholder={
              hasGeminiKey
                ? t("settings.models.gemini.apiKeyStored")
                : t("settings.models.gemini.apiKeyPlaceholder")
            }
            variant="compact"
            className="min-w-[280px]"
          />
          {hasGeminiKey ? (
            <button
              type="button"
              onClick={() => void clearGemini()}
              className="cursor-pointer whitespace-nowrap text-xs text-text/50 transition-colors hover:text-red-400"
            >
              {t("settings.models.keys.clear")}
            </button>
          ) : (
            <button
              type="button"
              onClick={() => openUrl("https://aistudio.google.com/apikey")}
              className="cursor-pointer whitespace-nowrap text-xs text-logo-primary hover:underline"
            >
              {t("settings.models.gemini.getKey")}
            </button>
          )}
        </div>
      </SettingContainer>
    </SettingsGroup>
  );
};
