/** The app's colour theme: follow macOS, or force light or dark. Stored per
 * machine in localStorage — it is a display preference, not app behaviour. */
export type ThemePreference = "system" | "light" | "dark";

const STORAGE_KEY = "fisilti-theme";

export const getThemePreference = (): ThemePreference => {
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored === "light" || stored === "dark" ? stored : "system";
};

/** Apply a theme to this window (`data-theme` on <html>, read by App.css). */
export const applyTheme = (preference: ThemePreference) => {
  const root = document.documentElement;
  if (preference === "system") delete root.dataset.theme;
  else root.dataset.theme = preference;
};

export const setThemePreference = (preference: ThemePreference) => {
  if (preference === "system") localStorage.removeItem(STORAGE_KEY);
  else localStorage.setItem(STORAGE_KEY, preference);
  applyTheme(preference);
  window.dispatchEvent(new Event("fisilti-theme-change"));
};
