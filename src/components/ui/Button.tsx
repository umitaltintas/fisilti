import React from "react";

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?:
    | "primary"
    | "primary-soft"
    | "secondary"
    | "danger"
    | "danger-ghost"
    | "ghost";
  size?: "sm" | "md" | "lg";
}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  (
    {
      children,
      className = "",
      variant = "primary",
      size = "md",
      type = "button",
      ...props
    },
    ref,
  ) => {
    const baseClasses =
      "inline-flex items-center justify-center gap-1.5 font-medium rounded-lg border focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50 transition-colors disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer";

    const variantClasses = {
      primary:
        "text-white bg-background-ui border-transparent hover:brightness-110",
      "primary-soft":
        "text-brand-text bg-brand-soft border-transparent hover:bg-logo-primary/20",
      secondary: "text-text bg-chip border-transparent hover:bg-mid-gray/20",
      danger: "text-white bg-rec border-transparent hover:brightness-95",
      "danger-ghost":
        "text-rec border-transparent hover:bg-rec/10 focus:bg-rec/15",
      ghost: "text-current border-transparent hover:bg-chip",
    };

    const sizeClasses = {
      sm: "px-2.5 py-1 text-xs",
      md: "px-3.5 py-[5px] text-[13px]",
      lg: "px-4 py-2 text-sm",
    };

    return (
      <button
        ref={ref}
        type={type}
        className={`${baseClasses} ${variantClasses[variant]} ${sizeClasses[size]} ${className}`}
        {...props}
      >
        {children}
      </button>
    );
  },
);

Button.displayName = "Button";
