import React, { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MicrophoneIcon,
  TranscriptionIcon,
  CancelIcon,
} from "../components/icons";
import "./RecordingOverlay.css";
import { commands } from "@/bindings";
import i18n, { syncLanguageFromSettings } from "@/i18n";
import { getLanguageDirection } from "@/lib/utils/rtl";
import { useTauriEvent } from "@/hooks/useTauriEvent";

type OverlayState = "recording" | "transcribing" | "processing";

const RecordingOverlay: React.FC = () => {
  const { t } = useTranslation();
  const [isVisible, setIsVisible] = useState(false);
  const [state, setState] = useState<OverlayState>("recording");
  const [levels, setLevels] = useState<number[]>(Array(16).fill(0));
  const smoothedLevelsRef = useRef<number[]>(Array(16).fill(0));
  const direction = getLanguageDirection(i18n.language);

  // Each subscription cleans up after itself even when the window unmounts
  // before `listen()` resolved (the old setup returned its cleanup from an
  // async function, where React never saw it).
  useTauriEvent<OverlayState>("show-overlay", (overlayState) => {
    // Sync language from settings each time overlay is shown, before the
    // text appears.
    void syncLanguageFromSettings()
      .catch(() => undefined)
      .then(() => {
        setState(overlayState);
        setIsVisible(true);
      });
  });

  useTauriEvent("hide-overlay", () => {
    setIsVisible(false);
  });

  useTauriEvent<number[]>("mic-level", (newLevels) => {
    // Apply smoothing to reduce jitter
    const smoothed = smoothedLevelsRef.current.map((prev, i) => {
      const target = newLevels[i] || 0;
      return prev * 0.7 + target * 0.3; // Smooth transition
    });

    smoothedLevelsRef.current = smoothed;
    setLevels(smoothed.slice(0, 9));
  });

  const getIcon = () => {
    if (state === "recording") {
      return <MicrophoneIcon />;
    } else {
      return <TranscriptionIcon />;
    }
  };

  return (
    <div
      dir={direction}
      className={`recording-overlay ${isVisible ? "fade-in" : ""}`}
    >
      <div className="overlay-left">{getIcon()}</div>

      <div className="overlay-middle">
        {state === "recording" && (
          <div className="bars-container">
            {levels.map((v, i) => (
              <div
                key={i}
                className="bar"
                style={{
                  height: `${Math.min(20, 4 + Math.pow(v, 0.7) * 16)}px`, // Cap at 20px max height
                  transition: "height 60ms ease-out, opacity 120ms ease-out",
                  opacity: Math.max(0.2, v * 1.7), // Minimum opacity for visibility
                }}
              />
            ))}
          </div>
        )}
        {state === "transcribing" && (
          <div className="transcribing-text">{t("overlay.transcribing")}</div>
        )}
        {state === "processing" && (
          <div className="transcribing-text">{t("overlay.processing")}</div>
        )}
      </div>

      <div className="overlay-right">
        {state === "recording" && (
          <button
            type="button"
            className="cancel-button"
            aria-label={t("overlay.cancel")}
            title={t("overlay.cancel")}
            onClick={() => {
              commands.cancelOperation().catch((error: unknown) => {
                console.error("Failed to cancel recording:", error);
              });
            }}
          >
            <CancelIcon />
          </button>
        )}
      </div>
    </div>
  );
};

export default RecordingOverlay;
