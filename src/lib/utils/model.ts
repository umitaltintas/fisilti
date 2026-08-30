import type { EngineType, ModelInfo } from "@/bindings";

/**
 * Which cloud provider a model talks to, or `null` when it runs on this device.
 *
 * Six components used to spell out `engine_type === "OpenRouter" ||
 * engine_type === "OpenRouterAsr"` inline, so adding a third cloud engine meant
 * finding all six. Engine-type knowledge lives here instead.
 */
export type CloudProvider = "openrouter" | "gemini";

export const cloudProviderOf = (
  engineType: EngineType | undefined,
): CloudProvider | null => {
  switch (engineType) {
    case "OpenRouter":
    case "OpenRouterAsr":
      return "openrouter";
    case "Gemini":
    case "GeminiLive":
      return "gemini";
    default:
      return null;
  }
};

/** Whether the model sends audio off the device to transcribe it. */
export const isCloudEngine = (engineType: EngineType | undefined): boolean =>
  cloudProviderOf(engineType) !== null;

/** Convenience wrapper for the common `model?.engine_type` case. */
export const isCloudModel = (model: ModelInfo | undefined | null): boolean =>
  isCloudEngine(model?.engine_type);

/**
 * Where a post-processing provider's API key is stored, or `null` when the key
 * belongs to that provider alone.
 *
 * Google and OpenRouter each share an account with a transcription engine, so
 * their key already has a home on the Models page. Offering a second field for
 * them under AI editing stores the same credential twice, and then whichever
 * copy was edited last decides whether the feature works — with nothing on
 * screen saying which one won.
 */
export const keyHomeForProvider = (
  providerId: string,
): CloudProvider | null => {
  switch (providerId) {
    case "google":
      return "gemini";
    case "openrouter":
      return "openrouter";
    default:
      return null;
  }
};
