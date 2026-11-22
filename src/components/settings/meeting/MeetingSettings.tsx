import React, { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Mic } from "lucide-react";

import { LiveSession } from "./LiveSession";
import { MeetingHistory } from "./MeetingHistory";
import { MeetingDetail } from "./MeetingDetail";
import { ImportRecording } from "./ImportRecording";
import { Button } from "../../ui/Button";
import { summaryProviderInfo } from "@/lib/meeting";
import { useModelStore } from "@/stores/modelStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { isLiveMeeting, useMeetingStore } from "@/stores/meetingStore";
import { isCloudModel, meetingModelId } from "@/lib/utils/model";

// The Meetings page, one view instead of tabs: the list of meetings with a
// Start button, the live workspace while recording, and a meeting's detail
// when one is opened. When a meeting finishes, its detail opens by itself.
//
// The session lives in the app-wide meeting store, so this page can unmount
// without losing a running meeting's transcript, notes or status.
export const MeetingsPage: React.FC = () => {
  const { t } = useTranslation();
  const [detailId, setDetailId] = useState<number | null>(null);

  const status = useMeetingStore((s) => s.status);
  const busy = useMeetingStore((s) => s.busy);
  const start = useMeetingStore((s) => s.start);
  const error = useMeetingStore((s) => s.error);
  const interrupted = useMeetingStore((s) => s.interrupted);
  const liveMeetingId = useMeetingStore((s) => s.meetingId);
  const isActive = status !== "idle";

  const settings = useSettingsStore((s) => s.settings);
  const models = useModelStore((s) => s.models);

  // Cloud meeting models have no live per-segment pass; the workspace says so
  // instead of "listening…". Read from the MEETING model, not dictation's.
  const selectedIsCloud = isCloudModel(
    models.find((m) => m.id === meetingModelId(settings)),
  );
  const templates = useMemo(
    () => settings?.meeting_summary_templates ?? [],
    [settings?.meeting_summary_templates],
  );
  const providerInfo = useMemo(() => summaryProviderInfo(settings), [settings]);

  const openDetail = (id: number) => {
    // The live meeting is shown by the workspace, not as a saved record.
    if (isLiveMeeting(id, { status, meetingId: liveMeetingId })) {
      setDetailId(null);
      return;
    }
    setDetailId(id);
  };

  // Starting a meeting always shows the live workspace.
  useEffect(() => {
    if (status === "running") setDetailId(null);
  }, [status]);

  // Stop → finalize → idle: open the meeting that just ended, where its
  // transcript, notes and (auto) summary are.
  const previousStatus = useRef(status);
  useEffect(() => {
    const was = previousStatus.current;
    previousStatus.current = status;
    if (was !== "idle" && status === "idle" && liveMeetingId != null) {
      setDetailId(liveMeetingId);
    }
  }, [status, liveMeetingId]);

  // The tray / mascot / another page asks for a meeting to be opened.
  const openRequest = useMeetingStore((s) => s.openRequest);
  const clearOpenRequest = useMeetingStore((s) => s.clearOpenRequest);
  useEffect(() => {
    if (openRequest === null) return;
    clearOpenRequest();
    openDetail(openRequest);
    // openDetail only reads the current status; the request is the trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openRequest, clearOpenRequest]);

  if (detailId !== null) {
    return (
      <div className="mx-auto w-full max-w-3xl">
        <MeetingDetail
          key={detailId}
          meetingId={detailId}
          templates={templates}
          providerInfo={providerInfo}
          onBack={() => setDetailId(null)}
        />
      </div>
    );
  }

  return (
    <div className="mx-auto w-full max-w-3xl space-y-5">
      <header className="flex items-center justify-between gap-3">
        <h1 className="text-[26px] leading-tight font-bold tracking-tight">
          {t("sidebar.meeting")}
        </h1>
        {!isActive && (
          <div className="flex items-center gap-2">
            <ImportRecording compact disabled={false} onImported={openDetail} />
            <Button
              variant="primary"
              size="md"
              disabled={busy}
              onClick={() => void start()}
            >
              {busy ? (
                <Loader2
                  width={14}
                  height={14}
                  className="animate-spin"
                  aria-hidden
                />
              ) : (
                <Mic width={14} height={14} aria-hidden />
              )}
              <span>{t("meeting.startMeeting")}</span>
            </Button>
          </div>
        )}
      </header>

      {(isActive || interrupted.length > 0) && (
        <LiveSession selectedIsCloud={selectedIsCloud} />
      )}

      {!isActive && (
        <>
          {error && <p className="px-1 text-xs text-rec">{error}</p>}
          <MeetingHistory
            onOpen={openDetail}
            onDeleted={(id) => {
              if (detailId === id) setDetailId(null);
            }}
          />
        </>
      )}
    </div>
  );
};
