import React, { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { errorMessage } from "@/lib/utils/errors";
import { SettingContainer } from "../ui/SettingContainer";
import { PathDisplay } from "../ui/PathDisplay";

interface AppDataDirectoryProps {
  descriptionMode?: "tooltip" | "inline";
  grouped?: boolean;
}

export const AppDataDirectory: React.FC<AppDataDirectoryProps> = ({
  descriptionMode = "inline",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const [appDirPath, setAppDirPath] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const loadAppDirectory = async () => {
      try {
        const result = await commands.getAppDirPath();
        if (result.status === "ok") {
          setAppDirPath(result.data);
        } else {
          setError(result.error);
        }
      } catch (err) {
        setError(errorMessage(err));
      } finally {
        setLoading(false);
      }
    };

    void loadAppDirectory();
  }, []);

  const handleOpen = async () => {
    if (!appDirPath) return;
    try {
      const result = await commands.openAppDataDir();
      if (result.status === "error") throw result.error;
    } catch (openError) {
      console.error("Failed to open app data directory:", openError);
      toast.error(t("errors.openFolderFailed"), {
        description: errorMessage(openError),
      });
    }
  };

  if (loading) {
    return (
      <div className="animate-pulse px-4 py-2">
        <div className="h-4 bg-mid-gray/20 rounded w-1/3 mb-2"></div>
        <div className="h-8 bg-mid-gray/10 rounded"></div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="px-4 py-2">
        <p role="alert" className="text-rec text-sm break-words">
          {t("errors.loadDirectory", { error })}
        </p>
      </div>
    );
  }

  return (
    <SettingContainer
      title={t("settings.about.appDataDirectory.title")}
      description={t("settings.about.appDataDirectory.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      <PathDisplay
        path={appDirPath}
        onOpen={() => void handleOpen()}
        disabled={!appDirPath}
      />
    </SettingContainer>
  );
};
