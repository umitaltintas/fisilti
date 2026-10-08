import React from "react";
import ReactDOM from "react-dom/client";
import { platform } from "@tauri-apps/plugin-os";
import App from "./App";

// Set platform before render so CSS can scope per-platform (e.g. scrollbar styles)
document.documentElement.dataset.platform = platform();

// Initialize i18n
import "./i18n";

// App-wide stores are initialized exactly once, here, rather than by whichever
// component happens to mount first. Each registers its backend event
// listeners for the lifetime of the window.
import { useModelStore } from "./stores/modelStore";
import { useSettingsStore } from "./stores/settingsStore";
import { useMeetingStore } from "./stores/meetingStore";
void useModelStore.getState().initialize();
void useSettingsStore.getState().initialize();
void useMeetingStore.getState().initialize();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
