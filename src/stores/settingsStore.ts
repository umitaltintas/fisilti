import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import type {
  AppSettings,
  AudioDevice,
  LogLevel,
  ModelUnloadTimeout,
  WhisperAcceleratorSetting,
  OrtAcceleratorSetting,
} from "@/bindings";
import { commands } from "@/bindings";
import i18n from "@/i18n";
import { changeMeetingExportDir } from "@/lib/meeting";
import { errorMessage } from "@/lib/utils/errors";

/**
 * App settings as the frontend sees them.
 *
 * `meeting_export_dir` exists in the Rust `AppSettings` but is missing from
 * the generated bindings until they are regenerated; it is declared here so it
 * can flow through the same store as every other setting.
 */
export type Settings = AppSettings & { meeting_export_dir?: string };

type PostProcessSettingType = "base_url" | "api_key" | "model";

interface SettingsStore {
  settings: Settings | null;
  defaultSettings: Settings | null;
  isLoading: boolean;
  /** Set when the initial settings load failed (the page shows a retry). */
  loadError: string | null;
  isUpdating: Record<string, boolean>;
  audioDevices: AudioDevice[];
  outputDevices: AudioDevice[];
  customSounds: { start: boolean; stop: boolean };
  postProcessModelOptions: Record<string, string[]>;

  // Actions
  initialize: () => Promise<void>;
  loadDefaultSettings: () => Promise<void>;
  /** Persist one setting optimistically. Resolves `false` (after rolling the
   * value back and showing an error toast) when the backend rejected it. */
  updateSetting: <K extends keyof Settings>(
    key: K,
    value: Settings[K],
  ) => Promise<boolean>;
  resetSetting: (key: keyof Settings) => Promise<boolean>;
  refreshSettings: () => Promise<void>;
  refreshAudioDevices: () => Promise<void>;
  refreshOutputDevices: () => Promise<void>;
  updateBinding: (id: string, binding: string) => Promise<void>;
  resetBinding: (id: string) => Promise<boolean>;
  getSetting: <K extends keyof Settings>(key: K) => Settings[K] | undefined;
  isUpdatingKey: (key: string) => boolean;
  playTestSound: (soundType: "start" | "stop") => Promise<void>;
  checkCustomSounds: () => Promise<void>;
  setPostProcessProvider: (providerId: string) => Promise<boolean>;
  updatePostProcessSetting: (
    settingType: PostProcessSettingType,
    providerId: string,
    value: string,
  ) => Promise<boolean>;
  updatePostProcessBaseUrl: (
    providerId: string,
    baseUrl: string,
  ) => Promise<boolean>;
  updatePostProcessApiKey: (
    providerId: string,
    apiKey: string,
  ) => Promise<boolean>;
  updatePostProcessModel: (
    providerId: string,
    model: string,
  ) => Promise<boolean>;
  fetchPostProcessModels: (providerId: string) => Promise<string[]>;
  setPostProcessModelOptions: (providerId: string, models: string[]) => void;

  // Internal state setters
  setSettings: (settings: Settings | null) => void;
  setDefaultSettings: (defaultSettings: Settings | null) => void;
  setLoading: (loading: boolean) => void;
  setUpdating: (key: string, updating: boolean) => void;
  setAudioDevices: (devices: AudioDevice[]) => void;
  setOutputDevices: (devices: AudioDevice[]) => void;
  setCustomSounds: (sounds: { start: boolean; stop: boolean }) => void;
}

// Note: Default settings are now fetched from Rust via commands.getDefaultSettings()
// This ensures platform-specific defaults (like overlay_position, shortcuts, paste_method) work correctly

const DEFAULT_AUDIO_DEVICE: AudioDevice = {
  index: "default",
  name: "Default",
  is_default: true,
};

/**
 * specta commands do not throw on a backend error: they resolve
 * `{ status: "error", error }`. Treating that as success is how a rejected
 * change used to stay on screen as if it had been saved. This turns it back
 * into a throw so one catch handles both failure shapes.
 */
const assertOk = (result: unknown): void => {
  if (
    result !== null &&
    typeof result === "object" &&
    "status" in result &&
    (result as { status: unknown }).status === "error"
  ) {
    throw (result as { error?: unknown }).error ?? "error";
  }
};

