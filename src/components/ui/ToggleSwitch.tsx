import React, { useId } from "react";
import { SettingContainer } from "./SettingContainer";

interface ToggleSwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  isUpdating?: boolean;
  label: string;
  description: string;
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  tooltipPosition?: "top" | "bottom";
}

export const ToggleSwitch: React.FC<ToggleSwitchProps> = ({
  checked,
  onChange,
  disabled = false,
  isUpdating = false,
  label,
  description,
  descriptionMode = "tooltip",
  grouped = false,
  tooltipPosition = "top",
}) => {
  const id = useId();
  const labelId = `${id}-label`;
  const descriptionId = `${id}-description`;

  return (
    <SettingContainer
      title={label}
      description={description}
      descriptionMode={descriptionMode}
      grouped={grouped}
      disabled={disabled}
      tooltipPosition={tooltipPosition}
      labelId={labelId}
      descriptionId={descriptionId}
    >
      <div className="relative inline-flex items-center">
        {/* The real control, invisible but stretched over the track so a
            click anywhere on it toggles. Named by the row's title. */}
        <input
          id={id}
          type="checkbox"
          role="switch"
          aria-labelledby={labelId}
          aria-describedby={descriptionId}
          aria-busy={isUpdating || undefined}
          className="peer absolute inset-0 z-10 m-0 h-full w-full cursor-pointer opacity-0 disabled:cursor-not-allowed"
          checked={checked}
          disabled={disabled || isUpdating}
          onChange={(e) => onChange(e.target.checked)}
        />
        <div
          aria-hidden
          className="relative h-5 w-[34px] rounded-full bg-mid-gray/35 transition-colors peer-focus-visible:ring-2 peer-focus-visible:ring-logo-primary/60 peer-focus-visible:ring-offset-1 peer-checked:bg-background-ui peer-checked:after:translate-x-[14px] rtl:peer-checked:after:-translate-x-[14px] after:absolute after:start-[2px] after:top-[2px] after:h-4 after:w-4 after:rounded-full after:bg-white after:shadow-[0_1px_2px_rgb(0_0_0/0.3)] after:transition-transform after:content-[''] peer-disabled:opacity-50"
        />
      </div>
      {isUpdating && (
        <div className="absolute inset-0 flex items-center justify-center">
          <div className="w-4 h-4 border-2 border-logo-primary border-t-transparent rounded-full animate-spin"></div>
        </div>
      )}
    </SettingContainer>
  );
};
