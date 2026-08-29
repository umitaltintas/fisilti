import React from "react";
import { useTranslation } from "react-i18next";
import { Check, Cloud, Download, Loader2, Trash2, X } from "lucide-react";
import type { ModelInfo } from "@/bindings";
import type { ModelCardStatus } from "@/components/onboarding";
import { formatModelSize } from "@/lib/utils/format";
import {
  getTranslatedModelDescription,
  getTranslatedModelName,
} from "@/lib/utils/modelTranslation";
import { LANGUAGES } from "@/lib/constants/languages";
import { isCloudModel } from "@/lib/utils/model";

/** Short, human summary of what languages a model covers. */
const languageSummary = (
  model: ModelInfo,
  t: (key: string, options?: Record<string, unknown>) => string,
): string => {
  const langs = model.supported_languages;
  if (langs.length === 0) return "";
  if (langs.length === 1) {
    const label =
      LANGUAGES.find((l) => l.value === langs[0])?.label ?? langs[0];
    return label;
  }
  if (langs.length > 40) return t("settings.models.groups.multilingualShort");
  return t("settings.models.languageCount", { count: langs.length });
};

/** A five-segment meter, small enough to sit inline in a list row. */
const Meter: React.FC<{ label: string; value: number }> = ({
  label,
  value,
}) => (
  <div
    className="flex items-center gap-1"
    title={`${label}: ${Math.round(value * 100)}%`}
  >
    <span className="text-[10px] uppercase tracking-wide text-text/35">
      {label}
    </span>
    <div className="flex gap-0.5">
      {[0, 1, 2, 3, 4].map((i) => (
        <span
          key={i}
          className={`h-2.5 w-1 rounded-[1px] ${
            value * 5 > i ? "bg-logo-primary/70" : "bg-mid-gray/25"
          }`}
        />
      ))}
    </div>
  </div>
);

interface ModelRowProps {
  model: ModelInfo;
  status: ModelCardStatus;
  onSelect: (modelId: string) => void;
  onDownload: (modelId: string) => void;
  onDelete: (modelId: string) => void;
  onCancel: (modelId: string) => void;
  downloadProgress?: number;
  downloadSpeed?: number;
}

/**
 * One model, rendered as a compact list row. The big bordered cards are kept
 * for onboarding (where there are only three of them); the settings list has
 * ~20 entries, so it needs rows that scan quickly.
 */
export const ModelRow: React.FC<ModelRowProps> = ({
  model,
  status,
  onSelect,
  onDownload,
  onDelete,
  onCancel,
  downloadProgress,
  downloadSpeed,
}) => {
  const { t } = useTranslation();
  const isCloud = isCloudModel(model);
  const isBusy =
    status === "downloading" ||
    status === "verifying" ||
    status === "extracting" ||
    status === "switching";
  const isActive = status === "active";
  const name = getTranslatedModelName(model, t);

  const handleActivate = () => {
    if (isBusy || isActive) return;
    if (status === "downloadable") onDownload(model.id);
    else onSelect(model.id);
  };

  return (
    <div
      role="button"
      tabIndex={isBusy || isActive ? -1 : 0}
      onClick={handleActivate}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          handleActivate();
        }
      }}
      className={`flex items-center gap-3 px-4 py-2.5 transition-colors ${
        isActive
          ? "bg-logo-primary/10"
          : isBusy
            ? ""
            : "cursor-pointer hover:bg-mid-gray/10"
      }`}
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-medium">{name}</span>
          {isActive && (
            <span className="inline-flex items-center gap-1 rounded-full bg-logo-primary/20 px-2 py-0.5 text-[10px] font-medium text-logo-primary">
              <Check className="h-3 w-3" />
              {t("modelSelector.active")}
            </span>
          )}
          {isCloud && (
            <Cloud className="h-3.5 w-3.5 shrink-0 text-text/35" aria-hidden />
          )}
          {model.is_custom && (
            <span className="rounded-full bg-mid-gray/20 px-2 py-0.5 text-[10px] text-text/60">
              {t("modelSelector.custom")}
            </span>
          )}
        </div>
        <p className="truncate text-xs text-text/50">
          {getTranslatedModelDescription(model, t)}
        </p>
        {status === "downloading" && downloadProgress !== undefined && (
          <div className="mt-1.5 flex items-center gap-2">
            <div className="h-1 flex-1 overflow-hidden rounded-full bg-mid-gray/20">
              <div
                className="h-full rounded-full bg-logo-primary transition-all"
                style={{ width: `${downloadProgress}%` }}
              />
            </div>
            <span className="tabular-nums text-[10px] text-text/50">
              {Math.round(downloadProgress)}%
              {downloadSpeed !== undefined && downloadSpeed > 0
                ? ` · ${downloadSpeed.toFixed(1)} MB/s`
                : ""}
            </span>
          </div>
        )}
      </div>

      <div className="hidden shrink-0 items-center gap-3 md:flex">
        {model.supported_languages.length > 0 && (
          <span className="text-xs text-text/40">
            {languageSummary(model, t)}
          </span>
        )}
        {model.accuracy_score > 0 && (
          <Meter
            label={t("onboarding.modelCard.accuracy")}
            value={model.accuracy_score}
          />
        )}
      </div>

      <div className="flex w-24 shrink-0 items-center justify-end gap-1">
        {status === "downloadable" && (
          <span className="flex items-center gap-1 text-xs text-text/50">
            <Download className="h-3.5 w-3.5" />
            {formatModelSize(Number(model.size_mb))}
          </span>
        )}
        {status === "switching" && (
          <Loader2 className="h-4 w-4 animate-spin text-text/40" />
        )}
        {(status === "verifying" || status === "extracting") && (
          <span className="text-xs text-text/50">
            {status === "verifying"
              ? t("modelSelector.verifyingGeneric")
              : t("modelSelector.extractingGeneric")}
          </span>
        )}
        {status === "downloading" && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onCancel(model.id);
            }}
            aria-label={t("modelSelector.cancelDownload")}
            className="rounded-md p-1.5 text-text/50 transition-colors hover:bg-mid-gray/20 hover:text-text cursor-pointer"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        )}
        {!isCloud && !model.is_custom && model.is_downloaded && !isBusy && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onDelete(model.id);
            }}
            title={t("modelSelector.deleteModel", { modelName: name })}
            className="rounded-md p-1.5 text-text/40 transition-colors hover:bg-mid-gray/20 hover:text-text cursor-pointer"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </button>
        )}
      </div>
    </div>
  );
};
