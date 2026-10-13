// A stand-in backend for developing the UI in a browser (npm run dev). Never part of a release build: ipc.ts only
// imports it when not running inside the app, in development mode. It keeps state in memory, applies commands to
// it, and simulates a board whose sliders drift so the live parts of the UI can be seen.

import type { AppState, AudioSnapshot, Config, CurvePreset, HardwareInfo, Session, UiEvent } from "./ipc";

const session = (id: number, endpointId: string, pid: number, processName: string, volume: number, state: Session["state"] = "active"): Session => ({
  id,
  endpointId,
  pid,
  processName,
  key: pid === 0 ? "system" : processName.toLowerCase(),
  displayName: "",
  exePath: pid === 0 ? null : `C:\\Programs\\${processName}`,
  isSystem: pid === 0,
  state,
  volume,
  muted: false,
});

const audio: AudioSnapshot = {
  available: true,
  endpoints: [
    { id: "headset", name: "Headset Earphone (CORSAIR VIRTUOSO SE Wireless Gaming Headset)", description: "Headset Earphone", flow: "render", isDefault: true, volume: 0.52, muted: false },
    { id: "speakers", name: "Speakers (Realtek(R) Audio)", description: "Speakers", flow: "render", isDefault: false, volume: 1, muted: false },
    { id: "monitor", name: "Odyssey G50A (NVIDIA High Definition Audio)", description: "Odyssey G50A", flow: "render", isDefault: false, volume: 1, muted: false },
    { id: "mic", name: "Headset Microphone (CORSAIR VIRTUOSO SE Wireless Gaming Headset)", description: "Headset Microphone", flow: "capture", isDefault: true, volume: 1, muted: false },
  ],
  sessions: [
    session(1, "headset", 0, "", 1, "inactive"),
    session(2, "headset", 33572, "Discord.exe", 0.83),
    session(3, "headset", 13892, "brave.exe", 0.42),
    session(4, "headset", 43024, "mpvnet.exe", 0.98),
    session(5, "headset", 38956, "steam.exe", 0.04, "inactive"),
    session(6, "headset", 45088, "javaw.exe", 0.04),
    session(7, "speakers", 51000, "Spotify.exe", 0.6),
  ],
};

const config: Config = {
  slider_mapping: { "0": ["deej.unmapped"], "1": ["firefox.exe", "brave.exe"], "2": ["discord.exe"] },
  invert_sliders: false,
  com_port: "COM3",
  baud_rate: 9600,
  noise_reduction: "low",
  slider_sensitivity: { default: { curve: 2, max_volume: null }, sliders: { "0": { curve: 2, max_volume: 1 }, "1": { curve: 2, max_volume: 1 } } },
  unmapped_exclude: ["mpvnet.exe"],
  next: { serial: { dtr: true, rts: false, reconnect_interval_ms: 2000 } },
};

const presets: CurvePreset[] = [
  { key: "linear", name: "Linear (original deej)", value: 1, description: "Volume follows the slider exactly - 10% slider = 10% volume" },
  { key: "gentle", name: "Gentle", value: 1.5, description: "Slightly finer control at low volumes - 10% slider = ~3% volume" },
  { key: "balanced", name: "Balanced", value: 2, description: "Recommended - 10% slider = 1% volume, 50% slider = 25% volume" },
  { key: "fine", name: "Fine", value: 2.5, description: "Very fine control at low volumes - 50% slider = ~18% volume" },
  { key: "very_fine", name: "Very fine", value: 3, description: "Maximum precision at low volumes - 50% slider = ~13% volume" },
];

