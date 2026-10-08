import React, { useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { History, Mic, Settings2 } from "lucide-react";

import { LiveSession } from "./LiveSession";
import { MeetingHistory } from "./MeetingHistory";
import { MeetingDetail } from "./MeetingDetail";
import { MeetingPreferences } from "./MeetingPreferences";
import { ImportRecording } from "./ImportRecording";
import { summaryProviderInfo } from "@/lib/meeting";
import { useModelStore } from "@/stores/modelStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { isLiveMeeting, useMeetingStore } from "@/stores/meetingStore";
import { isCloudModel, meetingModelId } from "@/lib/utils/model";

type MeetingTab = "session" | "history" | "settings";
const TAB_ORDER: MeetingTab[] = ["session", "history", "settings"];

// The Meeting section, split into three tabs so each job gets its own space:
// "Session" (the live/last workspace), "History" (past meetings + detail) and
// "Settings" (one-time configuration).
//
// The session itself lives in the app-wide meeting store, not here: this page
// can unmount (navigating to another section) without losing a running
// meeting's transcript, notes or status.
export const MeetingSettings: React.FC = () => {
  const { t } = useTranslation();
  const tabsId = useId();
  const tabRefs = useRef<Record<MeetingTab, HTMLButtonElement | null>>({
    session: null,
    history: null,
    settings: null,
  });

  const [tab, setTab] = useState<MeetingTab>("session");
  const [detailId, setDetailId] = useState<number | null>(null);

  const status = useMeetingStore((s) => s.status);
  const liveMeetingId = useMeetingStore((s) => s.meetingId);
  const isActive = status !== "idle";

  const settings = useSettingsStore((s) => s.settings);
  const models = useModelStore((s) => s.models);

  // Cloud models skip the live per-segment pass (it would be one request each),
  // so the live transcript stays empty until the on-stop finalize. Detect that
  // from the MEETING model — not the dictation one — to show an accurate hint
  // instead of "listening…".
  const selectedIsCloud = isCloudModel(
    models.find((m) => m.id === meetingModelId(settings)),
  );
  const templates = useMemo(
    () => settings?.meeting_summary_templates ?? [],
    [settings?.meeting_summary_templates],
  );
  const providerInfo = useMemo(() => summaryProviderInfo(settings), [settings]);

  const openDetail = (id: number) => {
    // The live meeting is shown by the Session tab, not as a saved record.
    if (isLiveMeeting(id, { status, meetingId: liveMeetingId })) {
      setTab("session");
      return;
    }
    setDetailId(id);
    setTab("history");
  };

  const goToHistoryList = () => {
    setDetailId(null);
    setTab("history");
  };

  // Re-clicking the active History tab pops back from a detail view.
  const selectTab = (next: MeetingTab) => {
    if (next === "history" && tab === "history") setDetailId(null);
    setTab(next);
  };

  // WAI-ARIA tabs: arrows move between tabs (and select them), Home/End jump.
  const handleTabKeyDown = (event: React.KeyboardEvent) => {
    const index = TAB_ORDER.indexOf(tab);
    const rtl = document.dir === "rtl";
    let next: number | null = null;
    if (event.key === (rtl ? "ArrowLeft" : "ArrowRight"))
      next = (index + 1) % TAB_ORDER.length;
    else if (event.key === (rtl ? "ArrowRight" : "ArrowLeft"))
      next = (index - 1 + TAB_ORDER.length) % TAB_ORDER.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = TAB_ORDER.length - 1;
    if (next === null) return;
    event.preventDefault();
    const target = TAB_ORDER[next];
    setTab(target);
    tabRefs.current[target]?.focus();
  };

  const tabs: { id: MeetingTab; label: string; Icon: typeof Mic }[] = [
    { id: "session", label: t("meeting.tabSession"), Icon: Mic },
    { id: "history", label: t("meeting.tabHistory"), Icon: History },
    { id: "settings", label: t("meeting.tabSettings"), Icon: Settings2 },
  ];

  const tabId = (id: MeetingTab) => `${tabsId}-tab-${id}`;
  const panelId = (id: MeetingTab) => `${tabsId}-panel-${id}`;

  const importCard = (
    <ImportRecording disabled={isActive} onImported={openDetail} />
  );

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div
        role="tablist"
        aria-label={t("sidebar.meeting")}
        className="flex items-center gap-1 rounded-lg border border-mid-gray/20 bg-mid-gray/5 p-1"
      >
        {tabs.map(({ id, label, Icon }) => {
          const selected = tab === id;
          return (
            <button
              key={id}
              ref={(el) => {
                tabRefs.current[id] = el;
              }}
              type="button"
              role="tab"
              id={tabId(id)}
              aria-selected={selected}
              aria-controls={panelId(id)}
              tabIndex={selected ? 0 : -1}
              onClick={() => selectTab(id)}
              onKeyDown={handleTabKeyDown}
              className={`flex-1 flex items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors cursor-pointer focus:outline-none focus-visible:ring-1 focus-visible:ring-logo-primary ${
                selected
                  ? "bg-background text-text border border-mid-gray/20 shadow-sm"
                  : "border border-transparent text-text/60 hover:text-text"
              }`}
            >
              <Icon width={15} height={15} aria-hidden />
              <span>{label}</span>
              {id === "session" && status === "running" && (
                <span className="relative flex h-2 w-2" aria-hidden>
                  <span className="absolute inline-flex h-full w-full rounded-full bg-red-500/70 animate-ping" />
                  <span className="relative inline-flex h-2 w-2 rounded-full bg-red-500" />
                </span>
              )}
            </button>
          );
        })}
      </div>

      <div role="tabpanel" id={panelId(tab)} aria-labelledby={tabId(tab)}>
        {tab === "session" && (
          <LiveSession
            selectedIsCloud={selectedIsCloud}
            templates={templates}
            providerInfo={providerInfo}
            onOpenMeeting={openDetail}
            onViewAllMeetings={goToHistoryList}
            importSlot={importCard}
          />
        )}

        {tab === "history" &&
          (detailId !== null ? (
            <MeetingDetail
              key={detailId}
              meetingId={detailId}
              templates={templates}
              providerInfo={providerInfo}
              onBack={() => setDetailId(null)}
            />
          ) : (
            <div className="space-y-4">
              {importCard}
              <MeetingHistory
                onOpen={openDetail}
                onDeleted={(id) => {
                  if (detailId === id) setDetailId(null);
                }}
              />
            </div>
          ))}

        {tab === "settings" && <MeetingPreferences />}
      </div>
    </div>
  );
};
