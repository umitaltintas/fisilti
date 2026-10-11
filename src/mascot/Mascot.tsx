import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  AppWindow,
  ClipboardCopy,
  EyeOff,
  History,
  Mic,
  Square,
  Users,
  X,
  type LucideIcon,
} from "lucide-react";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type MascotAction, type MascotLayout } from "@/bindings";
import { syncLanguageFromSettings } from "@/i18n";
import { MEETING_EVENTS } from "@/lib/meeting";
import { MascotCharacter, type MascotMood } from "./MascotCharacter";

/** Mirrors `DICTATION_STATE_CHANGED_EVENT` in `mascot.rs`. */
const DICTATION_STATE_EVENT = "dictation-state-changed";

type DictationState = "idle" | "recording" | "transcribing";
type MeetingState = "idle" | "running" | "finalizing";

/** Pointer travel (px) after which a press becomes a drag, not a click. */
const DRAG_THRESHOLD = 4;
/** The menu closes this long after the pointer leaves the window... */
const LEAVE_CLOSE_MS = 1500;
/** ...and this long after the last interaction in any case, because a
 * non-activating panel never gets a blur when the user clicks elsewhere. */
const IDLE_CLOSE_MS = 10000;
/** How long the happy face shows after an action. */
const CHEER_MS = 1200;

interface MenuItem {
  action: MascotAction;
  label: string;
  icon: LucideIcon;
  disabled?: boolean;
  tone?: "danger";
}