const settingUpdaters: {
  [K in keyof Settings]?: (value: Settings[K]) => Promise<unknown>;
} = {
  always_on_microphone: (value) =>
    commands.updateMicrophoneMode(value as boolean),
  audio_feedback: (value) =>
    commands.changeAudioFeedbackSetting(value as boolean),
  audio_feedback_volume: (value) =>
    commands.changeAudioFeedbackVolumeSetting(value as number),
  sound_theme: (value) => commands.changeSoundThemeSetting(value as string),
  start_hidden: (value) => commands.changeStartHiddenSetting(value as boolean),
  autostart_enabled: (value) =>
    commands.changeAutostartSetting(value as boolean),
  update_checks_enabled: (value) =>
    commands.changeUpdateChecksSetting(value as boolean),
  push_to_talk: (value) => commands.changePttSetting(value as boolean),
  selected_microphone: (value) =>
    commands.setSelectedMicrophone(
      value === "Default" || value === null || value === undefined
        ? "default"
        : value,
    ),
  clamshell_microphone: (value) =>
    commands.setClamshellMicrophone(
      value === "Default" || value === null || value === undefined
        ? "default"
        : value,
    ),
  selected_output_device: (value) =>
    commands.setSelectedOutputDevice(
      value === "Default" || value === null || value === undefined
        ? "default"
        : value,
    ),
  recording_retention_period: (value) =>
    commands.updateRecordingRetentionPeriod(value as string),
  translate_to_english: (value) =>
    commands.changeTranslateToEnglishSetting(value as boolean),
  selected_language: (value) =>
    commands.changeSelectedLanguageSetting(value as string),
  openrouter_custom_model: (value) =>
    commands.changeOpenrouterCustomModelSetting(value as string),
  gemini_api_key: (value) =>
    commands.changeGeminiApiKeySetting(value as string),
  overlay_position: (value) =>
    commands.changeOverlayPositionSetting(value as string),
  debug_mode: (value) => commands.changeDebugModeSetting(value as boolean),
  custom_words: (value) => commands.updateCustomWords(value as string[]),
  word_correction_threshold: (value) =>
    commands.changeWordCorrectionThresholdSetting(value as number),
  paste_method: (value) => commands.changePasteMethodSetting(value as string),
  typing_tool: (value) => commands.changeTypingToolSetting(value as string),
  external_script_path: (value) =>
    commands.changeExternalScriptPathSetting(value),
  clipboard_handling: (value) =>
    commands.changeClipboardHandlingSetting(value as string),
  auto_submit: (value) => commands.changeAutoSubmitSetting(value as boolean),
  auto_submit_key: (value) =>
    commands.changeAutoSubmitKeySetting(value as string),
  history_limit: (value) => commands.updateHistoryLimit(value as number),
  post_process_enabled: (value) =>
    commands.changePostProcessEnabledSetting(value as boolean),
  post_process_selected_prompt_id: (value) =>
    commands.setPostProcessSelectedPrompt(value ?? ""),
  mute_while_recording: (value) =>
    commands.changeMuteWhileRecordingSetting(value as boolean),
  append_trailing_space: (value) =>
    commands.changeAppendTrailingSpaceSetting(value as boolean),
  log_level: (value) => commands.setLogLevel(value as LogLevel),
  app_language: (value) => commands.changeAppLanguageSetting(value as string),
  experimental_enabled: (value) =>
    commands.changeExperimentalEnabledSetting(value as boolean),
  lazy_stream_close: (value) =>
    commands.changeLazyStreamCloseSetting(value as boolean),
  show_tray_icon: (value) =>
    commands.changeShowTrayIconSetting(value as boolean),
  whisper_accelerator: (value) =>
    commands.changeWhisperAcceleratorSetting(
      value as WhisperAcceleratorSetting,
    ),
  ort_accelerator: (value) =>
    commands.changeOrtAcceleratorSetting(value as OrtAcceleratorSetting),
  extra_recording_buffer_ms: (value) =>
    commands.changeExtraRecordingBufferSetting(value as number),
  model_unload_timeout: (value) =>
    commands.setModelUnloadTimeout(value as ModelUnloadTimeout),

  // Meeting settings. They used to be read and written by the meeting page on
  // its own, outside this store, so a failure there had no rollback and the
  // rest of the app never saw the new value.
  meeting_selected_model: (value) =>
    commands.changeMeetingSelectedModelSetting(value ?? ""),
  meeting_auto_summarize: (value) =>
    commands.changeMeetingAutoSummarizeSetting(value as boolean),
  meeting_calendar_names: (value) =>
    commands.changeMeetingCalendarNamesSetting(value as boolean),
  meeting_auto_detect: (value) =>
    commands.changeMeetingAutoDetectSetting(value as boolean),
  meeting_auto_end: (value) =>
    commands.changeMeetingAutoEndSetting(value as boolean),
  meeting_silence_timeout_secs: (value) =>
    commands.changeMeetingSilenceTimeoutSetting(value as number),
  meeting_auto_end_grace_secs: (value) =>
    commands.changeMeetingAutoEndGraceSetting(value as number),
  meeting_live_mode: (value) =>
    commands.changeMeetingLiveModeSetting(value as string),
  meeting_live_translate_target: (value) =>
    commands.changeMeetingLiveTranslateTargetSetting(value as string),
  meeting_gemini_diarize: (value) =>
    commands.changeMeetingGeminiDiarizeSetting(value as boolean),
  meeting_gemini_smart: (value) =>
    commands.changeMeetingGeminiSmartSetting(value as boolean),
  meeting_subtitles: (value) =>
    commands.changeMeetingSubtitlesSetting(value as boolean),
  meeting_export_dir: (value) => changeMeetingExportDir(value ?? ""),
};

