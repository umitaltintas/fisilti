import React, { useEffect, useId, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { RefreshCcw } from "lucide-react";
import { commands } from "@/bindings";
import { emit } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { errorMessage } from "@/lib/utils/errors";

import { Alert } from "../../ui/Alert";
import {
  Dropdown,
  SettingContainer,
  SettingsGroup,
  Textarea,
} from "@/components/ui";
import { Button } from "../../ui/Button";
import { ResetButton } from "../../ui/ResetButton";
import { Input } from "../../ui/Input";
import { useConfirm } from "../../ui/ConfirmDialog";

import { ProviderSelect } from "../PostProcessingSettingsApi/ProviderSelect";
import { BaseUrlField } from "../PostProcessingSettingsApi/BaseUrlField";
import { ApiKeyField } from "../PostProcessingSettingsApi/ApiKeyField";
import { ModelSelect } from "../PostProcessingSettingsApi/ModelSelect";
import { usePostProcessProviderState } from "../PostProcessingSettingsApi/usePostProcessProviderState";
import { ShortcutInput } from "../ShortcutInput";
import { PostProcessingToggle } from "../PostProcessingToggle";
import { useSettings } from "../../../hooks/useSettings";

const PostProcessingSettingsApiComponent: React.FC = () => {
  const { t } = useTranslation();
  const state = usePostProcessProviderState();

  return (
    <>
      <SettingContainer
        title={t("settings.postProcessing.api.provider.title")}
        description={t("settings.postProcessing.api.provider.description")}
        descriptionMode="inline"
        layout="horizontal"
        grouped={true}
      >
        <div className="flex items-center gap-2">
          <ProviderSelect
            options={state.providerOptions}
            value={state.selectedProviderId}
            onChange={state.handleProviderSelect}
          />
        </div>
      </SettingContainer>

      {state.isAppleProvider ? (
        state.appleIntelligenceUnavailable ? (
          <Alert variant="error" contained>
            {t("settings.postProcessing.api.appleIntelligence.unavailable")}
          </Alert>
        ) : null
      ) : (
        <>
          {state.selectedProvider?.id === "custom" && (
            <SettingContainer
              title={t("settings.postProcessing.api.baseUrl.title")}
              description={t("settings.postProcessing.api.baseUrl.description")}
              descriptionMode="inline"
              layout="horizontal"
              grouped={true}
            >
              <div className="flex items-center gap-2">
                <BaseUrlField
                  value={state.baseUrl}
                  onBlur={state.handleBaseUrlChange}
                  placeholder={t(
                    "settings.postProcessing.api.baseUrl.placeholder",
                  )}
                  disabled={state.isBaseUrlUpdating}
                  className="min-w-[380px]"
                />
              </div>
            </SettingContainer>
          )}

          <SettingContainer
            title={t("settings.postProcessing.api.apiKey.title")}
            description={t("settings.postProcessing.api.apiKey.description")}
            descriptionMode="inline"
            layout="horizontal"
            grouped={true}
          >
            {state.keyHome ? (
              /* This provider bills the same account as a transcription
                 engine, so its key already has a home on the Models page.
                 A second field here would be the same credential stored
                 twice. */
              <div className="flex items-center gap-3">
                <span
                  className={
                    state.hasKeyFromModels
                      ? "text-xs text-sub"
                      : "text-xs text-amber-500"
                  }
                >
                  {state.hasKeyFromModels
                    ? t("settings.postProcessing.api.apiKey.managedOnModels")
                    : t("settings.postProcessing.api.apiKey.missingOnModels")}
                </span>
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => void emit("navigate-section", "models")}
                >
                  {t("settings.postProcessing.api.apiKey.openModels")}
                </Button>
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <ApiKeyField
                  value={state.apiKey}
                  onCommit={state.handleApiKeyChange}
                  placeholder={t(
                    "settings.postProcessing.api.apiKey.placeholder",
                  )}
                  disabled={state.isApiKeyUpdating}
                  className="min-w-[320px]"
                />
              </div>
            )}
          </SettingContainer>
        </>
      )}

      {!state.isAppleProvider && (
        <SettingContainer
          title={t("settings.postProcessing.api.model.title")}
          description={
            state.isCustomProvider
              ? t("settings.postProcessing.api.model.descriptionCustom")
              : t("settings.postProcessing.api.model.descriptionDefault")
          }
          descriptionMode="inline"
          layout="stacked"
          grouped={true}
        >
          <div className="flex items-center gap-2">
            <ModelSelect
              value={state.model}
              options={state.modelOptions}
              disabled={state.isModelUpdating}
              isLoading={state.isFetchingModels}
              placeholder={
                state.modelOptions.length > 0
                  ? t(
                      "settings.postProcessing.api.model.placeholderWithOptions",
                    )
                  : t("settings.postProcessing.api.model.placeholderNoOptions")
              }
              onSelect={state.handleModelSelect}
              onCreate={state.handleModelCreate}
              onBlur={() => {}}
              className="flex-1 min-w-[380px]"
            />
            <ResetButton
              onClick={state.handleRefreshModels}
              disabled={state.isFetchingModels}
              ariaLabel={t("settings.postProcessing.api.model.refreshModels")}
              className="flex h-10 w-10 items-center justify-center"
            >
              <RefreshCcw
                className={`h-4 w-4 ${state.isFetchingModels ? "animate-spin" : ""}`}
              />
            </ResetButton>
          </div>
        </SettingContainer>
      )}
    </>
  );
};

