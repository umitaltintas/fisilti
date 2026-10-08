import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

export interface DropdownOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface DropdownProps {
  options: DropdownOption[];
  className?: string;
  selectedValue: string | null;
  onSelect: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  onRefresh?: () => void;
}

export const Dropdown: React.FC<DropdownProps> = ({
  options,
  selectedValue,
  onSelect,
  className = "",
  placeholder = "Select an option...",
  disabled = false,
  onRefresh,
}) => {
  const { t } = useTranslation();
  const [isOpen, setIsOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  const selectedOption = options.find(
    (option) => option.value === selectedValue,
  );

  const handleSelect = (value: string) => {
    onSelect(value);
    setIsOpen(false);
  };

  const handleToggle = () => {
    if (disabled) return;
    if (!isOpen && onRefresh) onRefresh();
    setIsOpen(!isOpen);
  };

  return (
    <div className={`relative ${className}`} ref={dropdownRef}>
      <button
        type="button"
        className={`flex min-w-[180px] items-center justify-between rounded-md bg-chip py-1 ps-2.5 pe-2 text-start text-[13px] transition-colors duration-150 focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50 ${
          disabled
            ? "cursor-not-allowed opacity-50"
            : "cursor-pointer hover:bg-mid-gray/20"
        }`}
        onClick={handleToggle}
        disabled={disabled}
      >
        <span className="truncate">{selectedOption?.label || placeholder}</span>
        <svg
          className={`ms-2 h-3 w-3 shrink-0 text-sub transition-transform duration-200 ${isOpen ? "rotate-180" : ""}`}
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth={2}
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {isOpen && !disabled && (
        <div className="absolute top-full right-0 left-0 z-50 mt-1 max-h-60 overflow-y-auto rounded-lg bg-surface p-1 shadow-[0_0_0_1px_var(--color-line),0_10px_30px_-8px_rgb(0_0_0/0.25)]">
          {options.length === 0 ? (
            <div className="px-2 py-1 text-sm text-mid-gray">
              {t("common.noOptionsFound")}
            </div>
          ) : (
            options.map((option) => (
              <button
                key={option.value}
                type="button"
                className={`w-full rounded-md px-2 py-1 text-start text-[13px] transition-colors duration-150 hover:bg-chip ${
                  selectedValue === option.value
                    ? "font-medium text-brand-text"
                    : ""
                } ${option.disabled ? "cursor-not-allowed opacity-50" : ""}`}
                onClick={() => handleSelect(option.value)}
                disabled={option.disabled}
              >
                <span className="truncate">{option.label}</span>
              </button>
            ))
          )}
        </div>
      )}
    </div>
  );
};
