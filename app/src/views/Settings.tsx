// Settings, grouped into sections. Each control applies at once and is saved to config.yaml (or, for appearance,
// to deej-next-ui.json, which the Go deej never reads).

import { useEffect, useState } from "react";
import {
  getAppInfo,
  getAutostart,
  getUiPrefs,
  setUiPrefs,
  openStartupFolder,
  setAutostart,
  type Autostart,
  getConfigStatus,
  getCurvePresets,
  openConfigFile,
  openLogsFolder,
  rescan,
  setSetting,
  setUnmappedExclude,
  type AppInfo,
  type AppState,
  type ConfigStatus,
  type CurvePreset,
  type Theme,
} from "../ipc";
import { appGroups } from "../model";
import { toast } from "../store";
import { Badge, Button, Card, IconButton, PageHeader, SectionTitle, Segmented, SettingRow, TextInput, Toggle, cx } from "../components/ui";
import { AddIcon, BellIcon, BoardIcon, DismissIcon, DocumentIcon, FolderIcon, InfoIcon, LinkIcon, MixerIcon, PowerIcon, RefreshIcon, SettingsIcon, StackIcon, TrayIcon, WarningIcon } from "../components/icons";
import { SensitivityEditor, TargetPicker } from "./Sliders";
import { useHardwareInfo, statusText } from "./Hardware";

export const SECTIONS = ["General", "Hardware", "Audio", "Slider mappings", "Appearance", "Advanced", "Diagnostics"] as const;
export type Section = (typeof SECTIONS)[number];

export default function Settings({ app, section, onSection, theme, onTheme, onOpenMappings }: {
  app: AppState;
  section: Section;
  onSection: (s: Section) => void;
  theme: Theme;
  onTheme: (t: Theme) => void;
  onOpenMappings: () => void;
}) {
  return (
    <>
      <PageHeader title="Settings" />
      <div className="mb-4 flex flex-wrap gap-1" role="tablist" aria-label="Settings sections">
        {SECTIONS.map((s) => (
          <button
            key={s}
            role="tab"
            aria-selected={section === s}
            onClick={() => onSection(s)}
            className={cx("h-8 rounded-[4px] px-3 text-[14px]", section === s ? "bg-control text-text font-semibold" : "text-text-muted hover:bg-control hover:text-text")}
          >
            {s}
          </button>
        ))}
      </div>
      {section === "General" && <General />}
      {section === "Hardware" && <HardwareSettings app={app} />}
      {section === "Audio" && <AudioSettings app={app} />}
      {section === "Slider mappings" && <MappingSettings app={app} onOpenMappings={onOpenMappings} />}
      {section === "Appearance" && <Appearance theme={theme} onTheme={onTheme} />}
      {section === "Advanced" && <Advanced app={app} />}
      {section === "Diagnostics" && <Diagnostics app={app} />}
    </>
  );
}

function useConfigStatus() {
  const [status, setStatus] = useState<ConfigStatus | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    getConfigStatus().then(setStatus).catch(() => {});
    getAppInfo().then(setInfo).catch(() => {});
  }, []);
  return { status, info };
}

function useAutostart() {
  const [autostart, setState] = useState<Autostart | null>(null);
  useEffect(() => {
    getAutostart().then(setState).catch(() => {});
  }, []);
  const set = (enabled: boolean) => {
    setState((a) => a && { ...a, enabled });
    setAutostart(enabled)
      .then(setState)
      .catch((e) => {
        toast("error", `Couldn't change Start with Windows: ${e}`);
        getAutostart().then(setState).catch(() => {});
      });
  };
  return [autostart, set] as const;
}

function useQuietNotifications() {
  const [quiet, setQuiet] = useState(false);
  useEffect(() => {
    getUiPrefs()
      .then((p) => setQuiet(p.quietNotifications ?? false))
      .catch(() => {});
  }, []);
  const set = (value: boolean) => {
    setQuiet(value);
    setUiPrefs({ quietNotifications: value }).catch((e) => toast("error", `Couldn't save: ${e}`));
  };
  return [quiet, set] as const;
}

