import React, { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Search, X } from "lucide-react";
import type { ModelCardStatus } from "@/components/onboarding";
import { useModelStore } from "@/stores/modelStore";
import { CollapsibleGroup } from "@/components/ui/CollapsibleGroup";
import { useConfirm } from "@/components/ui/ConfirmDialog";
import { LANGUAGES } from "@/lib/constants/languages";
import type { ModelInfo } from "@/bindings";
import { getTranslatedModelName } from "@/lib/utils/modelTranslation";
import { ActiveModelPanel } from "./ActiveModelPanel";
import { CloudKeysPanel } from "./CloudKeysPanel";
import { MeetingModelPanel } from "./MeetingModelPanel";
import { ModelRow } from "./ModelRow";
import { isCloudModel } from "@/lib/utils/model";

type GroupId = "installed" | "cloud" | "multilingual" | "english" | "regional";

// Groups are ordered the way people choose a model: what I already have, then
// cloud vs. local, then by which languages the model covers.
const GROUP_ORDER: GroupId[] = [
  "installed",
  "cloud",
  "multilingual",
  "english",
  "regional",
];

const groupOf = (model: ModelInfo): GroupId => {
  if (isCloudModel(model)) return "cloud";
  if (model.is_downloaded || model.is_custom) return "installed";
  if (model.supported_languages.length >= 20) return "multilingual";
  if (
    model.supported_languages.length === 1 &&
    model.supported_languages[0] === "en"
  ) {
    return "english";
  }
  return "regional";
};

/**
 * Matches a model against the search box. The query is checked against the
 * model's name and description *and* against the languages it supports, so
 * typing a language name ("turkish") narrows the list to models that handle it
 * — one control instead of a separate language filter dropdown.
 */
const matchesQuery = (
  model: ModelInfo,
  query: string,
  name: string,
): boolean => {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  if (name.toLowerCase().includes(q)) return true;
  if (model.description.toLowerCase().includes(q)) return true;
  return LANGUAGES.some(
    (lang) =>
      lang.value !== "auto" &&
      lang.label.toLowerCase().includes(q) &&
      model.supported_languages.includes(lang.value),
  );
};

