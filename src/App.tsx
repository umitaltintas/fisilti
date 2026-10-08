import { useCallback, useEffect, useState, useRef } from "react";
import { toast, Toaster } from "sonner";
import { useTranslation } from "react-i18next";
import { platform } from "@tauri-apps/plugin-os";
import {
  checkAccessibilityPermission,
  checkMicrophonePermission,
} from "tauri-plugin-macos-permissions-api";
import {
  DictationErrorEvent,
  ModelStateEvent,
  RecordingErrorEvent,
} from "./lib/types/events";
import "./App.css";
import AccessibilityPermissions from "./components/AccessibilityPermissions";
import Footer from "./components/footer";
import Onboarding, { AccessibilityOnboarding } from "./components/onboarding";
import { Sidebar, SidebarSection, SECTIONS_CONFIG } from "./components/Sidebar";
import { useSettingsStore } from "./stores/settingsStore";
import { useTauriEvent } from "./hooks/useTauriEvent";
import { commands } from "@/bindings";
import { getLanguageDirection, initializeRTL } from "@/lib/utils/rtl";

type OnboardingStep = "accessibility" | "model" | "done";

const renderSettingsContent = (section: SidebarSection) => {
  const ActiveComponent =
    SECTIONS_CONFIG[section]?.component || SECTIONS_CONFIG.general.component;
  return <ActiveComponent />;
};

/** Mounted once for the whole app, so toasts raised during onboarding (or
 * before the main UI renders) are not dropped. */
const AppToaster: React.FC = () => (
  <Toaster
    theme="system"
    toastOptions={{
      unstyled: true,
      classNames: {
        toast:
          "bg-background border border-mid-gray/20 rounded-lg shadow-lg px-4 py-3 flex items-center gap-3 text-sm",
        title: "font-medium",
        description: "text-mid-gray",
      },
    }}
  />
);

