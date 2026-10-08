import React, { useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { getTranslatedModelName } from "../../lib/utils/modelTranslation";
import { useModelStore } from "../../stores/modelStore";
import ModelStatusButton from "./ModelStatusButton";
import ModelDropdown from "./ModelDropdown";
import DownloadProgressDisplay from "./DownloadProgressDisplay";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useMeetingStore } from "@/stores/meetingStore";

import { ModelStateEvent } from "@/lib/types/events";

type ModelStatus =
  | "ready"
  | "loading"
  | "downloading"
  | "verifying"
  | "extracting"
  | "error"
  | "unloaded"
  | "none";

interface ModelSelectorProps {
  onError?: (error: string) => void;
}

const ModelSelector: React.FC<ModelSelectorProps> = ({ onError }) => {
  const { t } = useTranslation();
  const {
    models,
    currentModel,
    downloadProgress,
    downloadStats,
    verifyingModels,
    extractingModels,
    selectModel,
  } = useModelStore();

  const [modelStatus, setModelStatus] = useState<ModelStatus>("unloaded");
  const [modelError, setModelError] = useState<string | null>(null);
  const [showModelDropdown, setShowModelDropdown] = useState(false);
  // Track pending model switch for optimistic display
  const [pendingModelId, setPendingModelId] = useState<string | null>(null);

  const dropdownRef = useRef<HTMLDivElement>(null);

  const displayModelId = pendingModelId || currentModel;

  // Check model status when currentModel changes
  useEffect(() => {
    const checkStatus = async () => {
      if (currentModel) {
        try {
          const statusResult = await commands.getTranscriptionModelStatus();
          if (statusResult.status === "ok") {
            setModelStatus(
              statusResult.data === currentModel ? "ready" : "unloaded",
            );
          }
        } catch {
          setModelStatus("error");
          setModelError(t("modelSelector.errors.statusFailed"));
        }
      } else {
        setModelStatus("none");
      }
    };
    void checkStatus();
  }, [currentModel, t]);

  // Model loading lifecycle
  useTauriEvent<ModelStateEvent>("model-state-changed", (payload) => {
    const { event_type, error } = payload;
    switch (event_type) {
      case "loading_started":
        setModelStatus("loading");
        setModelError(null);
        break;
      case "loading_completed":
        setModelStatus("ready");
        setModelError(null);
        setPendingModelId(null);
        break;
      case "loading_failed":
        setModelStatus("error");
        setModelError(error || t("modelSelector.errors.loadFailed"));
        setPendingModelId(null);
        break;
      case "unloaded":
        setModelStatus("unloaded");
        setModelError(null);
        break;
    }
  });

  // A finished download becomes the dictation model only when there is none
  // yet. Downloading a model to try it, or for meetings, must not silently
  // swap the model you dictate with — and never mid-meeting, which owns the
  // engine. (First-run onboarding selects its own model; this component is
  // not mounted then.)
  useTauriEvent<string>("model-download-complete", (modelId) => {
    if (useModelStore.getState().currentModel) return;
    if (useMeetingStore.getState().status !== "idle") return;
    void (async () => {
      try {
        if (await commands.isRecording()) return;
        setPendingModelId(modelId);
        setModelError(null);
        setShowModelDropdown(false);
        const success = await selectModel(modelId);
        if (!success) setPendingModelId(null);
      } catch (error) {
        console.warn("Auto-select after download failed:", error);
      }
    })();
  });

  // Click outside to close dropdown
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(event.target as Node)
      ) {
        setShowModelDropdown(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  const handleModelSelect = async (modelId: string) => {
    setPendingModelId(modelId);
    setModelError(null);
    setShowModelDropdown(false);
    const success = await selectModel(modelId);
    if (!success) {
      const message = t("modelSelector.errors.switchFailed");
      setPendingModelId(null);
      setModelStatus("error");
      setModelError(message);
      onError?.(message);
    }
  };

  const getModelDisplayText = (): string => {
    const verifyingKeys = Object.keys(verifyingModels);
    if (verifyingKeys.length > 0) {
      if (verifyingKeys.length === 1) {
        const modelId = verifyingKeys[0];
        const model = models.find((m) => m.id === modelId);
        const modelName = model
          ? getTranslatedModelName(model, t)
          : t("modelSelector.verifyingGeneric").replace("...", "");
        return t("modelSelector.verifying", { modelName });
      } else {
        return t("modelSelector.verifyingGeneric");
      }
    }

    const extractingKeys = Object.keys(extractingModels);
    if (extractingKeys.length > 0) {
      if (extractingKeys.length === 1) {
        const modelId = extractingKeys[0];
        const model = models.find((m) => m.id === modelId);
        const modelName = model
          ? getTranslatedModelName(model, t)
          : t("modelSelector.extractingGeneric").replace("...", "");
        return t("modelSelector.extracting", { modelName });
      } else {
        return t("modelSelector.extractingMultiple", {
          count: extractingKeys.length,
        });
      }
    }

    const progressValues = Object.values(downloadProgress);
    if (progressValues.length > 0) {
      if (progressValues.length === 1) {
        const progress = progressValues[0];
        const percentage = Math.max(
          0,
          Math.min(100, Math.round(progress.percentage)),
        );
        return t("modelSelector.downloading", { percentage });
      } else {
        return t("modelSelector.downloadingMultiple", {
          count: progressValues.length,
        });
      }
    }

    const currentModelInfo = models.find((m) => m.id === displayModelId);

    switch (modelStatus) {
      case "ready":
        return currentModelInfo
          ? getTranslatedModelName(currentModelInfo, t)
          : t("modelSelector.modelReady");
      case "loading":
        return currentModelInfo
          ? t("modelSelector.loading", {
              modelName: getTranslatedModelName(currentModelInfo, t),
            })
          : t("modelSelector.loadingGeneric");
      case "extracting":
        return currentModelInfo
          ? t("modelSelector.extracting", {
              modelName: getTranslatedModelName(currentModelInfo, t),
            })
          : t("modelSelector.extractingGeneric");
      case "error":
        return modelError || t("modelSelector.modelError");
      case "unloaded":
        return currentModelInfo
          ? getTranslatedModelName(currentModelInfo, t)
          : t("modelSelector.modelUnloaded");
      case "none":
        return t("modelSelector.noModelDownloadRequired");
      default:
        return currentModelInfo
          ? getTranslatedModelName(currentModelInfo, t)
          : t("modelSelector.modelUnloaded");
    }
  };

  // Derive display status from model status + store state
  const getDisplayStatus = (): ModelStatus => {
    if (Object.keys(verifyingModels).length > 0) return "verifying";
    if (Object.keys(extractingModels).length > 0) return "extracting";
    if (Object.keys(downloadProgress).length > 0) return "downloading";
    return modelStatus;
  };

  return (
    <>
      {/* Model Status and Switcher */}
      <div className="relative" ref={dropdownRef}>
        <ModelStatusButton
          status={getDisplayStatus()}
          displayText={getModelDisplayText()}
          isDropdownOpen={showModelDropdown}
          onClick={() => setShowModelDropdown(!showModelDropdown)}
        />

        {/* Model Dropdown */}
        {showModelDropdown && (
          <ModelDropdown
            models={models}
            currentModelId={displayModelId}
            onModelSelect={(id) => void handleModelSelect(id)}
          />
        )}
      </div>

      {/* Download Progress Bar for Models */}
      <DownloadProgressDisplay
        downloadProgress={downloadProgress}
        downloadStats={downloadStats}
      />
    </>
  );
};

export default ModelSelector;