export const ModelsSettings: React.FC = () => {
  const { t } = useTranslation();
  const [switchingModelId, setSwitchingModelId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const { confirm, dialog } = useConfirm();

  const {
    models,
    currentModel,
    downloadingModels,
    downloadProgress,
    downloadStats,
    verifyingModels,
    extractingModels,
    loading,
    downloadModel,
    cancelDownload,
    selectModel,
    deleteModel,
  } = useModelStore();

  const currentModelInfo = models.find((m: ModelInfo) => m.id === currentModel);

  const getModelStatus = (modelId: string): ModelCardStatus => {
    if (modelId in extractingModels) return "extracting";
    if (modelId in verifyingModels) return "verifying";
    if (modelId in downloadingModels) return "downloading";
    if (switchingModelId === modelId) return "switching";
    if (modelId === currentModel) return "active";
    return models.find((m: ModelInfo) => m.id === modelId)?.is_downloaded
      ? "available"
      : "downloadable";
  };

  const handleModelSelect = async (modelId: string) => {
    setSwitchingModelId(modelId);
    try {
      await selectModel(modelId);
    } finally {
      setSwitchingModelId(null);
    }
  };

  const handleModelDelete = async (modelId: string) => {
    const model = models.find((m: ModelInfo) => m.id === modelId);
    const modelName = model ? getTranslatedModelName(model, t) : modelId;
    const confirmed = await confirm({
      title: t("settings.models.deleteTitle"),
      description:
        modelId === currentModel
          ? t("settings.models.deleteActiveConfirm", { modelName })
          : t("settings.models.deleteConfirm", { modelName }),
      destructive: true,
    });
    if (!confirmed) return;
    // Failures are reported by the model store.
    await deleteModel(modelId);
  };

  const handleModelCancel = (modelId: string) => {
    // Failures are reported by the model store.
    void cancelDownload(modelId);
  };

  const searching = query.trim().length > 0;

  const { grouped, matches } = useMemo(() => {
    const visible = models.filter((model: ModelInfo) =>
      matchesQuery(model, query, getTranslatedModelName(model, t)),
    );

    const buckets: Record<GroupId, ModelInfo[]> = {
      installed: [],
      cloud: [],
      multilingual: [],
      english: [],
      regional: [],
    };

    for (const model of visible) {
      const id =
        model.id in downloadingModels || model.id in extractingModels
          ? "installed"
          : groupOf(model);
      buckets[id].push(model);
    }

    // The active model heads its group; recommended models come next.
    const rank = (model: ModelInfo) =>
      model.id === currentModel ? 0 : model.is_recommended ? 1 : 2;
    for (const id of GROUP_ORDER) {
      buckets[id].sort(
        (a, b) =>
          rank(a) - rank(b) ||
          Number(a.is_custom) - Number(b.is_custom) ||
          b.accuracy_score - a.accuracy_score,
      );
    }

    return { grouped: buckets, matches: visible };
  }, [models, query, t, currentModel, downloadingModels, extractingModels]);

  // Only the first non-empty group starts expanded — normally "Installed", or
  // whatever comes first when the user has not downloaded anything yet.
  const visibleGroups = GROUP_ORDER.filter((id) => grouped[id].length > 0);

  const renderRow = (model: ModelInfo) => (
    <ModelRow
      key={model.id}
      model={model}
      status={getModelStatus(model.id)}
      onSelect={(id) => void handleModelSelect(id)}
      onDownload={(id) => void downloadModel(id)}
      onDelete={(id) => void handleModelDelete(id)}
      onCancel={handleModelCancel}
      downloadProgress={downloadProgress[model.id]?.percentage}
      downloadSpeed={downloadStats[model.id]?.speed}
    />
  );

  const listShell = (children: React.ReactNode) => (
    <div className="overflow-hidden rounded-lg border border-line bg-background">
      <div className="divide-y divide-line">{children}</div>
    </div>
  );

  if (loading) {
    return (
      <div className="mx-auto w-full max-w-3xl">
        <div className="flex items-center justify-center py-16">
          <div className="h-8 w-8 animate-spin rounded-full border-2 border-logo-primary border-t-transparent" />
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto w-full max-w-3xl space-y-6">
      {dialog}
      <ActiveModelPanel model={currentModelInfo} />
      <MeetingModelPanel models={models} dictationModel={currentModelInfo} />
      <CloudKeysPanel />

      <div className="space-y-3">
        <div className="flex items-center justify-between gap-3 px-1">
          <h2 className="text-xs font-medium uppercase tracking-wide text-mid-gray">
            {t("settings.models.browse")}
          </h2>
          <div className="relative w-64">
            <Search
              className="pointer-events-none absolute start-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-faint"
              aria-hidden
            />
            <input
              type="search"
              aria-label={t("settings.models.searchPlaceholder")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("settings.models.searchPlaceholder")}
              className="w-full rounded-lg border border-mid-gray/30 bg-background py-1.5 ps-8 pe-7 text-sm focus:border-logo-primary focus:outline-none [&::-webkit-search-cancel-button]:hidden"
            />
            {searching && (
              <button
                type="button"
                onClick={() => setQuery("")}
                aria-label={t("common.clear")}
                className="absolute end-2 top-1/2 -translate-y-1/2 text-faint hover:text-text cursor-pointer"
              >
                <X className="h-3.5 w-3.5" />
              </button>
            )}
          </div>
        </div>

        {matches.length === 0 && (
          <div className="py-8 text-center text-sm text-sub">
            {t("settings.models.noModelsMatch")}
          </div>
        )}

        {searching
          ? matches.length > 0 && listShell(matches.map(renderRow))
          : visibleGroups.map((id) => (
              <CollapsibleGroup
                key={id}
                title={t(`settings.models.groups.${id}`)}
                count={grouped[id].length}
                defaultOpen={id === visibleGroups[0]}
              >
                {listShell(grouped[id].map(renderRow))}
              </CollapsibleGroup>
            ))}
      </div>
    </div>
  );
};