interface PromptFormProps {
  name: string;
  text: string;
  onNameChange: (value: string) => void;
  onTextChange: (value: string) => void;
  /** The buttons under the form (create/cancel or update/delete). */
  children: React.ReactNode;
}

/** The one prompt editor, used for both creating and editing a prompt. */
const PromptForm: React.FC<PromptFormProps> = ({
  name,
  text,
  onNameChange,
  onTextChange,
  children,
}) => {
  const { t } = useTranslation();
  const nameId = useId();
  const textId = useId();
  return (
    <div className="space-y-3">
      <div className="space-y-2 flex flex-col">
        <label htmlFor={nameId} className="text-sm font-semibold">
          {t("settings.postProcessing.prompts.promptLabel")}
        </label>
        <Input
          id={nameId}
          type="text"
          value={name}
          onChange={(e) => onNameChange(e.target.value)}
          placeholder={t(
            "settings.postProcessing.prompts.promptLabelPlaceholder",
          )}
          variant="compact"
        />
      </div>

      <div className="space-y-2 flex flex-col">
        <label htmlFor={textId} className="text-sm font-semibold">
          {t("settings.postProcessing.prompts.promptInstructions")}
        </label>
        <Textarea
          id={textId}
          value={text}
          onChange={(e) => onTextChange(e.target.value)}
          placeholder={t(
            "settings.postProcessing.prompts.promptInstructionsPlaceholder",
          )}
        />
        <p className="text-xs text-mid-gray/70">
          <Trans
            i18nKey="settings.postProcessing.prompts.promptTip"
            components={{ code: <code /> }}
          />
        </p>
      </div>

      <div className="flex gap-2 pt-2">{children}</div>
    </div>
  );
};

