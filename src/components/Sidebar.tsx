import { useTranslation } from "react-i18next";
import React, { useEffect, useState } from "react";
import {
  Cog,
  History,
  House,
  Info,
  Loader2,
  Sparkles,
  Cpu,
  Radio,
  SlidersHorizontal,
} from "lucide-react";
import { useMeetingStore } from "@/stores/meetingStore";
import { useModelStore } from "@/stores/modelStore";
import FisiltiMark from "./icons/FisiltiMark";
import UpdateChecker from "./update-checker";
import { formatElapsed } from "./settings/meeting/shared";
import {
  HomePage,
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
  home: {
    labelKey: "sidebar.home",
    icon: House,
    component: HomePage,
    group: "main",
  },
  general: {
    labelKey: "sidebar.general",
    icon: SlidersHorizontal,
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
 * pulsing red dot and the elapsed time while recording, a spinner while the
 * stop is finalizing. */
const MeetingActivityIndicator: React.FC = () => {
  const { t } = useTranslation();
  const status = useMeetingStore((s) => s.status);
  const startedAtMs = useMeetingStore((s) => s.startedAtMs);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (status !== "running") return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [status]);

  if (status === "running") {
    return (
      <span className="ms-auto flex items-center gap-1.5 text-[11px] font-medium text-rec tabular-nums">
        <span className="rec-dot" aria-hidden />
        {startedAtMs ? formatElapsed((now - startedAtMs) / 1000) : null}
        <span className="sr-only">{t("meeting.recording")}</span>
      </span>
    );
  }
  if (status === "finalizing") {
    return (
      <span className="ms-auto flex items-center text-sub">
        <Loader2 width={13} height={13} className="animate-spin" aria-hidden />
        <span className="sr-only">{t("meeting.finalizing")}</span>
      </span>
    );
  }
  return null;
};

/** The model dictation will use, so it is never a mystery. */
const ActiveModel: React.FC<{ onOpen: () => void }> = ({ onOpen }) => {
  const { t } = useTranslation();
  const name = useModelStore(
    (s) => s.models.find((m) => m.id === s.currentModel)?.name ?? "",
  );
  if (!name) return null;
  return (
    <button
      type="button"
      onClick={onOpen}
      title={t("sidebar.models")}
      className="flex w-full cursor-pointer items-center gap-1.5 rounded-md px-2 py-1 text-start text-[11px] text-faint transition-colors hover:bg-mid-gray/10 hover:text-sub focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50"
    >
      <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-ok" aria-hidden />
      <span className="truncate">{name}</span>
    </button>
  );
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
      className="flex h-full w-[184px] shrink-0 flex-col border-e border-line bg-sidebar px-2 pt-3.5 pb-2"
    >
      <div className="flex items-center gap-2 px-2 pb-4">
        <span className="grid h-[22px] w-[22px] place-items-center rounded-[6px] bg-gradient-to-br from-logo-primary to-violet-400 text-white shadow-[inset_0_0_0_0.5px_rgb(255_255_255/0.25)]">
          <FisiltiMark width={13} height={13} className="!text-white" />
        </span>
        {/* eslint-disable-next-line i18next/no-literal-string -- brand name */}
        <span className="text-[15px] font-semibold tracking-tight">
          Fısıltı
        </span>
      </div>
      <div className="flex w-full flex-col gap-4">
        {GROUP_ORDER.map((group) => {
          const items = sections.filter((section) => section.group === group);
          if (items.length === 0) return null;
          const labelKey = GROUP_LABEL_KEYS[group];

          return (
            <div key={group} className="flex w-full flex-col gap-0.5">
              {labelKey && (
                <p className="px-2 pb-1 text-[10.5px] font-semibold tracking-wide text-faint uppercase">
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
                    className={`flex w-full cursor-pointer items-center gap-2 rounded-[7px] px-2 py-[5px] text-start text-[13px] transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50 ${
                      isActive
                        ? "bg-surface font-medium shadow-[0_1px_2px_rgb(0_0_0/0.08)]"
                        : "hover:bg-mid-gray/10"
                    }`}
                    onClick={() => onSectionChange(section.id)}
                  >
                    <Icon
                      width={14}
                      height={14}
                      className={`shrink-0 ${isActive ? "text-logo-primary" : "text-sub"}`}
                      aria-hidden
                    />
                    <span className="truncate" title={t(section.labelKey)}>
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
      <div className="mt-auto space-y-1 pt-3">
        <UpdateChecker quiet className="px-2 text-[11px]" />
        <ActiveModel onOpen={() => onSectionChange("models")} />
      </div>
    </nav>
  );
};
