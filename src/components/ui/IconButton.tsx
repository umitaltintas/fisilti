import React from "react";

interface IconButtonProps
  extends Omit<
    React.ButtonHTMLAttributes<HTMLButtonElement>,
    "aria-label" | "title" | "children"
  > {
  /** Accessible name. Required: an icon alone says nothing to a screen
   * reader. Also used as the hover tooltip. */
  label: string;
  /** The icon. Rendered `aria-hidden`; the label carries the meaning. */
  children: React.ReactNode;
  /** `danger` turns red on hover (delete); `active` stays highlighted
   * (e.g. a starred entry). */
  tone?: "default" | "danger" | "active";
  size?: "sm" | "md";
}

/**
 * The one icon-only button. Every toolbar glyph (copy, delete, rename, star…)
 * goes through here so it always has an accessible name and the same focus
 * ring, hit area and disabled look.
 */
export const IconButton = React.forwardRef<HTMLButtonElement, IconButtonProps>(
  (
    {
      label,
      children,
      tone = "default",
      size = "md",
      className = "",
      type = "button",
      ...props
    },
    ref,
  ) => {
    const toneClasses = {
      default: "text-sub hover:text-logo-primary",
      danger: "text-sub hover:text-rec",
      active: "text-logo-primary hover:text-logo-primary/80",
    }[tone];
    const sizeClasses = size === "sm" ? "p-1" : "p-1.5";

    return (
      <button
        ref={ref}
        type={type}
        aria-label={label}
        title={label}
        className={`${sizeClasses} rounded-md inline-flex items-center justify-center transition-colors cursor-pointer focus:outline-none focus-visible:ring-1 focus-visible:ring-logo-primary disabled:cursor-not-allowed disabled:text-text/20 ${toneClasses} ${className}`}
        {...props}
      >
        <span aria-hidden className="inline-flex">
          {children}
        </span>
      </button>
    );
  },
);

IconButton.displayName = "IconButton";
