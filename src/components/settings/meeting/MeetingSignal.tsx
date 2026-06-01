import React, { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";

import {
  listenMeetingAudioLevel,
  type MeetingAudioLevel,
} from "@/lib/meeting";

const BAR_COUNT = 16;
const WAVE_BUFFER = 480; // rolling oscilloscope sample buffer (~5 ticks of 96)

// Read a CSS custom property off the document root so the canvas follows
// handy's light/dark theme (canvas can't use Tailwind classes).
function cssVar(name: string, fallback: string): string {
  if (typeof window === "undefined") return fallback;
  const v = getComputedStyle(document.documentElement)
    .getPropertyValue(name)
    .trim();
  return v || fallback;
}

interface MeetingSignalProps {
  /** Only subscribes/animates while true; resets to flat when it goes false. */
  active: boolean;
}

/**
 * Live audio signal visualizer for an in-progress meeting.
 *
 * TOP: a scrolling oscilloscope rendered on a <canvas>. Each incoming event
 * carries 96 `wave` samples (-1..1) which we append to a rolling ring buffer;
 * a requestAnimationFrame loop redraws the trace every frame, smoothing the
 * drawn amplitude toward the latest samples (lerp) so the line glides rather
 * than snaps. The canvas is sized to its container and scaled by
 * devicePixelRatio so the trace stays crisp on retina displays.
 *
 * BOTTOM: 16 level bars driven by `bars` (0..1). These mirror the
 * RecordingOverlay smoothing approach -- each frame the rendered height lerps
 * toward the latest target -- and reuse its bar look (rounded, min height,
 * amplitude-based opacity).
 */
export const MeetingSignal: React.FC<MeetingSignalProps> = ({ active }) => {
  const { t } = useTranslation();

  const canvasRef = useRef<HTMLCanvasElement>(null);
  const barsWrapRef = useRef<HTMLDivElement>(null);
  const barRefs = useRef<(HTMLDivElement | null)[]>([]);

  // Latest targets coming off the event stream.
  const targetBarsRef = useRef<number[]>(new Array(BAR_COUNT).fill(0));
  // Smoothed values the rAF loop renders.
  const smoothedBarsRef = useRef<number[]>(new Array(BAR_COUNT).fill(0));
  // Rolling oscilloscope buffer of raw samples (-1..1), oldest first.
  const waveBufRef = useRef<Float32Array>(new Float32Array(WAVE_BUFFER));
  // Smoothed copy of the buffer that the rAF loop draws.
  const drawnWaveRef = useRef<Float32Array>(new Float32Array(WAVE_BUFFER));

  // Subscribe to the backend audio-level event while active. Appends each
  // tick's wave samples to the rolling buffer and stores the latest bars.
  useEffect(() => {
    if (!active) {
      // Reset to flat so a stopped meeting shows a clean baseline.
      targetBarsRef.current.fill(0);
      smoothedBarsRef.current.fill(0);
      waveBufRef.current.fill(0);
      drawnWaveRef.current.fill(0);
      return;
    }

    let unlisten: (() => void) | undefined;
    let cancelled = false;

    const onLevel = (lvl: MeetingAudioLevel) => {
      const bars = lvl.bars ?? [];
      for (let i = 0; i < BAR_COUNT; i++) {
        targetBarsRef.current[i] = bars[i] ?? 0;
      }
      const wave = lvl.wave ?? [];
      if (wave.length > 0) {
        const buf = waveBufRef.current;
        const n = wave.length;
        // Shift the buffer left by n and append the new samples at the tail.
        buf.copyWithin(0, n);
        const start = buf.length - n;
        for (let i = 0; i < n; i++) {
          buf[start + i] = wave[i] ?? 0;
        }
      }
    };

    listenMeetingAudioLevel(onLevel)
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {
        // Listener registration is best-effort; the canvas just stays flat.
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [active]);

  // rAF render loop: smooths bars + wave toward their targets and paints.
  useEffect(() => {
    let raf = 0;

    const render = () => {
      // --- Level bars: lerp rendered height toward latest target. ---
      const smoothed = smoothedBarsRef.current;
      const targets = targetBarsRef.current;
      for (let i = 0; i < BAR_COUNT; i++) {
        // Same blend ratio as RecordingOverlay (prev*0.7 + target*0.3).
        smoothed[i] = smoothed[i] * 0.7 + targets[i] * 0.3;
        const el = barRefs.current[i];
        if (el) {
          const v = smoothed[i];
          // Mirror RecordingOverlay's height/opacity curve, scaled taller.
          el.style.height = `${Math.min(36, 3 + Math.pow(v, 0.7) * 33)}px`;
          el.style.opacity = `${Math.max(0.2, Math.min(1, v * 1.7))}`;
        }
      }

      // --- Oscilloscope: lerp drawn samples toward the rolling buffer. ---
      const buf = waveBufRef.current;
      const drawn = drawnWaveRef.current;
      for (let i = 0; i < drawn.length; i++) {
        drawn[i] += (buf[i] - drawn[i]) * 0.35;
      }
      drawScope();

      raf = requestAnimationFrame(render);
    };

    const drawScope = () => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      const dpr = window.devicePixelRatio || 1;
      const cssW = canvas.clientWidth;
      const cssH = canvas.clientHeight;
      if (cssW === 0 || cssH === 0) return;

      // Resize backing store to match the container * dpr for crisp lines.
      const wantW = Math.round(cssW * dpr);
      const wantH = Math.round(cssH * dpr);
      if (canvas.width !== wantW || canvas.height !== wantH) {
        canvas.width = wantW;
        canvas.height = wantH;
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, cssW, cssH);

      const mid = cssH / 2;
      const grid = cssVar("--color-mid-gray", "#808080");
      const trace = cssVar("--color-logo-primary", "#6d28d9");

      // Zero baseline.
      ctx.strokeStyle = grid;
      ctx.globalAlpha = 0.25;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, mid);
      ctx.lineTo(cssW, mid);
      ctx.stroke();

      // Waveform trace.
      const data = drawnWaveRef.current;
      const len = data.length;
      const amp = mid * 0.9;
      ctx.globalAlpha = 1;
      ctx.strokeStyle = trace;
      ctx.lineWidth = 1.5;
      ctx.lineJoin = "round";
      ctx.lineCap = "round";
      ctx.beginPath();
      for (let i = 0; i < len; i++) {
        const x = (i / (len - 1)) * cssW;
        const y = mid - data[i] * amp;
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.stroke();
    };

    raf = requestAnimationFrame(render);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <div className="space-y-2">
      <h3 className="text-[11px] font-medium text-mid-gray uppercase tracking-wide">
        {t("meeting.signal")}
      </h3>
      <div className="rounded-md border border-mid-gray/20 bg-background/60 p-3 space-y-3">
        <canvas
          ref={canvasRef}
          className="block w-full h-16 rounded-sm"
          aria-hidden
        />
        <div
          ref={barsWrapRef}
          className="flex items-end justify-center gap-[3px] h-9"
          aria-hidden
        >
          {Array.from({ length: BAR_COUNT }).map((_, i) => (
            <div
              key={i}
              ref={(el) => {
                barRefs.current[i] = el;
              }}
              className="w-1.5 rounded-[2px] bg-logo-primary"
              style={{ height: "3px", minHeight: "3px" }}
            />
          ))}
        </div>
      </div>
    </div>
  );
};

export default MeetingSignal;
