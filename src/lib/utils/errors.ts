/**
 * The human-readable text of anything a command or promise rejected with.
 *
 * Tauri commands reject with a bare string, JS failures with an `Error`, and
 * the occasional plugin with an object; `String(e)` turns the last into
 * "[object Object]". The result is only ever shown as the *detail* of a
 * localized message, never on its own.
 */
export const errorMessage = (error: unknown): string => {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message: unknown }).message;
    if (typeof message === "string") return message;
  }
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
};