const Mascot: React.FC = () => {
  const { t } = useTranslation();
  const [dictation, setDictation] = useState<DictationState>("idle");
  const [meeting, setMeeting] = useState<MeetingState>("idle");
  const [meetingSupported, setMeetingSupported] = useState(false);
  const [layout, setLayout] = useState<MascotLayout | null>(null);
  const [dragging, setDragging] = useState(false);
  const [cheering, setCheering] = useState(false);

  const press = useRef<{ x: number; y: number; dragged: boolean } | null>(null);
  const closeTimer = useRef<number | undefined>(undefined);
  const cheerTimer = useRef<number | undefined>(undefined);
  const menuOpen = layout !== null;

  useEffect(() => {
    void commands.getMascotStatus().then((status) => {
      setDictation(status.dictation as DictationState);
      setMeeting(status.meeting as MeetingState);
      setMeetingSupported(status.meeting_supported);
    });
    const unlisteners = [
      listen<DictationState>(DICTATION_STATE_EVENT, (e) =>
        setDictation(e.payload),
      ),
      listen<MeetingState>(MEETING_EVENTS.state, (e) => setMeeting(e.payload)),
    ];
    return () => {
      unlisteners.forEach((p) => void p.then((unlisten) => unlisten()));
    };
  }, []);

  const setMenu = useCallback(async (open: boolean) => {
    window.clearTimeout(closeTimer.current);
    if (open) await syncLanguageFromSettings();
    const result = await commands.setMascotMenuOpen(open);
    if (result.status === "error") {
      console.error("Mascot menu toggle failed:", result.error);
      setLayout(null);
      return;
    }
    setLayout(open ? result.data : null);
    if (open) {
      closeTimer.current = window.setTimeout(
        () => void setMenu(false),
        IDLE_CLOSE_MS,
      );
    }
  }, []);

  /** Restart the close countdown; `delay` is shorter once the pointer left. */
  const armClose = (delay: number) => {
    if (!menuOpen) return;
    window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => void setMenu(false), delay);
  };

  useEffect(
    () => () => {
      window.clearTimeout(closeTimer.current);
      window.clearTimeout(cheerTimer.current);
    },
    [],
  );

  const cheer = () => {
    setCheering(true);
    window.clearTimeout(cheerTimer.current);
    cheerTimer.current = window.setTimeout(() => setCheering(false), CHEER_MS);
  };

  const runAction = async (action: MascotAction) => {
    await setMenu(false);
    const result = await commands.mascotAction(action);
    if (result.status === "error") {
      console.error(`Mascot action "${action}" failed:`, result.error);
      return;
    }
    cheer();
  };

  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    press.current = { x: e.screenX, y: e.screenY, dragged: false };
  };

  const onPointerMove = (e: React.PointerEvent) => {
    const p = press.current;
    if (!p || p.dragged || (e.buttons & 1) === 0) return;
    if (Math.hypot(e.screenX - p.x, e.screenY - p.y) < DRAG_THRESHOLD) return;
    p.dragged = true;
    setDragging(true);
    // Hand the drag to the OS; it moves the window natively and swallows the
    // matching pointerup, so the drag ends when the promise settles.
    void getCurrentWindow()
      .startDragging()
      .finally(() => {
        setDragging(false);
        press.current = null;
      });
  };

  const onPointerUp = () => {
    const p = press.current;
    press.current = null;
    if (!p || p.dragged) return;
    void setMenu(!menuOpen);
  };

  const dictationBusy = dictation === "transcribing";
  const items: MenuItem[] = [
    dictationBusy
      ? { action: "cancel", label: t("mascot.cancel"), icon: X }
      : {
          action: "toggle_dictation",
          label:
            dictation === "recording"
              ? t("tray.stopDictation")
              : t("tray.startDictation"),
          icon: dictation === "recording" ? Square : Mic,
        },
    ...(meetingSupported
      ? [
          {
            action: "toggle_meeting" as const,
            label:
              meeting === "running"
                ? t("tray.stopMeeting")
                : t("tray.startMeeting"),
            icon: meeting === "running" ? Square : Users,
            disabled: meeting === "finalizing",
          },
        ]
      : []),
    {
      action: "copy_last_transcript",
      label: t("tray.copyLastTranscript"),
      icon: ClipboardCopy,
    },
    { action: "open_history", label: t("mascot.history"), icon: History },
    ...(meetingSupported
      ? [
          {
            action: "open_meetings" as const,
            label: t("mascot.meetings"),
            icon: Users,
          },
        ]
      : []),
    { action: "open_app", label: t("mascot.openApp"), icon: AppWindow },
    { action: "hide", label: t("mascot.hide"), icon: EyeOff, tone: "danger" },
  ];

  let mood: MascotMood = "idle";
  if (dragging) mood = "dragging";
  else if (cheering) mood = "happy";
  else if (dictation === "recording") mood = "listening";
  else if (dictation === "transcribing" || meeting === "finalizing")
    mood = "thinking";
  else if (menuOpen) mood = "curious";

  const character = (
    <button
      type="button"
      className="mascot-hit"
      style={{ marginLeft: layout?.mascot_left ?? 0 }}
      aria-label={t("mascot.label")}
      aria-expanded={menuOpen}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      <MascotCharacter mood={mood} meetingLive={meeting === "running"} />
    </button>
  );

  const menu = menuOpen && (
    <div
      role="menu"
      className={`mascot-menu ${layout.menu_above ? "from-below" : "from-above"}`}
    >
      {items.map((item, index) => (
        <React.Fragment key={item.action}>
          {item.action === "hide" && index > 0 && (
            <div className="mascot-menu-sep" />
          )}
          <button
            type="button"
            role="menuitem"
            className={`mascot-menu-item ${item.tone === "danger" ? "danger" : ""}`}
            disabled={item.disabled}
            onClick={() => void runAction(item.action)}
          >
            <item.icon size={14} strokeWidth={2} />
            <span>{item.label}</span>
          </button>
        </React.Fragment>
      ))}
    </div>
  );

  return (
    <div
      className={`mascot-root ${layout?.menu_above === false ? "menu-below" : ""}`}
      onPointerEnter={() => armClose(IDLE_CLOSE_MS)}
      onPointerMove={() => armClose(IDLE_CLOSE_MS)}
      onPointerLeave={() => armClose(LEAVE_CLOSE_MS)}
    >
      {layout?.menu_above === false ? (
        <>
          {character}
          {menu}
        </>
      ) : (
        <>
          {menu}
          {character}
        </>
      )}
    </div>
  );
};

export default Mascot;