function General() {
  const { status, info } = useConfigStatus();
  const [autostart, setAutostartEnabled] = useAutostart();
  const [quiet, setQuiet] = useQuietNotifications();

  return (
    <Card>
      <SettingRow
        icon={<PowerIcon />}
        label="Start with Windows"
        description={
          autostart?.viaShortcut
            ? "Started by a shortcut in your Startup folder. Turning this off deletes that shortcut."
            : "Starts in the tray when you sign in, so the sliders work right away."
        }
      >
        <Toggle label="Start with Windows" checked={autostart?.enabled ?? false} disabled={!autostart} onChange={setAutostartEnabled} />
      </SettingRow>
      {autostart && autostart.others.length > 0 && (
        <div className="border-border flex gap-3 border-b px-4 py-3">
          <WarningIcon className="text-warning shrink-0" />
          <div className="min-w-0 flex-1">
            <div className="font-semibold">Another deej also starts with Windows</div>
            <div className="text-text-muted">Two copies can't share the board. Remove the one you no longer use.</div>
            <ul className="text-text-subtle mt-1 text-[12px] break-all">
              {autostart.others.map((o) => (
                <li key={o}>{o}</li>
              ))}
            </ul>
          </div>
          <Button className="self-center" onClick={() => openStartupFolder()}>
            Open Startup folder
          </Button>
        </div>
      )}
      <SettingRow
        icon={<TrayIcon />}
        label="Closing the window"
        description="deej keeps running in the tray, so the sliders keep working. Right-click the tray icon and choose Quit deej to stop it."
      />
      <SettingRow
        icon={<BellIcon />}
        label="Routine notifications"
        description="Windows notifications for good news too: board reconnected, config.yaml reloaded. Problems are always shown."
      >
        <Toggle label="Routine notifications" checked={!quiet} onChange={(v) => setQuiet(!v)} />
      </SettingRow>
      <SettingRow icon={<DocumentIcon />} label="Configuration file" description={status?.path ?? "…"}>
        <Button onClick={() => openConfigFile()}>Open in Notepad</Button>
      </SettingRow>
      <SettingRow icon={<RefreshIcon />} label="Re-scan audio" description="Find newly opened apps and devices now, and re-apply every slider.">
        <Button onClick={() => rescan()}>Re-scan</Button>
      </SettingRow>
      <SettingRow icon={<InfoIcon />} label="Version" description={info ? `deej ${info.version}` : "…"} />
    </Card>
  );
}

function HardwareSettings({ app }: { app: AppState }) {
  const c = app.config;
  return (
    <Card>
      <SettingRow icon={<LinkIcon />} label="Serial port" description={`Currently ${c.com_port} at ${c.baud_rate} baud. Change it on the Hardware page.`} />
      <SettingRow icon={<BoardIcon />} label="Invert sliders" description="Turn on if your sliders are upside down (top is 0%).">
        <Toggle label="Invert sliders" checked={c.invert_sliders} onChange={(v) => setSetting("invert_sliders", v)} />
      </SettingRow>
      <SettingRow
        icon={<MixerIcon />}
        label="Noise reduction"
        description="How far a slider must move before the volume changes: Low 2%, Default 3%, High 4%. Raise it if volumes drift while untouched."
      >
        <Segmented
          label="Noise reduction"
          value={c.noise_reduction}
          onChange={(v) => setSetting("noise_reduction", v)}
          options={[
            { value: "low", label: "Low" },
            { value: "default", label: "Default" },
            { value: "high", label: "High" },
          ]}
        />
      </SettingRow>
    </Card>
  );
}

function AudioSettings({ app }: { app: AppState }) {
  const exclude = app.config.unmapped_exclude;
  const [picking, setPicking] = useState(false);
  const names = new Map(appGroups(app.audio.sessions).map((g) => [g.key, g.name]));

  return (
    <>
      <Card>
        <SettingRow
          icon={<StackIcon />}
          label="Leave these apps alone"
          description="A slider set to “All other apps” (deej.unmapped) controls every app no other slider names, except these."
        >
          <Button onClick={() => setPicking((p) => !p)}>
            <AddIcon size={16} /> Add app
          </Button>
        </SettingRow>
        <div className="flex flex-wrap gap-2 px-4 py-3">
          {exclude.length === 0 && <span className="text-text-subtle">None</span>}
          {exclude.map((name) => (
            <span key={name} className="bg-control border-border-strong inline-flex h-8 items-center gap-2 rounded-full border pr-1 pl-3">
              {names.get(name) ?? name}
              <IconButton label={`Stop leaving ${name} alone`} className="h-6 w-6" onClick={() => setUnmappedExclude(exclude.filter((n) => n !== name))}>
                <DismissIcon size={14} />
              </IconButton>
            </span>
          ))}
        </div>
        {picking && (
          <div className="border-border border-t px-4 py-3">
            <TargetPicker
              app={app}
              appsOnly
              exclude={exclude}
              onPick={(name) => {
                setUnmappedExclude([...exclude, name]);
                setPicking(false);
              }}
              onClose={() => setPicking(false)}
            />
          </div>
        )}
      </Card>
    </>
  );
}

function MappingSettings({ app, onOpenMappings }: { app: AppState; onOpenMappings: () => void }) {
  const [presets, setPresets] = useState<CurvePreset[]>([]);
  useEffect(() => {
    getCurvePresets().then(setPresets).catch(() => {});
  }, []);

  return (
    <>
      <Card>
        <SettingRow icon={<MixerIcon />} label="Slider mappings" description="Assign apps, devices and special targets to each slider.">
          <Button onClick={onOpenMappings}>Open sliders</Button>
        </SettingRow>
      </Card>
      <SectionTitle>Default response</SectionTitle>
      <Card className="p-4">
        <p className="text-text-muted mb-3">
          How slider position turns into volume for every slider that doesn't have its own setting. A curve above 1 gives finer control at low
          volumes.
        </p>
        <SensitivityEditor app={app} slider={null} presets={presets} />
      </Card>
    </>
  );
}

