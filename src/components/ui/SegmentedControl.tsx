import React, { useRef } from "react";

export interface Segment<T extends string> {
  id: T;
  label: string;
}

interface SegmentedControlProps<T extends string> {
  segments: Segment<T>[];
  value: T;
  onChange: (value: T) => void;
  label: string;
  className?: string;
}

/** A macOS-style segmented control, used as a tab bar (WAI-ARIA tabs:
 * arrows move and select, Home/End jump). */
export function SegmentedControl<T extends string>({
  segments,
  value,
  onChange,
  label,
  className = "",
}: SegmentedControlProps<T>) {
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});

  const handleKeyDown = (event: React.KeyboardEvent) => {
    const index = segments.findIndex((s) => s.id === value);
    const rtl = document.dir === "rtl";
    let next: number | null = null;
    if (event.key === (rtl ? "ArrowLeft" : "ArrowRight"))
      next = (index + 1) % segments.length;
    else if (event.key === (rtl ? "ArrowRight" : "ArrowLeft"))
      next = (index - 1 + segments.length) % segments.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = segments.length - 1;
    if (next === null) return;
    event.preventDefault();
    const target = segments[next].id;
    onChange(target);
    refs.current[target]?.focus();
  };

  return (
    <div
      role="tablist"
      aria-label={label}
      className={`inline-flex rounded-[9px] bg-chip p-[3px] ${className}`}
    >
      {segments.map((segment) => {
        const selected = segment.id === value;
        return (
          <button
            key={segment.id}
            ref={(el) => {
              refs.current[segment.id] = el;
            }}
            type="button"
            role="tab"
            aria-selected={selected}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(segment.id)}
            onKeyDown={handleKeyDown}
            className={`min-w-24 cursor-pointer rounded-[7px] px-3.5 py-1 text-[13px] font-medium transition-all focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50 ${
              selected
                ? "bg-surface text-text shadow-[0_1px_2px_rgb(0_0_0/0.12),0_0_0_0.5px_rgb(0_0_0/0.04)]"
                : "text-sub hover:text-text"
            }`}
          >
            {segment.label}
          </button>
        );
      })}
    </div>
  );
}
