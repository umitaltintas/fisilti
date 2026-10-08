import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { ModelInfo } from "@/bindings";
import type { ModelCardStatus } from "./ModelCard";
import ModelCard from "./ModelCard";
import FisiltiWordmark from "../icons/FisiltiWordmark";
import { useModelStore } from "../../stores/modelStore";
import { isCloudModel } from "@/lib/utils/model";

interface OnboardingProps {
  onModelSelected: () => void;
}

const Onboarding: React.FC<OnboardingProps> = ({ onModelSelected }) => {
  const { t } = useTranslation();
  const {
    models,
    downloadModel,
    selectModel,
    downloadingModels,
    verifyingModels,
    extractingModels,
    downloadProgress,
    downloadStats,
  } = useModelStore();
  const [selectedModelId, setSelectedModelId] = useState<string | null>(null);

  const isDownloading = selectedModelId !== null;

  // Watch for the selected model to finish downloading + verifying + extracting
  useEffect(() => {
    if (!selectedModelId) return;

    const model = models.find((m) => m.id === selectedModelId);
    const stillDownloading = selectedModelId in downloadingModels;
    const stillVerifying = selectedModelId in verifyingModels;
    const stillExtracting = selectedModelId in extractingModels;

    if (
      model?.is_downloaded &&
      !stillDownloading &&
      !stillVerifying &&
      !stillExtracting
    ) {
      // Model is ready — select it and transition. On failure the model
      // stays in the list (now as "downloaded"), so it can be picked again
      // without another download.
      const modelId = selectedModelId;
      void selectModel(modelId, { silent: true }).then((success) => {
        if (success) {
          onModelSelected();
        } else {
          toast.error(t("onboarding.errors.selectModel"));
          setSelectedModelId((current) =>
            current === modelId ? null : current,
          );
        }
      });
    }
  }, [
    selectedModelId,
    models,
    downloadingModels,
    verifyingModels,
    extractingModels,
    selectModel,
    onModelSelected,
    t,
  ]);

  // An already-downloaded model (e.g. one whose selection failed before) is
  // selected directly by the effect above; no second download.
  const handleSelectDownloaded = (modelId: string) => {
    setSelectedModelId(modelId);
  };

  const handleDownloadModel = async (modelId: string) => {
    setSelectedModelId(modelId);

    // Error toast is handled centrally by the model-download-failed event listener
    // in modelStore — no toast here to avoid duplicates.
    const success = await downloadModel(modelId);
    if (!success) {
      setSelectedModelId(null);
    }
  };

  const getModelStatus = (model: ModelInfo): ModelCardStatus => {
    if (model.id in extractingModels) return "extracting";
    if (model.id in verifyingModels) return "verifying";
    if (model.id in downloadingModels) return "downloading";
    if (model.is_downloaded) {
      return selectedModelId === model.id ? "switching" : "available";
    }
    return "downloadable";
  };

  // Local models only: a cloud model needs an API key first, which is set up
  // on the Models page once the app is running.
  const localModels = models.filter((m: ModelInfo) => !isCloudModel(m));

  const getModelDownloadProgress = (modelId: string): number | undefined => {
    return downloadProgress[modelId]?.percentage;
  };

  const getModelDownloadSpeed = (modelId: string): number | undefined => {
    return downloadStats[modelId]?.speed;
  };

  return (
    <div className="h-screen w-screen flex flex-col p-6 gap-4 inset-0">
      <div className="flex flex-col items-center gap-2 shrink-0">
        <FisiltiWordmark width={200} />
        <p className="text-text/70 max-w-md font-medium mx-auto">
          {t("onboarding.subtitle")}
        </p>
      </div>

      <div className="max-w-[600px] w-full mx-auto text-center flex-1 flex flex-col min-h-0">
        <div className="flex flex-col gap-4 pb-6">
          {localModels
            .filter((model: ModelInfo) => model.is_recommended)
            .map((model: ModelInfo) => (
              <ModelCard
                key={model.id}
                model={model}
                variant="featured"
                status={getModelStatus(model)}
                disabled={isDownloading}
                onSelect={handleSelectDownloaded}
                onDownload={handleDownloadModel}
                downloadProgress={getModelDownloadProgress(model.id)}
                downloadSpeed={getModelDownloadSpeed(model.id)}
              />
            ))}

          {localModels
            .filter((model: ModelInfo) => !model.is_recommended)
            .sort(
              (a: ModelInfo, b: ModelInfo) =>
                Number(a.size_mb) - Number(b.size_mb),
            )
            .map((model: ModelInfo) => (
              <ModelCard
                key={model.id}
                model={model}
                status={getModelStatus(model)}
                disabled={isDownloading}
                onSelect={handleSelectDownloaded}
                onDownload={handleDownloadModel}
                downloadProgress={getModelDownloadProgress(model.id)}
                downloadSpeed={getModelDownloadSpeed(model.id)}
              />
            ))}
        </div>
      </div>
    </div>
  );
};

export default Onboarding;
