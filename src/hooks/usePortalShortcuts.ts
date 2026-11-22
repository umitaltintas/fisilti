import { useCallback, useEffect, useState } from "react";
import { commands, type PortalShortcutStatus } from "@/bindings";
import { useTauriEvent } from "./useTauriEvent";

const INITIAL: PortalShortcutStatus = {
  active: false,
  triggers: {},
  error: null,
};

/**
 * State of the XDG GlobalShortcuts portal backend (Linux/Wayland).
 *
 * When `active`, the desktop — not the app — owns the key choice: `triggers`
 * holds what it actually bound, and it is edited in the system settings.
 */
export function usePortalShortcuts(): PortalShortcutStatus & {
  openSystemSettings: () => Promise<string | null>;
} {
  const [status, setStatus] = useState<PortalShortcutStatus>(INITIAL);

  useEffect(() => {
    let cancelled = false;
    commands
      .getPortalShortcutStatus()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch((error: unknown) => {
        console.error("Failed to read portal shortcut status:", error);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useTauriEvent<PortalShortcutStatus>("portal-shortcuts-changed", setStatus);

  const openSystemSettings = useCallback(async () => {
    const result = await commands.configurePortalShortcuts();
    return result.status === "error" ? String(result.error) : null;
  }, []);

  return { ...status, openSystemSettings };
}
