import React, { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Tooltip } from "./Tooltip";

interface SettingContainerProps {
  title: string;
  description: string;
  children: React.ReactNode;
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  layout?: "horizontal" | "stacked";
  disabled?: boolean;
  tooltipPosition?: "top" | "bottom";
  /** Id put on the title, so the control can point `aria-labelledby` at it. */
  labelId?: string;
  /** Id put on the description, for `aria-describedby`. */
  descriptionId?: string;
}

/** The ⓘ next to a setting's title: a real button that shows the description
 * on hover, focus or click. */
const InfoTooltip: React.FC<{
  description: string;
  descriptionId?: string;
  position: "top" | "bottom";
}> = ({ description, descriptionId, position }) => {
  const { t } = useTranslation();
  const [showTooltip, setShowTooltip] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);

  // Close on a click anywhere else.
  useEffect(() => {
    if (!showTooltip) return;
    const handleClickOutside = (event: MouseEvent) => {
      if (
        wrapperRef.current &&
        !wrapperRef.current.contains(event.target as Node)
      ) {
        setShowTooltip(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [showTooltip]);

  return (
    <div
      ref={wrapperRef}
      className="relative flex"
      onMouseEnter={() => setShowTooltip(true)}
      onMouseLeave={() => setShowTooltip(false)}
    >
      <button
        type="button"
        aria-label={t("common.moreInfo")}
        aria-expanded={showTooltip}
        onClick={() => setShowTooltip((value) => !value)}
        onFocus={() => setShowTooltip(true)}
        onBlur={() => setShowTooltip(false)}
        onKeyDown={(event) => {
          if (event.key === "Escape") setShowTooltip(false);
        }}
        className="rounded-full text-faint cursor-help hover:text-logo-primary focus:outline-none focus-visible:ring-1 focus-visible:ring-logo-primary transition-colors duration-200"
      >
        <svg
          className="w-3.5 h-3.5 select-none"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
          aria-hidden
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth={2}
            d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
          />
        </svg>
      </button>
      {/* Kept in the DOM (visually hidden) so aria-describedby always
          resolves, not only while the tooltip is showing. */}
      {descriptionId && (
        <span id={descriptionId} className="sr-only">
          {description}
        </span>
      )}
      {showTooltip && (
        <Tooltip targetRef={wrapperRef} position={position}>
          <p className="text-sm text-center leading-relaxed">{description}</p>
        </Tooltip>
      )}
    </div>
  );
};

export const SettingContainer: React.FC<SettingContainerProps> = ({
  title,
  description,
  children,
  descriptionMode = "inline",
  grouped = false,
  layout = "horizontal",
  disabled = false,
  tooltipPosition = "top",
  labelId,
  descriptionId,
}) => {
  const fallbackId = useId();
  const titleId = labelId ?? `${fallbackId}-title`;
  const dimmed = disabled ? "opacity-50" : "";

  const titleElement = (
    <h3 id={titleId} className={`text-[13px] font-medium ${dimmed}`}>
      {title}
    </h3>
  );

  const header =
    descriptionMode === "tooltip" ? (
      <div className="flex items-center gap-2">
        {titleElement}
        <InfoTooltip
          description={description}
          descriptionId={descriptionId}
          position={layout === "stacked" ? "top" : tooltipPosition}
        />
      </div>
    ) : (
      <>
        {titleElement}
        <p
          id={descriptionId}
          className={`mt-0.5 text-xs leading-snug text-sub ${dimmed}`}
        >
          {description}
        </p>
      </>
    );

  if (layout === "stacked") {
    const containerClasses = grouped ? "px-3.5 py-2.5" : "card px-3.5 py-2.5";
    return (
      <div className={containerClasses}>
        <div className="mb-2">{header}</div>
        <div className="w-full">{children}</div>
      </div>
    );
  }

  const horizontalContainerClasses = grouped
    ? "flex min-h-[46px] items-center justify-between gap-4 px-3.5 py-2.5"
    : "card flex min-h-[46px] items-center justify-between gap-4 px-3.5 py-2.5";

  return (
    <div className={horizontalContainerClasses}>
      <div className="max-w-2/3 min-w-0">{header}</div>
      <div className="relative">{children}</div>
    </div>
  );
};
