import React from "react";
import { useSettings } from "../../hooks/useSettings";
import { GlobalShortcutInput } from "./GlobalShortcutInput";
import { NativeKeysShortcutInput } from "./NativeKeysShortcutInput";
import { PortalShortcutInput } from "./PortalShortcutInput";
import { usePortalShortcuts } from "../../hooks/usePortalShortcuts";

interface ShortcutInputProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  shortcutId: string;
  disabled?: boolean;
}

/**
 * Wrapper component that selects the appropriate shortcut input implementation
 * based on the keyboard_implementation setting.
 *
 * - "tauri" (default): Uses GlobalShortcutInput with JS keyboard events
 * - "native": Uses NativeKeysShortcutInput with backend key events
 *
 * On Linux/Wayland the XDG GlobalShortcuts portal takes over regardless of
 * that setting: the desktop owns the key choice, so the row only displays it.
 */
export const ShortcutInput: React.FC<ShortcutInputProps> = (props) => {
  const { getSetting } = useSettings();
  const portal = usePortalShortcuts();
  const keyboardImplementation = getSetting("keyboard_implementation");

  if (portal.active) {
    return (
      <PortalShortcutInput
        {...props}
        trigger={portal.triggers[props.shortcutId]}
        error={portal.error}
        openSystemSettings={portal.openSystemSettings}
      />
    );
  }

  // Default to Tauri implementation if not set
  if (keyboardImplementation === "native") {
    return <NativeKeysShortcutInput {...props} />;
  }

  return <GlobalShortcutInput {...props} />;
};
