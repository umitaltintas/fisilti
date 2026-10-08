import React from "react";

interface SettingsGroupProps {
  title?: string;
  description?: string;
  children: React.ReactNode;
}

/** A titled card of setting rows, in the style of macOS System Settings. */
export const SettingsGroup: React.FC<SettingsGroupProps> = ({
  title,
  description,
  children,
}) => {
  return (
    <section className="space-y-1.5">
      {title && (
        <div className="px-1">
          <h2 className="text-xs font-semibold text-sub">{title}</h2>
          {description && (
            <p className="mt-0.5 text-xs text-faint">{description}</p>
          )}
        </div>
      )}
      <div className="card overflow-visible">
        <div className="divide-y divide-line">{children}</div>
      </div>
    </section>
  );
};