function App() {
  const { t, i18n } = useTranslation();
  const [onboardingStep, setOnboardingStep] = useState<OnboardingStep | null>(
    null,
  );
  // Track if this is a returning user who just needs to grant permissions
  // (vs a new user who needs full onboarding including model selection)
  const [isReturningUser, setIsReturningUser] = useState(false);
  const [currentSection, setCurrentSection] =
    useState<SidebarSection>("home");
  // Narrow selectors: App re-renders for these values only, not for every
  // settings change anywhere in the app.
  const debugMode = useSettingsStore((s) => s.settings?.debug_mode ?? false);
  const updateSetting = useSettingsStore((s) => s.updateSetting);
  const refreshAudioDevices = useSettingsStore(
    (state) => state.refreshAudioDevices,
  );
  const refreshOutputDevices = useSettingsStore(
    (state) => state.refreshOutputDevices,
  );
  const direction = getLanguageDirection(i18n.language);
  const hasCompletedPostOnboardingInit = useRef(false);

  // Initialize RTL direction when language changes
  useEffect(() => {
    initializeRTL(i18n.language);
  }, [i18n.language]);

  // Initialize Enigo, shortcuts, and refresh audio devices when main app loads
  useEffect(() => {
    if (onboardingStep === "done" && !hasCompletedPostOnboardingInit.current) {
      hasCompletedPostOnboardingInit.current = true;
      Promise.all([
        commands.initializeEnigo(),
        commands.initializeShortcuts(),
      ]).catch((e: unknown) => {
        console.warn("Failed to initialize:", e);
      });
      void refreshAudioDevices();
      void refreshOutputDevices();
    }
  }, [onboardingStep, refreshAudioDevices, refreshOutputDevices]);

  // Navigate to a sidebar section when the backend (the tray's "Meetings"
  // item) or a page (a "go to Models" link) asks for it.
  useTauriEvent<string>("navigate-section", (section) => {
    if (section in SECTIONS_CONFIG) {
      setCurrentSection(section as SidebarSection);
    }
  });

  // Handle keyboard shortcuts for debug mode toggle
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      // Check for Ctrl+Shift+D (Windows/Linux) or Cmd+Shift+D (macOS)
      const isDebugShortcut =
        event.shiftKey &&
        event.key.toLowerCase() === "d" &&
        (event.ctrlKey || event.metaKey);

      if (isDebugShortcut) {
        event.preventDefault();
        void updateSetting("debug_mode", !debugMode);
        // Debug settings are a group inside Advanced rather than their own
        // sidebar entry, so jump there to show what the shortcut revealed.
        if (!debugMode) {
          setCurrentSection("advanced");
        }
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [debugMode, updateSetting]);

  // Recording errors from the backend become a toast.
  useTauriEvent<RecordingErrorEvent>(
    "recording-error",
    ({ error_type, detail }) => {
      if (error_type === "microphone_permission_denied") {
        const currentPlatform = platform();
        const platformKey = `errors.micPermissionDenied.${currentPlatform}`;
        const description = t(platformKey, {
          defaultValue: t("errors.micPermissionDenied.generic"),
        });
        toast.error(t("errors.micPermissionDeniedTitle"), { description });
      } else {
        toast.error(
          t("errors.recordingFailed", {
            error: detail ?? t("errors.unknown"),
          }),
        );
      }
    },
  );

  // Dictation pipeline failures. "Nothing was heard" is not an error, so it
  // gets a gentle info toast instead of a red one.
  useTauriEvent<DictationErrorEvent>(
    "dictation-error",
    ({ stage, message }) => {
      const title = t(`errors.dictation.${stage}`, {
        defaultValue: t("errors.dictation.generic"),
      });
      const options = message ? { description: message } : undefined;
      if (stage === "no_speech") {
        toast.info(title, options);
      } else {
        toast.error(title, options);
      }
    },
  );

  // Model loading failures become a toast.
  useTauriEvent<ModelStateEvent>("model-state-changed", (payload) => {
    if (payload.event_type === "loading_failed") {
      toast.error(
        t("errors.modelLoadFailed", {
          model: payload.model_name || t("errors.modelLoadFailedUnknown"),
        }),
        {
          description: payload.error,
        },
      );
    }
  });

  const checkOnboardingStatus = useCallback(async () => {
    const revealMainWindowForPermissions = async () => {
      try {
        await commands.showMainWindowCommand();
      } catch (e) {
        console.warn(
          "Failed to show main window for permission onboarding:",
          e,
        );
      }
    };

    try {
      // Check if they have any models available
      const result = await commands.hasAnyModelsAvailable();
      const hasModels = result.status === "ok" && result.data;
      const currentPlatform = platform();

      if (hasModels) {
        // Returning user - check if they need to grant permissions first
        setIsReturningUser(true);

        if (currentPlatform === "macos") {
          try {
            const [hasAccessibility, hasMicrophone] = await Promise.all([
              checkAccessibilityPermission(),
              checkMicrophonePermission(),
            ]);
            if (!hasAccessibility || !hasMicrophone) {
              await revealMainWindowForPermissions();
              setOnboardingStep("accessibility");
              return;
            }
          } catch (e) {
            console.warn("Failed to check macOS permissions:", e);
            // If we can't check, proceed to main app and let them fix it there
          }
        }

        if (currentPlatform === "windows") {
          try {
            const microphoneStatus =
              await commands.getWindowsMicrophonePermissionStatus();
            if (
              microphoneStatus.supported &&
              microphoneStatus.overall_access === "denied"
            ) {
              await revealMainWindowForPermissions();
              setOnboardingStep("accessibility");
              return;
            }
          } catch (e) {
            console.warn("Failed to check Windows microphone permissions:", e);
            // If we can't check, proceed to main app and let them fix it there
          }
        }

        setOnboardingStep("done");
      } else {
        // New user - start full onboarding
        setIsReturningUser(false);
        setOnboardingStep("accessibility");
      }
    } catch (error) {
      console.error("Failed to check onboarding status:", error);
      setOnboardingStep("accessibility");
    }
  }, []);

  useEffect(() => {
    void checkOnboardingStatus();
  }, [checkOnboardingStatus]);

  // Stable identities: AccessibilityOnboarding runs its permission check in an
  // effect keyed on this callback, so a new function per render re-ran it.
  const handleAccessibilityComplete = useCallback(() => {
    // Returning users already have models, skip to main app
    // New users need to select a model
    setOnboardingStep(isReturningUser ? "done" : "model");
  }, [isReturningUser]);

  const handleModelSelected = useCallback(() => {
    // Transition to main app - the chosen model is downloaded and selected
    setOnboardingStep("done");
  }, []);

  let content: React.ReactNode;
  if (onboardingStep === null) {
    // Still checking onboarding status
    content = null;
  } else if (onboardingStep === "accessibility") {
    content = (
      <AccessibilityOnboarding onComplete={handleAccessibilityComplete} />
    );
  } else if (onboardingStep === "model") {
    content = <Onboarding onModelSelected={handleModelSelected} />;
  } else {
    content = (
      <div
        dir={direction}
        className="h-screen flex flex-col select-none cursor-default"
      >
        {/* Main content area that takes remaining space */}
        <div className="flex-1 flex overflow-hidden">
          <Sidebar
            activeSection={currentSection}
            onSectionChange={setCurrentSection}
          />
          {/* Scrollable content area */}
          <main className="flex-1 flex flex-col overflow-hidden">
            <div className="flex-1 overflow-y-auto">
              <div className="flex flex-col items-center gap-4 px-7 pt-5 pb-8">
                <AccessibilityPermissions />
                {renderSettingsContent(currentSection)}
              </div>
            </div>
          </main>
        </div>
        {/* Fixed footer at bottom */}
        <Footer />
      </div>
    );
  }

  return (
    <>
      <AppToaster />
      {content}
    </>
  );
}

export default App;
