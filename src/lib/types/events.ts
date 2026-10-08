export interface ModelStateEvent {
  event_type: string;
  model_id?: string;
  model_name?: string;
  error?: string;
}

export interface RecordingErrorEvent {
  error_type: string;
  detail?: string;
}

/** Where in the dictation pipeline something went wrong. */
export type DictationErrorStage =
  | "transcription"
  | "paste"
  | "model_load"
  | "no_speech"
  | "recording";

/** Payload of the `dictation-error` event. */
export interface DictationErrorEvent {
  stage: DictationErrorStage;
  /** Backend detail, shown under the localized title. */
  message: string;
}