const PostProcessingSettingsPromptsComponent: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating, refreshSettings } =
    useSettings();
  const { confirm, dialog } = useConfirm();
  const [isCreating, setIsCreating] = useState(false);
  const [draftName, setDraftName] = useState("");
  const [draftText, setDraftText] = useState("");
  const [saving, setSaving] = useState(false);

  const prompts = getSetting("post_process_prompts") || [];
  const selectedPromptId = getSetting("post_process_selected_prompt_id") || "";
  const selectedPrompt =
    prompts.find((prompt) => prompt.id === selectedPromptId) || null;

  useEffect(() => {
    if (isCreating) return;

    if (selectedPrompt) {
      setDraftName(selectedPrompt.name);
      setDraftText(selectedPrompt.prompt);
    } else {
      setDraftName("");
      setDraftText("");
    }
    // Keyed on the prompt's content, not the object identity, so an
    // unrelated settings refresh does not wipe what is being typed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    isCreating,
    selectedPromptId,
    selectedPrompt?.name,
    selectedPrompt?.prompt,
  ]);

  const hasPrompts = prompts.length > 0;
  const isDirty = isCreating
    ? draftName.trim() !== "" || draftText.trim() !== ""
    : !!selectedPrompt &&
      (draftName.trim() !== selectedPrompt.name ||
        draftText.trim() !== selectedPrompt.prompt.trim());

  /** Unsaved edits are lost when switching away; ask first. */
  const confirmDiscardEdits = async () =>
    !isDirty ||
    confirm({
      title: t("settings.postProcessing.prompts.unsavedTitle"),
      description: t("settings.postProcessing.prompts.unsavedText"),
      confirmLabel: t("settings.postProcessing.prompts.discardChanges"),
      destructive: true,
    });

  const reportError = (key: string, error: unknown) => {
    console.error(key, error);
    toast.error(t(key), { description: errorMessage(error) });
  };

  const handlePromptSelect = async (promptId: string | null) => {
    if (!promptId || promptId === selectedPromptId) return;
    if (!(await confirmDiscardEdits())) return;
    setIsCreating(false);
    await updateSetting("post_process_selected_prompt_id", promptId);
  };

  const handleCreatePrompt = async () => {
    if (!draftName.trim() || !draftText.trim()) return;
    setSaving(true);
    try {
      const result = await commands.addPostProcessPrompt(
        draftName.trim(),
        draftText.trim(),
      );
      if (result.status === "error") throw result.error;
      await refreshSettings();
      await updateSetting("post_process_selected_prompt_id", result.data.id);
      setIsCreating(false);
    } catch (error) {
      reportError("settings.postProcessing.prompts.errors.create", error);
    } finally {
      setSaving(false);
    }
  };

  const handleUpdatePrompt = async () => {
    if (!selectedPromptId || !draftName.trim() || !draftText.trim()) return;
    setSaving(true);
    try {
      const result = await commands.updatePostProcessPrompt(
        selectedPromptId,
        draftName.trim(),
        draftText.trim(),
      );
      if (result.status === "error") throw result.error;
      await refreshSettings();
      toast.success(t("settings.postProcessing.prompts.saved"));
    } catch (error) {
      reportError("settings.postProcessing.prompts.errors.update", error);
    } finally {
      setSaving(false);
    }
  };

  const handleDeletePrompt = async () => {
    if (!selectedPrompt) return;
    const ok = await confirm({
      title: t("settings.postProcessing.prompts.deleteConfirmTitle"),
      description: t("settings.postProcessing.prompts.deleteConfirmText", {
        name: selectedPrompt.name,
      }),
      destructive: true,
    });
    if (!ok) return;
    setSaving(true);
    try {
      const result = await commands.deletePostProcessPrompt(selectedPrompt.id);
      if (result.status === "error") throw result.error;
      await refreshSettings();
      setIsCreating(false);
    } catch (error) {
      reportError("settings.postProcessing.prompts.errors.delete", error);
    } finally {
      setSaving(false);
    }
  };

  const handleCancelCreate = () => {
    setIsCreating(false);
    setDraftName(selectedPrompt?.name ?? "");
    setDraftText(selectedPrompt?.prompt ?? "");
  };

  const handleStartCreate = async () => {
    if (!(await confirmDiscardEdits())) return;
    setIsCreating(true);
    setDraftName("");
    setDraftText("");
  };

  const draftIncomplete = !draftName.trim() || !draftText.trim();

  return (
    <SettingContainer
      title={t("settings.postProcessing.prompts.selectedPrompt.title")}
      description={t(
        "settings.postProcessing.prompts.selectedPrompt.description",
      )}
      descriptionMode="inline"
      layout="stacked"
      grouped={true}
    >
      {dialog}
      <div className="space-y-3">
        <div className="flex gap-2">
          <Dropdown
            selectedValue={selectedPromptId || null}
            options={prompts.map((p) => ({
              value: p.id,
              label: p.name,
            }))}
            onSelect={(value) => void handlePromptSelect(value)}
            placeholder={
              prompts.length === 0
                ? t("settings.postProcessing.prompts.noPrompts")
                : t("settings.postProcessing.prompts.selectPrompt")
            }
            disabled={
              isUpdating("post_process_selected_prompt_id") || isCreating
            }
            className="flex-1"
          />
          <Button
            onClick={() => void handleStartCreate()}
            variant="primary"
            size="md"
            disabled={isCreating}
          >
            {t("settings.postProcessing.prompts.createNew")}
          </Button>
        </div>

        {!isCreating && hasPrompts && selectedPrompt && (
          <PromptForm
            name={draftName}
            text={draftText}
            onNameChange={setDraftName}
            onTextChange={setDraftText}
          >
            <Button
              onClick={() => void handleUpdatePrompt()}
              variant="primary"
              size="md"
              disabled={draftIncomplete || !isDirty || saving}
            >
              {t("settings.postProcessing.prompts.updatePrompt")}
            </Button>
            <Button
              onClick={() => void handleDeletePrompt()}
              variant="danger-ghost"
              size="md"
              disabled={!selectedPromptId || prompts.length <= 1 || saving}
            >
              {t("settings.postProcessing.prompts.deletePrompt")}
            </Button>
          </PromptForm>
        )}

        {!isCreating && !selectedPrompt && (
          <div className="p-3 bg-mid-gray/5 rounded-md border border-line">
            <p className="text-sm text-mid-gray">
              {hasPrompts
                ? t("settings.postProcessing.prompts.selectToEdit")
                : t("settings.postProcessing.prompts.createFirst")}
            </p>
          </div>
        )}

        {isCreating && (
          <PromptForm
            name={draftName}
            text={draftText}
            onNameChange={setDraftName}
            onTextChange={setDraftText}
          >
            <Button
              onClick={() => void handleCreatePrompt()}
              variant="primary"
              size="md"
              disabled={draftIncomplete || saving}
            >
              {t("settings.postProcessing.prompts.createPrompt")}
            </Button>
            <Button onClick={handleCancelCreate} variant="secondary" size="md">
              {t("settings.postProcessing.prompts.cancel")}
            </Button>
          </PromptForm>
        )}
      </div>
    </SettingContainer>
  );
};

