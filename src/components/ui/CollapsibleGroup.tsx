import React, { useId, useState } from "react";
import { ChevronDown } from "lucide-react";

interface CollapsibleGroupProps {
  title: string;
  /** Small muted count/summary shown next to the title. */
  count?: number;
  description?: string;
  defaultOpen?: boolean;
  children: React.ReactNode;
}

/**
 * A titled section that can be folded away. Used to keep long lists (models,
 * rarely-touched settings) collapsed by default so a page shows only what the
 * user is likely to need.
 */
export const CollapsibleGroup: React.FC<CollapsibleGroupProps> = ({
  title,
  count,
  description,
  defaultOpen = false,
  children,
}) => {
  const [open, setOpen] = useState(defaultOpen);
  const contentId = useId();

  return (
    <div className="space-y-2">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={contentId}
        onClick={() => setOpen((prev) => !prev)}
        className="group flex w-full items-center gap-2 px-1 py-1 text-start cursor-pointer"
      >
        <ChevronDown
          className={`w-4 h-4 shrink-0 text-faint transition-transform ${
            open ? "" : "-rotate-90"
          }`}
        />
        <h2 className="text-xs font-semibold text-sub transition-colors group-hover:text-text">
          {title}
        </h2>
        {count !== undefined && (
          <span className="text-xs text-faint tabular-nums">{count}</span>
        )}
      </button>
      {description && open && (
        <p className="px-1 text-xs text-sub">{description}</p>
      )}
      {open && (
        <div id={contentId} className="space-y-2">
          {children}
        </div>
      )}
    </div>
  );
};
