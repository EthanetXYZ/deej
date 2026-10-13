// Typed access to the backend. Every command the UI calls and every event it receives goes through here.
// The shapes mirror the Rust types (serde) in app/src-tauri/src/commands.rs and coordinator.rs.

export type Flow = "render" | "capture";

export interface Endpoint {
  id: string;
  name: string;
  description: string;
  flow: Flow;
  isDefault: boolean;
  volume: number;
  muted: boolean;
}

export interface Session {
  id: number;
  endpointId: string;
  pid: number;
  processName: string;
  key: string;
  displayName: string;
  exePath: string | null;
  isSystem: boolean;
  state: "active" | "inactive";
  volume: number;
  muted: boolean;
}

export interface AudioSnapshot {
  available: boolean;
  endpoints: Endpoint[];
  sessions: Session[];
}

export type NoiseReduction = "low" | "default" | "high";

export interface SensitivityOverride {
  curve: number | null;
  max_volume: number | null;
}

export interface Config {
  slider_mapping: Record<string, string[]>;
  invert_sliders: boolean;
  com_port: string;
  baud_rate: number;
  noise_reduction: NoiseReduction;
  slider_sensitivity: { default: SensitivityOverride; sliders: Record<string, SensitivityOverride> };
  unmapped_exclude: string[];
  next: { serial: { dtr: boolean; rts: boolean; reconnect_interval_ms: number } };
}

export type ConnectionState = "connecting" | "connected" | "notFound" | "busy" | "lost" | "failed";

export interface HardwareStatus {
  port: string;
  baudRate: number;
  state: ConnectionState;
  message?: string;
}

export interface Diagnostic {
  severity: "warning" | "info";
  key: string;
  line: number | null;
  message: string;
}

export interface SliderTargets {
  slider: number;
  sessions: number[];
  endpoints: string[];
  followsFocus: boolean;
}

export interface AppState {
  config: Config;
  configError: string | null;
  diagnostics: Diagnostic[];
  audio: AudioSnapshot;
  positions: number[];
  hardware: HardwareStatus | null;
  sliderTargets: SliderTargets[];
}

export interface SerialDiagnostics {
  bytes: number;
  validLines: number;
  invalidLines: number;
  unconfirmedLines: number;
  connects: number;
  sliderCount: number;
  lastError: string | null;
  recentLines: string[];
  msSinceLastLine: number | null;
}

export interface PortInfo {
  name: string;
  usb: [number, number] | null;
  manufacturer: string | null;
  product: string | null;
}

export interface HardwareInfo {
  diagnostics: SerialDiagnostics | null;
  ports: PortInfo[];
}

export interface CurvePreset {
  key: string;
  name: string;
  value: number;
  description: string;
}

export interface ConfigStatus {
  path: string;
  locationKind: "executableDir" | "workingDir" | "appData";
  createdDefault: boolean;
  error: string | null;
}

export interface AppInfo {
  version: string;
  buildType: string;
}

export type Theme = "dark" | "light" | "system";

export interface UiPrefs {
  theme: Theme;
  /** Only problems get a Windows notification. */
  quietNotifications?: boolean;
}

export interface Autostart {
  enabled: boolean;
  /** Started by a shortcut in the Startup folder (turning it off deletes that shortcut). */
  viaShortcut: boolean;
  /** Other deej programs that also start with Windows. */
  others: string[];
}

export type UiEvent =
  | { type: "sliders"; positions: number[] }
  | { type: "hardware"; status: HardwareStatus }
  | { type: "audio"; snapshot: AudioSnapshot }
  | { type: "volume"; target: { kind: "session"; id: number } | { kind: "endpoint"; id: string }; volume: number; muted: boolean; external: boolean }
  | { type: "configReloaded"; diagnostics: Diagnostic[] }
  | { type: "configChanged" }
  | { type: "configError"; message: string }
  | { type: "saved" }
  | { type: "saveFailed"; message: string };

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = (handler: (event: UiEvent) => void) => Promise<() => void>;

interface Backend {
  invoke: Invoke;
  listen: Listen;
}

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// outside the app (npm run dev in a browser) a mock backend stands in, so the UI can be developed and checked
// visually; it's only ever loaded in development builds
let backend: Promise<Backend> | null = null;

function getBackend(): Promise<Backend> {
  backend ??= (async () => {
    if (inTauri) {
      const [{ invoke }, { listen }] = await Promise.all([import("@tauri-apps/api/core"), import("@tauri-apps/api/event")]);
      return {
        invoke: (command, args) => invoke(command, args),
        listen: (handler) => listen<UiEvent>("deej://event", (e) => handler(e.payload)),
      };
    }
    if (import.meta.env.DEV) {
      return (await import("./mock")).mockBackend();
    }
    throw new Error("deej's backend isn't available");
  })();
  return backend;
}

const call = async <T>(command: string, args?: Record<string, unknown>) => (await getBackend()).invoke<T>(command, args);

export const onEvent = async (handler: (event: UiEvent) => void) => (await getBackend()).listen(handler);

export const getState = () => call<AppState>("get_state");
export const getHardwareInfo = () => call<HardwareInfo>("get_hardware_info");
export const getCurvePresets = () => call<CurvePreset[]>("get_curve_presets");
export const getConfigStatus = () => call<ConfigStatus>("get_config_status");
export const getAppInfo = () => call<AppInfo>("get_app_info");
export const getAppIcon = (path: string) => call<string | null>("get_app_icon", { path });

export const setSliderTargets = (slider: number, targets: string[]) => call<void>("set_slider_targets", { slider, targets });
export const setSensitivity = (
  slider: number | null,
  change: { curve?: number; maxVolume?: number; clear?: boolean },
) =>
  call<void>("set_sensitivity", {
    slider,
    curve: change.curve ?? null,
    maxVolume: change.maxVolume ?? null,
    clear: change.clear ?? false,
  });
export const setSetting = (key: string, value: boolean | number | string) => call<void>("set_setting", { key, value });
export const setUnmappedExclude = (names: string[]) => call<void>("set_unmapped_exclude", { names });

export const rescan = () => call<void>("rescan");
export const reconnectHardware = () => call<void>("reconnect_hardware");
export const openConfigFile = () => call<void>("open_config_file");
export const openLogsFolder = () => call<void>("open_logs_folder");

export const getAutostart = () => call<Autostart>("get_autostart");
export const setAutostart = (enabled: boolean) => call<Autostart>("set_autostart", { enabled });
export const openStartupFolder = () => call<void>("open_startup_folder");

export const getUiPrefs = () => call<UiPrefs>("get_ui_prefs");
export const setUiPrefs = (prefs: Partial<UiPrefs>) => call<void>("set_ui_prefs", { prefs });

export const reportUiError = (message: string) => call<void>("report_ui_error", { message }).catch(() => {});
export const reportUiReady = (page: string, endpoints: number, appRows: number, sliders: number) =>
  call<void>("report_ui_ready", { page, endpoints, appRows, sliders }).catch(() => {});