export const PostProcessingSettingsApi = React.memo(
  PostProcessingSettingsApiComponent,
);
PostProcessingSettingsApi.displayName = "PostProcessingSettingsApi";

export const PostProcessingSettingsPrompts = React.memo(
  PostProcessingSettingsPromptsComponent,
);
PostProcessingSettingsPrompts.displayName = "PostProcessingSettingsPrompts";

export const PostProcessingSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  // The on/off switch used to live under Advanced -> Experimental while the
  // configuration lived here, so the whole section silently vanished from the
  // sidebar. The switch now sits on top of the thing it turns on.
  const enabled = getSetting("post_process_enabled") || false;

  return (
    <div className="mx-auto w-full max-w-3xl space-y-6">
      <SettingsGroup
        title={t("settings.postProcessing.title")}
        description={t("settings.postProcessing.description")}
      >
        <PostProcessingToggle descriptionMode="inline" grouped={true} />
      </SettingsGroup>

      {enabled && (
        <>
          <SettingsGroup title={t("settings.postProcessing.hotkey.title")}>
            <ShortcutInput
              shortcutId="transcribe_with_post_process"
              descriptionMode="inline"
              grouped={true}
            />
          </SettingsGroup>

          <SettingsGroup title={t("settings.postProcessing.api.title")}>
            <PostProcessingSettingsApi />
          </SettingsGroup>

          <SettingsGroup title={t("settings.postProcessing.prompts.title")}>
            <PostProcessingSettingsPrompts />
          </SettingsGroup>
        </>
      )}
    </div>
  );
};