function Appearance({ theme, onTheme }: { theme: Theme; onTheme: (t: Theme) => void }) {
  return (
    <Card>
      <SettingRow icon={<SettingsIcon />} label="Theme" description="Dark, light, or follow Windows.">
        <Segmented
          label="Theme"
          value={theme}
          onChange={onTheme}
          options={[
            { value: "dark", label: "Dark" },
            { value: "light", label: "Light" },
            { value: "system", label: "Windows setting" },
          ]}
        />
      </SettingRow>
    </Card>
  );
}

function Advanced({ app }: { app: AppState }) {
  const s = app.config.next.serial;
  const [interval, setIntervalText] = useState(String(s.reconnect_interval_ms));
  useEffect(() => setIntervalText(String(s.reconnect_interval_ms)), [s.reconnect_interval_ms]);

  return (
    <>
      <Card>
        <SettingRow label="Assert DTR when connecting" description="On for most boards. Native-USB boards (Pro Micro, many RP2040/ESP32) may send nothing without it. On an Uno or Nano it resets the board.">
          <Toggle label="Assert DTR" checked={s.dtr} onChange={(v) => setSetting("deej_next.serial.dtr", v)} />
        </SettingRow>
        <SettingRow label="Assert RTS when connecting" description="Off for most boards. Some ESP32 boards use it together with DTR to reset.">
          <Toggle label="Assert RTS" checked={s.rts} onChange={(v) => setSetting("deej_next.serial.rts", v)} />
        </SettingRow>
        <SettingRow label="Reconnect interval" description="How often to retry when the board is missing (250–60000 ms).">
          <TextInput
            aria-label="Reconnect interval in milliseconds"
            className="w-24 text-right"
            inputMode="numeric"
            value={interval}
            onChange={(e) => setIntervalText(e.target.value)}
            onBlur={() => {
              const v = Number(interval);
              if (Number.isInteger(v) && v >= 250 && v <= 60000) setSetting("deej_next.serial.reconnect_interval_ms", v);
              else setIntervalText(String(s.reconnect_interval_ms));
            }}
          />
          <span className="text-text-muted">ms</span>
        </SettingRow>
      </Card>
      <p className="text-text-subtle mt-2 text-[12px]">These are stored under deej_next: in config.yaml. The original deej ignores them.</p>
    </>
  );
}

function Diagnostics({ app }: { app: AppState }) {
  const { status, info } = useConfigStatus();
  const hw = useHardwareInfo(2000);
  const hardware = statusText(app.hardware);

  return (
    <>
      <Card>
        <SettingRow icon={<DocumentIcon />} label="Configuration" description={status?.path}>
          {app.configError ? <Badge tone="danger">Error</Badge> : <Badge tone="success">Loaded</Badge>}
        </SettingRow>
        <SettingRow icon={<BoardIcon />} label="Board" description={hw?.diagnostics ? `${hw.diagnostics.validLines.toLocaleString()} readings, ${hw.diagnostics.connects} connection(s)` : undefined}>
          <Badge tone={hardware.tone}>{hardware.text}</Badge>
        </SettingRow>
        <SettingRow icon={<MixerIcon />} label="Audio" description={`${app.audio.endpoints.length} devices, ${app.audio.sessions.length} app sessions`}>
          {app.audio.available ? <Badge tone="success">Available</Badge> : <Badge tone="warning">Unavailable</Badge>}
        </SettingRow>
        <SettingRow icon={<FolderIcon />} label="Logs" description="deej-latest-run.log, deej-previous-run.log, crash reports and config backups.">
          <Button onClick={() => openLogsFolder()}>Open folder</Button>
        </SettingRow>
        <SettingRow icon={<InfoIcon />} label="Version" description={info?.version} />
      </Card>

      <SectionTitle>Notes about config.yaml</SectionTitle>
      <Card>
        {app.configError && (
          <div className="border-border flex gap-3 border-b px-4 py-3">
            <WarningIcon className="text-danger shrink-0" />
            <div>
              <div className="font-semibold">The file can't be used; the previous settings are active</div>
              <div className="text-text-muted">{app.configError}</div>
            </div>
          </div>
        )}
        {app.diagnostics.length === 0 && !app.configError && <p className="text-text-muted px-4 py-3">Nothing to report. Every setting was understood.</p>}
        {app.diagnostics.map((d, i) => (
          <div key={i} className="border-border flex gap-3 border-b px-4 py-3 last:border-b-0">
            {d.severity === "warning" ? <WarningIcon className="text-warning shrink-0" /> : <InfoIcon className="text-text-muted shrink-0" />}
            <div className="min-w-0">
              <code className="font-mono text-[13px]">{d.key}</code> <span className="text-text-muted">{d.message}</span>
              {d.line !== null && <span className="text-text-subtle"> (line {d.line})</span>}
            </div>
          </div>
        ))}
      </Card>
    </>
  );
}
