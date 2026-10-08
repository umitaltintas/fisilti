import React, { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Radio } from "lucide-react";
import type { ModelInfo } from "@/bindings";
import { Dropdown } from "@/components/ui/Dropdown";
import { SettingContainer } from "@/components/ui/SettingContainer";
import { SettingsGroup } from "@/components/ui/SettingsGroup";
import { isCloudModel } from "@/lib/utils/model";
import { getTranslatedModelName } from "@/lib/utils/modelTranslation";
import { useSettingsStore } from "@/stores/settingsStore";

interface MeetingModelPanelProps {
  models: ModelInfo[];
  /** The dictation model, shown so "same as dictation" names something real. */
  dictationModel: ModelInfo | undefined;
}

/**
 * Which model transcribes meetings.
 *
 * Meetings and dictation want different trade-offs often enough to deserve
 * their own selection: a long meeting is worth an accurate cloud model, while
 * push-to-talk dictation usually wants something local and instant. The default
 * is still "same as dictation", so nothing changes until the user asks for it.
 *
 * This lives on the Models page rather than in the meeting settings because
 * model selection has exactly one home.
 */
export const MeetingModelPanel: React.FC<MeetingModelPanelProps> = ({
  models,
  dictationModel,
}) => {
  const { t } = useTranslation();
  const selected = useSettingsStore(
    (s) => s.settings?.meeting_selected_model ?? "",
  );
  const updateSetting = useSettingsStore((s) => s.updateSetting);
  const refreshSettings = useSettingsStore((s) => s.refreshSettings);
  const updating = useSettingsStore(
    (s) => s.isUpdating["meeting_selected_model"] === true,
  );

  const options = useMemo(() => {
    const followLabel = dictationModel
      ? t("settings.models.meetingModel.sameAsDictationNamed", {
          model: getTranslatedModelName(dictationModel, t),
        })
      : t("settings.models.meetingModel.sameAsDictation");

    // Same rule as the tray: only models you could actually run right now.
    const selectable = models
      .filter((model) => model.is_downloaded)
      .sort((a, b) => a.name.localeCompare(b.name));

    return [
      { value: "", label: followLabel },
      ...selectable.map((model) => ({
        value: model.id,
        label: isCloudModel(model)
          ? `${getTranslatedModelName(model, t)} · ${t("settings.models.badges.cloud")}`
          : getTranslatedModelName(model, t),
      })),
    ];
  }, [models, dictationModel, t]);

  const handleSelect = async (modelId: string) => {
    // A rejected id (e.g. a model deleted meanwhile) is rolled back and
    // reported by the store; on success, pull the persisted value back in
    // case the backend normalized it.
    if (await updateSetting("meeting_selected_model", modelId)) {
      await refreshSettings();
    }
  };

  return (
    <SettingsGroup title={t("settings.models.meetingModel.title")}>
      <div className="flex items-center gap-3 px-4 py-3">
        <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-logo-primary/15 text-logo-primary">
          <Radio className="h-4 w-4" />
        </div>
        <p className="text-xs text-text/60">
          {t("settings.models.meetingModel.description")}
        </p>
      </div>
      <SettingContainer
        title={t("settings.models.meetingModel.label")}
        description={t("settings.models.meetingModel.hint")}
        descriptionMode="tooltip"
        grouped
      >
        <Dropdown
          options={options}
          selectedValue={selected}
          disabled={updating}
          onSelect={(value) => void handleSelect(value)}
        />
      </SettingContainer>
    </SettingsGroup>
  );
};