export function mockBackend() {
  const state: AppState = {
    config,
    configError: null,
    diagnostics: [{ severity: "info", key: "process_refresh_frequency", line: 31, message: "is from an older deej version and no longer has any effect" }],
    audio,
    positions: [0.19, 0.65, 0.91],
    hardware: { port: "COM3", baudRate: 9600, state: "connected" },
    sliderTargets: [],
  };
  const autostart = { enabled: false, viaShortcut: false, others: ["Startup shortcut “deej.exe.lnk”: C:\\deej\\deej.exe"] };
  const listeners = new Set<(e: UiEvent) => void>();
  const emit = (e: UiEvent) => listeners.forEach((l) => l(e));

  const resolve = () => {
    const mapped = new Set(Object.values(state.config.slider_mapping).flat().map((t) => t.toLowerCase()).filter((t) => !t.startsWith("deej.")));
    state.sliderTargets = Object.entries(state.config.slider_mapping).map(([slider, targets]) => {
      const sessions = new Set<number>();
      const endpoints = new Set<string>();
      for (const raw of targets) {
        const t = raw.toLowerCase();
        if (t === "master") endpoints.add(state.audio.endpoints.find((e) => e.flow === "render" && e.isDefault)!.id);
        else if (t === "mic") endpoints.add(state.audio.endpoints.find((e) => e.flow === "capture" && e.isDefault)!.id);
        else if (t === "deej.unmapped")
          state.audio.sessions.filter((s) => !s.isSystem && !mapped.has(s.key) && !state.config.unmapped_exclude.includes(s.key)).forEach((s) => sessions.add(s.id));
        else {
          state.audio.sessions.filter((s) => s.key === t).forEach((s) => sessions.add(s.id));
          state.audio.endpoints.filter((e) => e.name.toLowerCase() === t).forEach((e) => endpoints.add(e.id));
        }
      }
      return { slider: Number(slider), sessions: [...sessions], endpoints: [...endpoints], followsFocus: targets.includes("deej.current") };
    });
  };
  resolve();

  const changed = () => {
    resolve();
    emit({ type: "configChanged" });
    setTimeout(() => emit({ type: "saved" }), 400);
  };

  // a board with three slightly noisy sliders; slider 1 sweeps slowly so the live views have something to show
  let t = 0;
  setInterval(() => {
    t += 1;
    const sweep = 0.5 + 0.45 * Math.sin(t / 20);
    state.positions = [0.19 + (t % 7 === 0 ? 0.02 : 0), Math.round(sweep * 100) / 100, 0.91];
    emit({ type: "sliders", positions: state.positions });
  }, 120);

  const handlers: Record<string, (args: any) => unknown> = {
    get_state: () => structuredClone(state),
    get_hardware_info: (): HardwareInfo => ({
      diagnostics: {
        bytes: 812_345,
        validLines: 58_120,
        invalidLines: 3,
        unconfirmedLines: 1,
        connects: 1,
        sliderCount: 3,
        lastError: null,
        recentLines: ["193|666|931", "194|666|930", "193|667|931"],
        msSinceLastLine: 12,
      },
      ports: [
        { name: "COM1", usb: null, manufacturer: null, product: null },
        { name: "COM3", usb: [0x2341, 0x0043], manufacturer: "Microsoft", product: "USB Serial Device (COM3)" },
      ],
    }),
    get_curve_presets: () => presets,
    get_config_status: () => ({ path: "C:\\deej\\config.yaml", locationKind: "executableDir", createdDefault: false, error: null }),
    get_app_info: () => ({ version: "0.1.0 (dev, mock)", buildType: "dev" }),
    get_app_icon: () => null,
    set_slider_targets: ({ slider, targets }: { slider: number; targets: string[] }) => {
      if (targets.length) state.config.slider_mapping[String(slider)] = targets;
      else delete state.config.slider_mapping[String(slider)];
      changed();
    },
    set_sensitivity: ({ slider, curve, maxVolume, clear }: { slider: number | null; curve: number | null; maxVolume: number | null; clear: boolean }) => {
      const s = state.config.slider_sensitivity;
      if (slider === null) {
        if (curve !== null) s.default.curve = curve;
        if (maxVolume !== null) s.default.max_volume = maxVolume / 100;
      } else if (clear) {
        delete s.sliders[String(slider)];
      } else {
        const o = (s.sliders[String(slider)] ??= { curve: null, max_volume: null });
        if (curve !== null) o.curve = curve;
        if (maxVolume !== null) o.max_volume = maxVolume / 100;
      }
      changed();
    },
    set_setting: ({ key, value }: { key: string; value: never }) => {
      const c = state.config as unknown as Record<string, unknown>;
      if (key.startsWith("deej_next.serial.")) (state.config.next.serial as Record<string, unknown>)[key.split(".")[2]] = value;
      else c[key] = value;
      changed();
    },
    set_unmapped_exclude: ({ names }: { names: string[] }) => {
      state.config.unmapped_exclude = names;
      changed();
    },
    rescan: () => {},
    reconnect_hardware: () => {
      emit({ type: "hardware", status: { port: "COM3", baudRate: 9600, state: "connecting" } });
      setTimeout(() => emit({ type: "hardware", status: { port: "COM3", baudRate: 9600, state: "connected" } }), 1500);
    },
    open_config_file: () => {},
    open_logs_folder: () => {},
    get_autostart: () => ({ ...autostart }),
    set_autostart: ({ enabled }: { enabled: boolean }) => {
      autostart.enabled = enabled;
      return { ...autostart };
    },
    open_startup_folder: () => {},
    get_ui_prefs: () => JSON.parse(localStorage.getItem("deej-mock-prefs") ?? '{"theme":"dark"}'),
    set_ui_prefs: ({ prefs }: { prefs: object }) =>
      localStorage.setItem("deej-mock-prefs", JSON.stringify({ ...JSON.parse(localStorage.getItem("deej-mock-prefs") ?? "{}"), ...prefs })),
  };

  return {
    invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      const handler = handlers[command];
      if (!handler) throw new Error(`mock: no handler for ${command}`);
      (window as unknown as { __calls?: unknown[] }).__calls?.push([command, args]);
      return handler(args ?? {}) as T;
    },
    listen: async (handler: (e: UiEvent) => void) => {
      listeners.add(handler);
      return () => listeners.delete(handler);
    },
  };
}
