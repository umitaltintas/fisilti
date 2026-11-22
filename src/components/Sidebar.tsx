import { useTranslation } from "react-i18next";
import React, { useEffect, useState } from "react";
import {
  History,
  Loader2,
  Radio,
  Settings,
  type LucideIcon,
} from "lucide-react";
import { useMeetingStore } from "@/stores/meetingStore";
import FisiltiMark from "./icons/FisiltiMark";
import UpdateChecker from "./update-checker";
import { formatElapsed } from "./settings/meeting/shared";

/** The app's three places: two kinds of content and the settings. */
export type AppSection = "meetings" | "dictation" | "settings";

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

const SidebarItem: React.FC<{
  icon: LucideIcon;
  label: string;
  active: boolean;
  onClick: () => void;
  children?: React.ReactNode;
}> = ({ icon: Icon, label, active, onClick, children }) => (
  <button
    type="button"
    aria-current={active ? "page" : undefined}
    onClick={onClick}
    className={`flex w-full cursor-pointer items-center gap-2.5 rounded-[7px] px-2.5 py-[6px] text-start text-[13px] transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50 ${
      active ? "bg-mid-gray/20 font-medium" : "hover:bg-mid-gray/10"
    }`}
  >
    <Icon
      width={15}
      height={15}
      className={`shrink-0 ${active ? "text-logo-primary" : "text-sub"}`}
    />
    <span className="truncate">{label}</span>
    {children}
  </button>
);

interface SidebarProps {
  activeSection: AppSection;
  onSectionChange: (section: AppSection) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
}) => {
  const { t } = useTranslation();

  return (
    <nav
      aria-label={t("sidebar.navigation")}
      className="flex h-full w-[196px] shrink-0 flex-col border-e border-line bg-sidebar px-2.5 pt-4 pb-2.5"
    >
      <div className="flex items-center gap-2 px-2 pb-5">
        <span className="grid h-[22px] w-[22px] place-items-center rounded-[6px] bg-gradient-to-br from-logo-primary to-violet-400 text-white shadow-[inset_0_0_0_0.5px_rgb(255_255_255/0.25)]">
          <FisiltiMark width={13} height={13} className="!text-white" />
        </span>
        {/* eslint-disable-next-line i18next/no-literal-string -- brand name */}
        <span className="text-[15px] font-semibold tracking-tight">
          Fısıltı
        </span>
      </div>
      <div className="flex flex-col gap-0.5">
        <SidebarItem
          icon={Radio}
          label={t("sidebar.meeting")}
          active={activeSection === "meetings"}
          onClick={() => onSectionChange("meetings")}
        >
          <MeetingActivityIndicator />
        </SidebarItem>
        <SidebarItem
          icon={History}
          label={t("sidebar.history")}
          active={activeSection === "dictation"}
          onClick={() => onSectionChange("dictation")}
        />
      </div>
      <div className="mt-auto space-y-1.5 pt-3">
        <UpdateChecker quiet className="px-2 text-[11px]" />
        <SidebarItem
          icon={Settings}
          label={t("sidebar.settings")}
          active={activeSection === "settings"}
          onClick={() => onSectionChange("settings")}
        />
      </div>
    </nav>
  );
};