const showSaveError = (error: unknown) => {
  toast.error(i18n.t("errors.settingSaveFailed"), {
    description: errorMessage(error),
  });
};

// Initialization runs once per window, however many components ask for it.
// The promise is stored before the first await so concurrent callers share it.
let initPromise: Promise<void> | null = null;

export const useSettingsStore = create<SettingsStore>()(
  subscribeWithSelector((set, get) => ({
    settings: null,
    defaultSettings: null,
    isLoading: true,
    loadError: null,
    isUpdating: {},
    audioDevices: [],
    outputDevices: [],
    customSounds: { start: false, stop: false },
    postProcessModelOptions: {},

    // Internal setters
    setSettings: (settings) => set({ settings }),
    setDefaultSettings: (defaultSettings) => set({ defaultSettings }),
    setLoading: (isLoading) => set({ isLoading }),
    setUpdating: (key, updating) =>
      set((state) => ({
        isUpdating: { ...state.isUpdating, [key]: updating },
      })),
    setAudioDevices: (audioDevices) => set({ audioDevices }),
    setOutputDevices: (outputDevices) => set({ outputDevices }),
    setCustomSounds: (customSounds) => set({ customSounds }),

    // Getters
    getSetting: (key) => get().settings?.[key],
    isUpdatingKey: (key) => get().isUpdating[key] || false,

    // Load settings from store
    refreshSettings: async () => {
      try {
        const result = await commands.getAppSettings();
        if (result.status === "ok") {
          const settings: Settings = result.data;
          const normalizedSettings: Settings = {
            ...settings,
            always_on_microphone: settings.always_on_microphone ?? false,
            selected_microphone: settings.selected_microphone ?? "Default",
            clamshell_microphone: settings.clamshell_microphone ?? "Default",
            selected_output_device:
              settings.selected_output_device ?? "Default",
          };
          set({
            settings: normalizedSettings,
            isLoading: false,
            loadError: null,
          });
        } else {
          console.error("Failed to load settings:", result.error);
          set({ isLoading: false, loadError: errorMessage(result.error) });
        }
      } catch (error) {
        console.error("Failed to load settings:", error);
        set({ isLoading: false, loadError: errorMessage(error) });
      }
    },

    // Load audio devices
    refreshAudioDevices: async () => {
      try {
        const result = await commands.getAvailableMicrophones();
        if (result.status === "ok") {
          const devicesWithDefault = [
            DEFAULT_AUDIO_DEVICE,
            ...result.data.filter(
              (d) => d.name !== "Default" && d.name !== "default",
            ),
          ];
          set({ audioDevices: devicesWithDefault });
        } else {
          set({ audioDevices: [DEFAULT_AUDIO_DEVICE] });
        }
      } catch (error) {
        console.error("Failed to load audio devices:", error);
        set({ audioDevices: [DEFAULT_AUDIO_DEVICE] });
      }
    },

    // Load output devices
    refreshOutputDevices: async () => {
      try {
        const result = await commands.getAvailableOutputDevices();
        if (result.status === "ok") {
          const devicesWithDefault = [
            DEFAULT_AUDIO_DEVICE,
            ...result.data.filter(
              (d) => d.name !== "Default" && d.name !== "default",
            ),
          ];
          set({ outputDevices: devicesWithDefault });
        } else {
          set({ outputDevices: [DEFAULT_AUDIO_DEVICE] });
        }
      } catch (error) {
        console.error("Failed to load output devices:", error);
        set({ outputDevices: [DEFAULT_AUDIO_DEVICE] });
      }
    },

    // Play a test sound
    playTestSound: async (soundType: "start" | "stop") => {
      try {
        await commands.playTestSound(soundType);
      } catch (error) {
        console.error(`Failed to play test sound (${soundType}):`, error);
      }
    },

    checkCustomSounds: async () => {
      try {
        const sounds = await commands.checkCustomSounds();
        get().setCustomSounds(sounds);
      } catch (error) {
        console.error("Failed to check custom sounds:", error);
      }
    },

    // Update a specific setting
    updateSetting: async <K extends keyof Settings>(
      key: K,
      value: Settings[K],
    ) => {
      const { setUpdating } = get();
      const updateKey = String(key);
      const previousValue = get().settings?.[key];

      setUpdating(updateKey, true);
      set((state) => ({
        settings: state.settings ? { ...state.settings, [key]: value } : null,
      }));

      try {
        const updater = settingUpdaters[key];
        if (updater) {
          assertOk(await updater(value));
        } else if (key !== "bindings" && key !== "selected_model") {
          console.warn(`No handler for setting: ${String(key)}`);
        }
        return true;
      } catch (error) {
        console.error(`Failed to update setting ${String(key)}:`, error);
        // Undo only this key, and only if it still holds the value we wrote:
        // restoring a whole earlier snapshot would also revert any other
        // setting changed while this request was in flight.
        set((state) =>
          state.settings && Object.is(state.settings[key], value)
            ? { settings: { ...state.settings, [key]: previousValue } }
            : {},
        );
        showSaveError(error);
        return false;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Reset a setting to its default value
    resetSetting: async (key) => {
      const { defaultSettings } = get();
      const defaultValue = defaultSettings?.[key];
      if (defaultValue === undefined) return false;
      return get().updateSetting(key, defaultValue);
    },

    // Update a specific binding
    updateBinding: async (id, binding) => {
      const { settings, setUpdating } = get();
      const updateKey = `binding_${id}`;
      const originalBinding = settings?.bindings?.[id]?.current_binding;

      setUpdating(updateKey, true);

      try {
        // Optimistic update
        set((state) => {
          const existing = state.settings?.bindings[id];
          if (!state.settings || !existing) return {};
          return {
            settings: {
              ...state.settings,
              bindings: {
                ...state.settings.bindings,
                [id]: { ...existing, current_binding: binding },
              },
            },
          };
        });

        const result = await commands.changeBinding(id, binding);

        // Check if the command executed successfully
        if (result.status === "error") {
          throw new Error(result.error);
        }

        // Check if the binding change was successful
        if (!result.data.success) {
          throw new Error(result.data.error || "binding rejected");
        }
      } catch (error) {
        console.error(`Failed to update binding ${id}:`, error);

        // Roll back this binding only, and only if it is still ours.
        if (originalBinding) {
          set((state) => {
            const existing = state.settings?.bindings[id];
            if (
              !state.settings ||
              !existing ||
              existing.current_binding !== binding
            ) {
              return {};
            }
            return {
              settings: {
                ...state.settings,
                bindings: {
                  ...state.settings.bindings,
                  [id]: { ...existing, current_binding: originalBinding },
                },
              },
            };
          });
        }

        // Re-throw to let the caller know it failed
        throw error;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Reset a specific binding
    resetBinding: async (id) => {
      const { setUpdating, refreshSettings } = get();
      const updateKey = `binding_${id}`;

      setUpdating(updateKey, true);

      try {
        assertOk(await commands.resetBinding(id));
        await refreshSettings();
        return true;
      } catch (error) {
        console.error(`Failed to reset binding ${id}:`, error);
        showSaveError(error);
        return false;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    setPostProcessProvider: async (providerId) => {
      const { settings, setUpdating, refreshSettings } = get();
      const updateKey = "post_process_provider_id";
      const previousId = settings?.post_process_provider_id ?? null;

      setUpdating(updateKey, true);

      set((state) => ({
        settings: state.settings
          ? { ...state.settings, post_process_provider_id: providerId }
          : null,
      }));

      // Clear cached model options for the new provider so the dropdown
      // doesn't show stale models from a previous fetch or base_url.
      get().setPostProcessModelOptions(providerId, []);

      try {
        assertOk(await commands.setPostProcessProvider(providerId));
        await refreshSettings();
        return true;
      } catch (error) {
        console.error("Failed to set post-process provider:", error);
        if (previousId !== null) {
          set((state) =>
            state.settings?.post_process_provider_id === providerId
              ? {
                  settings: {
                    ...state.settings,
                    post_process_provider_id: previousId,
                  },
                }
              : {},
          );
        }
        showSaveError(error);
        return false;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    // Generic updater for post-processing provider settings
    updatePostProcessSetting: async (settingType, providerId, value) => {
      const { setUpdating, refreshSettings } = get();
      const updateKey = `post_process_${settingType}:${providerId}`;

      setUpdating(updateKey, true);

      try {
        if (settingType === "base_url") {
          assertOk(
            await commands.changePostProcessBaseUrlSetting(providerId, value),
          );
        } else if (settingType === "api_key") {
          assertOk(
            await commands.changePostProcessApiKeySetting(providerId, value),
          );
        } else {
          assertOk(
            await commands.changePostProcessModelSetting(providerId, value),
          );
        }
        await refreshSettings();
        return true;
      } catch (error) {
        console.error(`Failed to update post-process ${settingType}:`, error);
        showSaveError(error);
        return false;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updatePostProcessBaseUrl: async (providerId, baseUrl) => {
      const { setUpdating, refreshSettings } = get();
      const updateKey = `post_process_base_url:${providerId}`;

      setUpdating(updateKey, true);

      try {
        // Persist the new base URL first.
        assertOk(
          await commands.changePostProcessBaseUrlSetting(providerId, baseUrl),
        );

        // Reset the stored model since the previous value is almost certainly
        // invalid for the new endpoint (e.g. switching Custom from Groq to
        // Cerebras). Only proceed if the reset succeeds.
        assertOk(await commands.changePostProcessModelSetting(providerId, ""));

        // Clear cached model options only after both backend writes succeed.
        get().setPostProcessModelOptions(providerId, []);

        // Single refresh after both backend writes.
        await refreshSettings();
        return true;
      } catch (error) {
        console.error("Failed to update post-process base URL:", error);
        showSaveError(error);
        return false;
      } finally {
        setUpdating(updateKey, false);
      }
    },

    updatePostProcessApiKey: async (providerId, apiKey) => {
      const saved = await get().updatePostProcessSetting(
        "api_key",
        providerId,
        apiKey,
      );
      // A new key may unlock a different model list; drop the cached one so
      // the user refreshes against the new credential. Only once it saved.
      if (saved) get().setPostProcessModelOptions(providerId, []);
      return saved;
    },

    updatePostProcessModel: async (providerId, model) => {
      return get().updatePostProcessSetting("model", providerId, model);
    },

    fetchPostProcessModels: async (providerId) => {
      const updateKey = `post_process_models_fetch:${providerId}`;
      const { setUpdating, setPostProcessModelOptions } = get();

      setUpdating(updateKey, true);

      try {
        const result = await commands.fetchPostProcessModels(providerId);
        if (result.status === "ok") {
          setPostProcessModelOptions(providerId, result.data);
          return result.data;
        }
        throw result.error;
      } catch (error) {
        // Don't cache an empty list on error - let the user retry.
        console.error("Failed to fetch models:", error);
        toast.error(i18n.t("errors.fetchModelsFailed"), {
          description: errorMessage(error),
        });
        return [];
      } finally {
        setUpdating(updateKey, false);
      }
    },

    setPostProcessModelOptions: (providerId, models) =>
      set((state) => ({
        postProcessModelOptions: {
          ...state.postProcessModelOptions,
          [providerId]: models,
        },
      })),

    // Load default settings from Rust
    loadDefaultSettings: async () => {
      try {
        const result = await commands.getDefaultSettings();
        if (result.status === "ok") {
          set({ defaultSettings: result.data });
        } else {
          console.error("Failed to load default settings:", result.error);
        }
      } catch (error) {
        console.error("Failed to load default settings:", error);
      }
    },

    // Initialize everything (idempotent).
    initialize: () => {
      if (initPromise) return initPromise;
      initPromise = (async () => {
        // Re-fetch settings when the backend changes them (e.g. language
        // reset during model switch). The backend is the source of truth.
        // Registered once for the window's lifetime.
        void listen("model-state-changed", () => {
          void get().refreshSettings();
        });

        // Note: Audio devices are NOT refreshed here. The frontend (App.tsx)
        // is responsible for calling refreshAudioDevices/refreshOutputDevices
        // after onboarding completes. This avoids triggering permission dialogs
        // on macOS before the user is ready.
        const { refreshSettings, checkCustomSounds, loadDefaultSettings } =
          get();
        await Promise.all([
          loadDefaultSettings(),
          refreshSettings(),
          checkCustomSounds(),
        ]);
      })();
      return initPromise;
    },
  })),
);
