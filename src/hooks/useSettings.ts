import { useEffect } from "react";
import { useShallow } from "zustand/react/shallow";
import { useSettingsStore, type Settings } from "../stores/settingsStore";
import type { AudioDevice } from "@/bindings";

interface UseSettingsReturn {
  // State
  settings: Settings | null;
  isLoading: boolean;
  isUpdating: (key: string) => boolean;
  audioDevices: AudioDevice[];
  outputDevices: AudioDevice[];
  audioFeedbackEnabled: boolean;
  postProcessModelOptions: Record<string, string[]>;

  // Actions
  updateSetting: <K extends keyof Settings>(
    key: K,
    value: Settings[K],
  ) => Promise<boolean>;
  resetSetting: (key: keyof Settings) => Promise<boolean>;
  refreshSettings: () => Promise<void>;
  refreshAudioDevices: () => Promise<void>;
  refreshOutputDevices: () => Promise<void>;

  // Binding-specific actions
  updateBinding: (id: string, binding: string) => Promise<void>;
  resetBinding: (id: string) => Promise<boolean>;

  // Convenience getters
  getSetting: <K extends keyof Settings>(key: K) => Settings[K] | undefined;

  // Post-processing helpers
  setPostProcessProvider: (providerId: string) => Promise<boolean>;
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
}

/**
 * Convenience view over the settings store.
 *
 * Subscribes only to the slices it returns (shallow-compared), so a component
 * using it no longer re-renders on unrelated store churn such as default
 * settings or custom-sound detection. Components that need a single value
 * should prefer `useSettingsStore((s) => s.settings?.foo)` directly.
 */
export const useSettings = (): UseSettingsReturn => {
  const state = useSettingsStore(
    useShallow((s) => ({
      settings: s.settings,
      isLoading: s.isLoading,
      // Subscribed so `isUpdating(key)` callers re-render when it changes.
      isUpdatingMap: s.isUpdating,
      audioDevices: s.audioDevices,
      outputDevices: s.outputDevices,
      postProcessModelOptions: s.postProcessModelOptions,
      initialize: s.initialize,
      isUpdatingKey: s.isUpdatingKey,
      updateSetting: s.updateSetting,
      resetSetting: s.resetSetting,
      refreshSettings: s.refreshSettings,
      refreshAudioDevices: s.refreshAudioDevices,
      refreshOutputDevices: s.refreshOutputDevices,
      updateBinding: s.updateBinding,
      resetBinding: s.resetBinding,
      getSetting: s.getSetting,
      setPostProcessProvider: s.setPostProcessProvider,
      updatePostProcessBaseUrl: s.updatePostProcessBaseUrl,
      updatePostProcessApiKey: s.updatePostProcessApiKey,
      updatePostProcessModel: s.updatePostProcessModel,
      fetchPostProcessModels: s.fetchPostProcessModels,
    })),
  );

  // The store is initialized once at startup (main.tsx); this is a no-op
  // safety net for windows that mount a settings consumer without it.
  const { initialize } = state;
  useEffect(() => {
    void initialize();
  }, [initialize]);

  return {
    settings: state.settings,
    isLoading: state.isLoading,
    isUpdating: state.isUpdatingKey,
    audioDevices: state.audioDevices,
    outputDevices: state.outputDevices,
    audioFeedbackEnabled: state.settings?.audio_feedback || false,
    postProcessModelOptions: state.postProcessModelOptions,
    updateSetting: state.updateSetting,
    resetSetting: state.resetSetting,
    refreshSettings: state.refreshSettings,
    refreshAudioDevices: state.refreshAudioDevices,
    refreshOutputDevices: state.refreshOutputDevices,
    updateBinding: state.updateBinding,
    resetBinding: state.resetBinding,
    getSetting: state.getSetting,
    setPostProcessProvider: state.setPostProcessProvider,
    updatePostProcessBaseUrl: state.updatePostProcessBaseUrl,
    updatePostProcessApiKey: state.updatePostProcessApiKey,
    updatePostProcessModel: state.updatePostProcessModel,
    fetchPostProcessModels: state.fetchPostProcessModels,
  };
};
