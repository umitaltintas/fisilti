import { useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";

import "./SubtitleOverlay.css";

/** Mirrors Rust `SubtitleUpdate`. */
interface SubtitleUpdate {
  /** Finalized transcript the model will not revise. */
  settled: string;
  /** Speculative hypothesis for the utterance still being spoken. */
  pending: string;
}

// A subtitle strip: finalized text plus the sentence currently being spoken.
//
// The two are styled differently on purpose. The pending line is a guess that
// the model rewrites as the speaker continues, so it is dimmed — otherwise
// words appear to flicker and correct themselves for no visible reason, which
// reads as a bug rather than as speech in progress.
//
// The window itself is click-through and excluded from screen capture; this
// component only decides what the strip says.
export default function SubtitleOverlay() {
  const [update, setUpdate] = useState<SubtitleUpdate>({
    settled: "",
    pending: "",
  });

  useEffect(() => {
    const unlisten = listen<SubtitleUpdate>("subtitle-update", (event) => {
      setUpdate(event.payload);
    });
    // Tell the backend the page is alive. A transparent window that renders
    // nothing looks identical whether the page crashed or simply has no text
    // yet, so this is the only signal that distinguishes them in the log.
    void emit("subtitle-ready");
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

  const hasText =
    update.settled.trim().length > 0 || update.pending.trim().length > 0;
  if (!hasText) return null;

  return (
    <div className="subtitle-strip">
      <p className="subtitle-text">
        {update.settled && (
          <span className="subtitle-settled">{update.settled}</span>
        )}
        {update.settled && update.pending ? " " : null}
        {update.pending && (
          <span className="subtitle-pending">{update.pending}</span>
        )}
      </p>
    </div>
  );
}
