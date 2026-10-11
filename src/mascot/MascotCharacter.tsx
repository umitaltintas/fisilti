import React from "react";

export type MascotMood =
  | "idle"
  | "curious"
  | "listening"
  | "thinking"
  | "happy"
  | "dragging";

interface Props {
  mood: MascotMood;
  /** A meeting is being recorded: shows the red "on air" dot. */
  meetingLive: boolean;
}

/**
 * Fısıltı's mascot: a small violet whisper-ghost. Pure SVG + CSS; every
 * animation is keyed off `data-mood` in `Mascot.css`.
 */
export const MascotCharacter: React.FC<Props> = ({ mood, meetingLive }) => {
  const happy = mood === "happy";
  return (
    <div className="mascot-figure" data-mood={mood}>
      <svg viewBox="0 0 72 72" width="72" height="72" aria-hidden="true">
        <defs>
          <linearGradient id="mascot-body" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stopColor="#a78bfa" />
            <stop offset="1" stopColor="#6d28d9" />
          </linearGradient>
          <radialGradient id="mascot-shine" cx="0.35" cy="0.25" r="0.5">
            <stop offset="0" stopColor="#ffffff" stopOpacity="0.55" />
            <stop offset="1" stopColor="#ffffff" stopOpacity="0" />
          </radialGradient>
        </defs>

        {/* Ground shadow */}
        <ellipse className="mascot-shadow" cx="36" cy="68" rx="15" ry="2.5" />

        {/* Sound waves while listening */}
        <g className="mascot-waves" fill="none" strokeLinecap="round">
          <path d="M8 30 q-4 6 0 12" />
          <path d="M3 26 q-6 10 0 20" />
          <path d="M64 30 q4 6 0 12" />
          <path d="M69 26 q6 10 0 20" />
        </g>

        <g className="mascot-body">
          {/* Ghost body with a wavy hem */}
          <path
            d="M14 36 C14 20 24 10 36 10 C48 10 58 20 58 36 L58 56
               q-3.6 5 -7.3 0 q-3.6 -5 -7.3 0 q-3.6 5 -7.3 0
               q-3.6 -5 -7.3 0 q-3.6 5 -7.3 0 q-3.6 -5 -7.4 0 Z"
            fill="url(#mascot-body)"
            stroke="#4c1d95"
            strokeWidth="1.5"
            strokeLinejoin="round"
          />
          <path
            d="M14 36 C14 20 24 10 36 10 C48 10 58 20 58 36 L58 44 L14 44 Z"
            fill="url(#mascot-shine)"
          />

          {/* Cheeks */}
          <ellipse
            cx="22"
            cy="40"
            rx="3.5"
            ry="2"
            fill="#f9a8d4"
            opacity="0.7"
          />
          <ellipse
            cx="50"
            cy="40"
            rx="3.5"
            ry="2"
            fill="#f9a8d4"
            opacity="0.7"
          />

          {/* Eyes */}
          {happy ? (
            <g
              fill="none"
              stroke="#1e1b4b"
              strokeWidth="2.4"
              strokeLinecap="round"
            >
              <path d="M24 33 q4 -5 8 0" />
              <path d="M40 33 q4 -5 8 0" />
            </g>
          ) : (
            <g className="mascot-eyes">
              <ellipse cx="28" cy="32" rx="4.2" ry="5" fill="#fff" />
              <ellipse cx="44" cy="32" rx="4.2" ry="5" fill="#fff" />
              <g className="mascot-pupils" fill="#1e1b4b">
                <circle cx="28.6" cy="33" r="2.4" />
                <circle cx="44.6" cy="33" r="2.4" />
                <circle cx="29.6" cy="31.8" r="0.8" fill="#fff" />
                <circle cx="45.6" cy="31.8" r="0.8" fill="#fff" />
              </g>
            </g>
          )}

          {/* Mouth: a smile, or an "o" while listening */}
          <path
            className="mascot-smile"
            d="M32 42 q4 3.5 8 0"
            fill="none"
            stroke="#1e1b4b"
            strokeWidth="2"
            strokeLinecap="round"
          />
          <ellipse
            className="mascot-mouth-open"
            cx="36"
            cy="43"
            rx="2.6"
            ry="3"
            fill="#1e1b4b"
          />
        </g>

        {/* Thinking dots */}
        <g className="mascot-dots" fill="#c4b5fd">
          <circle cx="48" cy="6" r="2.2" />
          <circle cx="56" cy="6" r="2.2" />
          <circle cx="64" cy="6" r="2.2" />
        </g>

        {meetingLive && (
          <circle
            className="mascot-live"
            cx="58"
            cy="14"
            r="4.5"
            fill="#e5484d"
            stroke="#fff"
            strokeWidth="1.5"
          />
        )}
      </svg>
    </div>
  );
};
