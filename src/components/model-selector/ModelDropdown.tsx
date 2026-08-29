import React from "react";
import { useTranslation } from "react-i18next";
import { Check, Cloud } from "lucide-react";
import type { ModelInfo } from "@/bindings";
import {
  getTranslatedModelName,
  getTranslatedModelDescription,
} from "../../lib/utils/modelTranslation";
import { isCloudModel } from "@/lib/utils/model";

interface ModelDropdownProps {
  models: ModelInfo[];
  currentModelId: string;
  onModelSelect: (modelId: string) => void;
}

const ModelDropdown: React.FC<ModelDropdownProps> = ({
  models,
  currentModelId,
  onModelSelect,
}) => {
  const { t } = useTranslation();
  const downloadedModels = models.filter((m) => m.is_downloaded);

  // Cloud models are always "downloaded", so an ungrouped list mixed six remote
  // entries in with the one or two the user actually installed.
  const sections = [
    {
      id: "local",
      label: t("settings.models.groups.installed"),
      items: downloadedModels.filter((m) => !isCloudModel(m)),
    },
    {
      id: "cloud",
      label: t("settings.models.groups.cloud"),
      items: downloadedModels.filter(isCloudModel),
    },
  ].filter((section) => section.items.length > 0);

  return (
    <div className="absolute bottom-full start-0 mb-2 w-72 max-h-[60vh] overflow-y-auto bg-background border border-mid-gray/20 rounded-lg shadow-lg py-1 z-50">
      {sections.length > 0 ? (
        sections.map((section) => (
          <div key={section.id}>
            <p className="px-3 pt-2 pb-1 text-[10px] font-medium uppercase tracking-wider text-mid-gray/70">
              {section.label}
            </p>
            {section.items.map((model) => {
              const isActive = currentModelId === model.id;
              return (
                <button
                  key={model.id}
                  type="button"
                  onClick={() => onModelSelect(model.id)}
                  className={`w-full px-3 py-1.5 text-start transition-colors cursor-pointer hover:bg-mid-gray/10 focus:outline-none ${
                    isActive ? "bg-logo-primary/10" : ""
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <span
                      className={`truncate text-sm ${
                        isActive ? "text-logo-primary" : "text-text/80"
                      }`}
                    >
                      {getTranslatedModelName(model, t)}
                    </span>
                    {isCloudModel(model) && (
                      <Cloud className="h-3 w-3 shrink-0 text-text/35" />
                    )}
                    {model.is_custom && (
                      <span className="text-[10px] font-medium uppercase text-text/40">
                        {t("modelSelector.custom")}
                      </span>
                    )}
                    {isActive && (
                      <Check className="ms-auto h-3.5 w-3.5 shrink-0 text-logo-primary" />
                    )}
                  </div>
                  <div className="truncate text-xs text-text/40">
                    {getTranslatedModelDescription(model, t)}
                  </div>
                </button>
              );
            })}
          </div>
        ))
      ) : (
        <div className="px-3 py-2 text-sm text-text/60">
          {t("modelSelector.noModelsAvailable")}
        </div>
      )}
    </div>
  );
};

export default ModelDropdown;
