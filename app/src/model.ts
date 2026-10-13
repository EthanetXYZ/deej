// Display logic shared by the views: names, grouping, and which sliders control what.

import type { AppState, Config, Session } from "./ipc";

export const SPECIAL_TARGETS = [
  { value: "master", label: "Master volume", description: "The default playback device" },
  { value: "mic", label: "Microphone", description: "The default recording device" },
  { value: "system", label: "System sounds", description: "Windows notification sounds" },
  { value: "deej.current", label: "Focused app", description: "Whichever app is in the foreground" },
  { value: "deej.unmapped", label: "All other apps", description: "Every app no slider names" },
] as const;

/** How a config target reads in the UI. */
export function targetLabel(target: string, audio: AppState["audio"]): string {
  const lower = target.toLowerCase();
  const special = SPECIAL_TARGETS.find((s) => s.value === lower);
  if (special) return special.label;

  const session = audio.sessions.find((s) => s.key === lower);
  if (session) return appName(session);

  const endpoint = audio.endpoints.find((e) => e.name.toLowerCase() === lower);
  if (endpoint) return endpoint.name;

  return target;
}

export function appName(session: Pick<Session, "displayName" | "processName" | "isSystem">): string {
  if (session.isSystem) return "System sounds";
  // display names like "@%SystemRoot%\..." are resource references, not readable names
  if (session.displayName && !session.displayName.startsWith("@")) return session.displayName;
  const base = session.processName.replace(/\.exe$/i, "");
  return base.charAt(0).toUpperCase() + base.slice(1);
}

/** All sessions of one app (same executable), shown as one mixer row. */
export interface AppGroup {
  key: string;
  name: string;
  sessions: Session[];
  exePath: string | null;
  active: boolean;
  volume: number;
  muted: boolean;
  endpointIds: string[];
}

export function appGroups(sessions: Session[]): AppGroup[] {
  const groups = new Map<string, Session[]>();
  for (const s of sessions) groups.set(s.key, [...(groups.get(s.key) ?? []), s]);

  return [...groups.entries()]
    .map(([key, list]) => {
      const lead = list.find((s) => s.state === "active") ?? list[0];
      return {
        key,
        name: appName(lead),
        sessions: list,
        exePath: list.find((s) => s.exePath)?.exePath ?? null,
        active: list.some((s) => s.state === "active"),
        volume: lead.volume,
        muted: list.every((s) => s.muted),
        endpointIds: [...new Set(list.map((s) => s.endpointId))],
      };
    })
    .sort((a, b) => Number(b.active) - Number(a.active) || Number(a.key === "system") - Number(b.key === "system") || a.name.localeCompare(b.name));
}

/** Every slider worth showing: the ones the board reports plus the ones the config maps. */
export function sliderIndexes(app: AppState): number[] {
  const indexes = new Set<number>(app.positions.map((_, i) => i));
  Object.keys(app.config.slider_mapping).forEach((k) => indexes.add(Number(k)));
  return [...indexes].sort((a, b) => a - b);
}

export function effectiveSensitivity(config: Config, slider: number | null) {
  const d = config.slider_sensitivity.default;
  const base = { curve: d.curve ?? 1, maxVolume: Math.round((d.max_volume ?? 1) * 100) };
  if (slider === null) return { ...base, usesDefault: true };

  const own = config.slider_sensitivity.sliders[String(slider)];
  return {
    curve: own?.curve ?? base.curve,
    maxVolume: own?.max_volume != null ? Math.round(own.max_volume * 100) : base.maxVolume,
    usesDefault: !own || (own.curve === null && own.max_volume === null),
  };
}

/** Volume a slider position produces with a given curve: maxVolume% × position^curve. */
export const curveVolume = (position: number, curve: number, maxVolume: number) => (maxVolume / 100) * Math.pow(Math.max(0, Math.min(1, position)), curve);

export const HIGHLIGHT_MS = 900;
