import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { errorMessage } from "@/lib/utils/errors";
import { SettingContainer } from "../../ui/SettingContainer";
import { PathDisplay } from "../../ui/PathDisplay";

interface LogDirectoryProps {
  descriptionMode?: "tooltip" | "inline";
  grouped?: boolean;
}

export const LogDirectory: React.FC<LogDirectoryProps> = ({
  descriptionMode = "inline",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const [logDir, setLogDir] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const loadLogDirectory = async () => {
      try {
        const result = await commands.getLogDirPath();
        if (result.status === "ok") {
          setLogDir(result.data);
        } else {
          setError(result.error);
        }
      } catch (err) {
        setError(errorMessage(err));
      } finally {
        setLoading(false);
      }
    };

    void loadLogDirectory();
  }, []);

  const handleOpen = async () => {
    if (!logDir) return;
    try {
      const result = await commands.openLogDir();
      if (result.status === "error") throw result.error;
    } catch (openError) {
      console.error("Failed to open log directory:", openError);
      toast.error(t("errors.openFolderFailed"), {
        description: errorMessage(openError),
      });
    }
  };

  return (
    <SettingContainer
      title={t("settings.debug.logDirectory.title")}
      description={t("settings.debug.logDirectory.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      {loading ? (
        <div className="animate-pulse">
          <div className="h-8 bg-mid-gray/10 rounded" />
        </div>
      ) : error ? (
        <div role="alert" className="text-xs text-rec break-words">
          {t("errors.loadDirectory", { error })}
        </div>
      ) : (
        <PathDisplay
          path={logDir}
          onOpen={() => void handleOpen()}
          disabled={!logDir}
        />
      )}
    </SettingContainer>
  );
};
