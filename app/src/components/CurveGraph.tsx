// The response curve with the slider's live position riding on it, as the fork's settings window showed it:
// a dot at (position, volume) that glides towards each new reading instead of jumping.

import { useEffect, useRef, useState } from "react";
import { curveVolume } from "../model";
import { percent } from "./ui";

/** Each frame, move this fraction of the remaining way to the latest reading (the fork used the same value). */
const SMOOTHING = 0.35;

/** Follows `target` smoothly, one step per animation frame. Unknown positions (< 0) aren't smoothed. */
export function useGlide(target: number): number {
  const [shown, setShown] = useState(target);
  const current = useRef(target);

  useEffect(() => {
    if (target < 0 || current.current < 0) {
      current.current = target;
      setShown(target);
      return;
    }

    let frame = 0;
    const step = () => {
      const gap = target - current.current;
      current.current = Math.abs(gap) < 0.002 ? target : current.current + gap * SMOOTHING;
      setShown(current.current);
      if (current.current !== target) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [target]);

  return shown;
}

function curvePath(curve: number, maxVolume: number) {
  let d = "";
  for (let i = 0; i <= 50; i++) {
    const p = i / 50;
    d += `${i === 0 ? "M" : "L"}${(p * 100).toFixed(2)} ${(100 - curveVolume(p, curve, maxVolume) * 100).toFixed(2)}`;
  }
  return d;
}

/** The curve (accent), the linear reference (dashed) and, if `position` is known, the live dot. */
export function CurveGraph({ curve, maxVolume, position = -1, size = 44 }: { curve: number; maxVolume: number; position?: number; size?: number }) {
  const shown = useGlide(position);
  // the graph is drawn in a 100×100 box; keep lines a similar thickness at every size
  const scale = 100 / size;

  return (
    <svg
      width={size}
      height={size}
      viewBox="-6 -6 112 112"
      className="bg-surface border-border shrink-0 overflow-visible rounded-md border"
      role="img"
      aria-label={`Response curve ${curve}, maximum ${maxVolume}%${position >= 0 ? `, slider at ${percent(position)}` : ""}`}
    >
      <path d="M0 100 L100 0" stroke="var(--color-text-subtle)" strokeWidth={1.2 * scale} strokeDasharray={`${3 * scale} ${3 * scale}`} fill="none" />
      <path d={curvePath(curve, maxVolume)} stroke="var(--color-accent)" strokeWidth={2.4 * scale} fill="none" strokeLinecap="round" strokeLinejoin="round" />
      {shown >= 0 && (
        <circle
          cx={shown * 100}
          cy={100 - curveVolume(shown, curve, maxVolume) * 100}
          r={4 * scale}
          fill="var(--color-card)"
          stroke="var(--color-accent)"
          strokeWidth={2 * scale}
        />
      )}
    </svg>
  );
}

/** "62% → 38%" with a thin bar for each: where the slider is, and the volume that gives. */
export function LiveReadout({ position, curve, maxVolume, className }: { position: number; curve: number; maxVolume: number; className?: string }) {
  const shown = useGlide(position);
  const known = position >= 0;
  // numbers follow the real reading so they don't flicker through in-between values
  const volume = known ? curveVolume(position, curve, maxVolume) : 0;

  return (
    <div className={className} title="Slider position → the volume it sets">
      <div className="flex items-baseline gap-1 text-[12px] tabular-nums">
        {known ? (
          <>
            <span className="text-text-muted">{percent(position)}</span>
            <span className="text-text-subtle">→</span>
            <span className="font-semibold">{volume > 0 && volume < 0.1 ? `${(volume * 100).toFixed(1)}%` : percent(volume)}</span>
            <span className="text-text-subtle">volume</span>
          </>
        ) : (
          <span className="text-text-subtle">no reading</span>
        )}
      </div>
      <div className="mt-1 grid gap-0.5">
        <div className="bg-surface h-1 overflow-hidden rounded-full">
          <div className="bg-text-subtle h-full" style={{ width: `${Math.max(0, shown) * 100}%` }} />
        </div>
        <div className="bg-surface h-1 overflow-hidden rounded-full">
          <div className="bg-accent h-full" style={{ width: `${known ? curveVolume(shown, curve, maxVolume) * 100 : 0}%` }} />
        </div>
      </div>
    </div>
  );
}
