import React from "react";
import { useTranslation } from "react-i18next";
import {
  Cog,
  History,
  Info,
  Loader2,
  Sparkles,
  Cpu,
  Radio,
} from "lucide-react";
import { useMeetingStore } from "@/stores/meetingStore";
import FisiltiWordmark from "./icons/FisiltiWordmark";
import FisiltiMark from "./icons/FisiltiMark";
import {
  GeneralSettings,
  AdvancedSettings,
  HistorySettings,
  AboutSettings,
  PostProcessingSettings,
  ModelsSettings,
  MeetingSettings,
} from "./settings";

export type SidebarSection = keyof typeof SECTIONS_CONFIG;

interface IconProps {
  width?: number | string;
  height?: number | string;
  size?: number | string;
  className?: string;
  [key: string]: unknown;
}

/** Which cluster of the sidebar a section belongs to. */
type SectionGroup = "main" | "features" | "system";

interface SectionConfig {
  labelKey: string;
  icon: React.ComponentType<IconProps>;
  component: React.ComponentType;
  group: SectionGroup;
}

// Every section is always visible: sections that appear and disappear based on
// a toggle buried on another page were a big part of "where do I configure
// this?". Feature switches now live at the top of their own page instead.
export const SECTIONS_CONFIG = {
  general: {
    labelKey: "sidebar.general",
    icon: FisiltiMark,
    component: GeneralSettings,
    group: "main",
  },
  models: {
    labelKey: "sidebar.models",
    icon: Cpu,
    component: ModelsSettings,
    group: "main",
  },
  postprocessing: {
    labelKey: "sidebar.postProcessing",
    icon: Sparkles,
    component: PostProcessingSettings,
    group: "features",
  },
  meeting: {
    labelKey: "sidebar.meeting",
    icon: Radio,
    component: MeetingSettings,
    group: "features",
  },
  history: {
    labelKey: "sidebar.history",
    icon: History,
    component: HistorySettings,
    group: "features",
  },
  advanced: {
    labelKey: "sidebar.advanced",
    icon: Cog,
    component: AdvancedSettings,
    group: "system",
  },
  about: {
    labelKey: "sidebar.about",
    icon: Info,
    component: AboutSettings,
    group: "system",
  },
} as const satisfies Record<string, SectionConfig>;

const GROUP_ORDER: SectionGroup[] = ["main", "features", "system"];

// The first cluster carries the app wordmark, so it needs no extra heading.
const GROUP_LABEL_KEYS: Record<SectionGroup, string | null> = {
  main: null,
  features: "sidebar.groups.features",
  system: "sidebar.groups.system",
};

/** Live-meeting marker on the Meetings entry, visible from every page: a
 * pulsing red dot while recording, a spinner while the stop is finalizing. */
const MeetingActivityIndicator: React.FC = () => {
  const { t } = useTranslation();
  const status = useMeetingStore((s) => s.status);
  if (status === "running") {
    return (
      <span className="ms-auto flex items-center">
        <span className="relative flex h-2 w-2" aria-hidden>
          <span className="absolute inline-flex h-full w-full rounded-full bg-red-500/70 animate-ping" />
          <span className="relative inline-flex h-2 w-2 rounded-full bg-red-500" />
        </span>
        <span className="sr-only">{t("meeting.recording")}</span>
      </span>
    );
  }
  if (status === "finalizing") {
    return (
      <span className="ms-auto flex items-center">
        <Loader2 width={13} height={13} className="animate-spin" aria-hidden />
        <span className="sr-only">{t("meeting.finalizing")}</span>
      </span>
    );
  }
  return null;
};

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
}) => {
  const { t } = useTranslation();

  const sections = Object.entries(SECTIONS_CONFIG).map(([id, config]) => ({
    id: id as SidebarSection,
    ...config,
  }));

  return (
    <nav
      aria-label={t("sidebar.navigation")}
      className="flex h-full w-44 flex-col items-center border-e border-mid-gray/20 px-2"
    >
      <FisiltiWordmark width={120} className="m-4" />
      <div className="flex w-full flex-col gap-4 border-t border-mid-gray/20 pt-3">
        {GROUP_ORDER.map((group) => {
          const items = sections.filter((section) => section.group === group);
          if (items.length === 0) return null;
          const labelKey = GROUP_LABEL_KEYS[group];

          return (
            <div key={group} className="flex w-full flex-col gap-1">
              {labelKey && (
                <p className="px-2 pb-0.5 text-[10px] font-medium uppercase tracking-wider text-mid-gray/70">
                  {t(labelKey)}
                </p>
              )}
              {items.map((section) => {
                const Icon = section.icon;
                const isActive = activeSection === section.id;

                return (
                  <button
                    key={section.id}
                    type="button"
                    aria-current={isActive ? "page" : undefined}
                    className={`flex w-full cursor-pointer items-center gap-2 rounded-lg p-2 text-start transition-colors ${
                      isActive
                        ? "bg-logo-primary/80"
                        : "opacity-85 hover:bg-mid-gray/20 hover:opacity-100"
                    }`}
                    onClick={() => onSectionChange(section.id)}
                  >
                    <Icon
                      width={20}
                      height={20}
                      className="shrink-0"
                      aria-hidden
                    />
                    <span
                      className="truncate text-sm font-medium"
                      title={t(section.labelKey)}
                    >
                      {t(section.labelKey)}
                    </span>
                    {section.id === "meeting" && <MeetingActivityIndicator />}
                  </button>
                );
              })}
            </div>
          );
        })}
      </div>
    </nav>
  );
};
