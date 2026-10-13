// Sliders: what each physical slider controls, and how it responds. This is deej's main page. Changes apply at once and are saved to
// config.yaml (comments and layout kept) about half a second later.

import { useEffect, useMemo, useRef, useState } from "react";
import { getCurvePresets, setSensitivity, setSliderTargets, type AppState, type CurvePreset } from "../ipc";
import { SPECIAL_TARGETS, appGroups, effectiveSensitivity, HIGHLIGHT_MS, sliderIndexes, targetLabel } from "../model";
import { useStore } from "../store";
import { AppImage, Button, Card, IconButton, PageHeader, Select, Slider, TextInput, Toggle, cx, percent, useLocalValue } from "../components/ui";
import { CurveGraph, LiveReadout } from "../components/CurveGraph";
import { AddIcon, ChevronDownIcon, DismissIcon, FocusIcon, MicIcon, MonitorIcon, SearchIcon, SpeakerIcon, StackIcon } from "../components/icons";

export default function Sliders({ app }: { app: AppState }) {
  const sliders = sliderIndexes(app);
  const [presets, setPresets] = useState<CurvePreset[]>([]);
  useEffect(() => {
    getCurvePresets().then(setPresets).catch(() => {});
  }, []);

  return (
    <>
      <PageHeader title="Sliders" subtitle="What each physical slider controls and how it responds. Changes are saved to config.yaml automatically." />
      {sliders.length === 0 && (
        <Card className="p-6">
          <p className="text-text-muted">No sliders yet. Connect the board, or add a mapping below.</p>
        </Card>
      )}
      <div className="flex flex-col gap-3">
        {sliders.map((i) => (
          <SliderCard key={i} app={app} slider={i} presets={presets} />
        ))}
        <AddSlider app={app} next={sliders.length ? Math.max(...sliders) + 1 : 0} />
      </div>
    </>
  );
}

function SliderCard({ app, slider, presets }: { app: AppState; slider: number; presets: CurvePreset[] }) {
  const targets = app.config.slider_mapping[String(slider)] ?? [];
  const position = app.positions[slider] ?? -1;
  const onBoard = slider < app.positions.length;
  const moved = useStore((s) => s.moved[slider] ?? -Infinity);
  const recent = performance.now() - moved < HIGHLIGHT_MS;
  const [picking, setPicking] = useState(false);
  const [showCurve, setShowCurve] = useState(false);
  const sens = effectiveSensitivity(app.config, slider);

  const update = (next: string[]) => setSliderTargets(slider, next);

  return (
    <Card className={cx("transition-colors", recent && "border-accent")}>
      <div className="flex items-center gap-4 px-4 pt-3">
        <CurveGraph curve={sens.curve} maxVolume={sens.maxVolume} position={position} size={44} />
        <div className="w-20 shrink-0">
          <div className="font-semibold">Slider {slider}</div>
          {!onBoard && <div className="text-text-subtle text-[12px]">not on board</div>}
        </div>
        <LiveReadout position={position} curve={sens.curve} maxVolume={sens.maxVolume} className="max-w-[360px] flex-1" />
      </div>

      <div className="flex flex-wrap items-center gap-2 px-4 py-3">
        {targets.length === 0 && <span className="text-text-subtle">Not controlling anything</span>}
        {targets.map((t) => (
          <TargetChip key={t} app={app} slider={slider} target={t} onRemove={() => update(targets.filter((x) => x !== t))} />
        ))}
        <Button variant="subtle" onClick={() => setPicking((p) => !p)} aria-expanded={picking}>
          <AddIcon size={16} /> Add
        </Button>
        <div className="flex-1" />
        <button type="button" onClick={() => setShowCurve((s) => !s)} className="text-text-muted hover:text-text flex items-center gap-1 text-[12px]" aria-expanded={showCurve}>
          {sens.usesDefault ? "Default response" : `Curve ${sens.curve}, max ${sens.maxVolume}%`}
          <ChevronDownIcon size={14} className={cx("transition-transform", showCurve && "rotate-180")} />
        </button>
      </div>

      {picking && (
        <div className="border-border border-t px-4 py-3">
          <TargetPicker
            app={app}
            exclude={targets}
            onPick={(t) => {
              update([...targets, t]);
              setPicking(false);
            }}
            onClose={() => setPicking(false)}
          />
        </div>
      )}

      {showCurve && (
        <div className="border-border border-t px-4 py-3">
          <SensitivityEditor app={app} slider={slider} presets={presets} />
        </div>
      )}
    </Card>
  );
}

function targetIcon(target: string) {
  switch (target.toLowerCase()) {
    case "master":
      return <SpeakerIcon size={16} />;
    case "mic":
      return <MicIcon size={16} />;
    case "system":
      return <SpeakerIcon size={16} />;
    case "deej.current":
      return <FocusIcon size={16} />;
    case "deej.unmapped":
      return <StackIcon size={16} />;
  }
  return null;
}

const levelText = (volume: number, muted: boolean) => (muted ? "muted" : percent(volume));

/** What a target is doing right now: its volume, or for groups, how many apps it covers. */
function targetStatus(app: AppState, slider: number, target: string): string | null {
  const lower = target.toLowerCase();
  const defaultOf = (flow: "render" | "capture") => app.audio.endpoints.find((e) => e.flow === flow && e.isDefault);

  switch (lower) {
    case "master":
    case "mic": {
      const e = defaultOf(lower === "master" ? "render" : "capture");
      return e ? levelText(e.volume, e.muted) : "no device";
    }
    case "deej.current":
      return "follows the focused app";
    case "deej.unmapped": {
      const resolved = app.sliderTargets.find((t) => t.slider === slider)?.sessions ?? [];
      const named = new Set((app.config.slider_mapping[String(slider)] ?? []).map((t) => t.toLowerCase()));
      const keys = new Set(app.audio.sessions.filter((s) => resolved.includes(s.id) && !named.has(s.key)).map((s) => s.key));
      return keys.size === 1 ? "1 app" : `${keys.size} apps`;
    }
  }

  const sessions = app.audio.sessions.filter((s) => s.key === lower);
  if (sessions.length) {
    const lead = sessions.find((s) => s.state === "active") ?? sessions[0];
    return levelText(lead.volume, sessions.every((s) => s.muted));
  }

  const device = app.audio.endpoints.find((e) => e.name.toLowerCase() === lower);
  return device ? levelText(device.volume, device.muted) : null;
}

function TargetChip({ app, slider, target, onRemove }: { app: AppState; slider: number; target: string; onRemove: () => void }) {
  const lower = target.toLowerCase();
  const session = app.audio.sessions.find((s) => s.key === lower && !s.isSystem);
  const device = app.audio.endpoints.find((e) => e.name.toLowerCase() === lower);
  const special = SPECIAL_TARGETS.some((s) => s.value === lower);
  const missing = !special && !session && !device && lower !== "system";
  const status = missing ? null : targetStatus(app, slider, target);

  return (
    <span
      className="bg-control border-border-strong inline-flex h-8 items-center gap-2 rounded-full border pr-1 pl-2"
      title={missing ? `${target} isn't playing audio right now` : target}
    >
      {session ? <AppImage exePath={session.exePath} size={16} /> : device ? <MonitorIcon size={16} /> : targetIcon(target)}
      <span className={cx(missing && "text-text-muted")}>{targetLabel(target, app.audio)}</span>
      {missing && <span className="text-text-subtle text-[12px]">not running</span>}
      {status && <span className="text-text-subtle text-[12px] tabular-nums">{status}</span>}
      <IconButton label={`Remove ${target}`} onClick={onRemove} className="h-6 w-6">
        <DismissIcon size={14} />
      </IconButton>
    </span>
  );
}

/** Search across special targets, running apps and devices; anything typed can be added too. */
export function TargetPicker({ app, exclude, onPick, onClose, appsOnly }: { app: AppState; exclude: string[]; onPick: (t: string) => void; onClose: () => void; appsOnly?: boolean }) {
  const [query, setQuery] = useState("");
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);

  const q = query.trim().toLowerCase();
  const excluded = new Set(exclude.map((e) => e.toLowerCase()));
  const matches = (...texts: string[]) => !q || texts.some((t) => t.toLowerCase().includes(q));

  const special = appsOnly ? [] : SPECIAL_TARGETS.filter((s) => !excluded.has(s.value) && matches(s.label, s.value, s.description));
  const apps = useMemo(() => appGroups(app.audio.sessions).filter((g) => g.key !== "system"), [app.audio.sessions]).filter(
    (g) => !excluded.has(g.key) && matches(g.name, g.key),
  );
  const devices = appsOnly ? [] : app.audio.endpoints.filter((e) => !excluded.has(e.name.toLowerCase()) && matches(e.name));
  const typed = q && !excluded.has(q) && !special.some((s) => s.value === q) && !apps.some((a) => a.key === q) ? query.trim() : null;

  return (
    <div onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="relative">
        <SearchIcon size={16} className="text-text-subtle absolute top-2 left-2" />
        <TextInput
          ref={input}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              const first = special[0]?.value ?? apps[0]?.key ?? typed;
              if (first) onPick(first);
            }
          }}
          placeholder={appsOnly ? "Search apps, or type an exe name (e.g. vlc.exe)" : "Search apps and devices, or type an exe name (e.g. vlc.exe)"}
          className="w-full pl-8"
        />
      </div>
      <div className="mt-2 max-h-72 overflow-auto">
        {typed && <PickerItem onClick={() => onPick(typed)} title={`Add “${typed}”`} detail="Matches an app with this executable name, whether or not it's running" />}
        {special.length > 0 && <PickerGroup label="Special">{special.map((s) => <PickerItem key={s.value} icon={targetIcon(s.value)} title={s.label} detail={s.description} onClick={() => onPick(s.value)} />)}</PickerGroup>}
        {apps.length > 0 && (
          <PickerGroup label="Apps with audio">
            {apps.map((g) => (
              <PickerItem key={g.key} icon={<AppImage exePath={g.exePath} size={16} />} title={g.name} detail={g.key} onClick={() => onPick(g.key)} />
            ))}
          </PickerGroup>
        )}
        {devices.length > 0 && (
          <PickerGroup label="Devices">
            {devices.map((e) => (
              <PickerItem key={e.id} icon={<MonitorIcon size={16} />} title={e.name} detail={e.flow === "capture" ? "Recording device" : "Playback device"} onClick={() => onPick(e.name)} />
            ))}
          </PickerGroup>
        )}
        {!typed && special.length + apps.length + devices.length === 0 && <p className="text-text-subtle px-2 py-3">Nothing matches.</p>}
      </div>
    </div>
  );
}

function PickerGroup({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="mt-2 first:mt-0">
      <div className="text-text-subtle px-2 pb-1 text-[12px] font-semibold">{label}</div>
      {children}
    </div>
  );
}

function PickerItem({ icon, title, detail, onClick }: { icon?: React.ReactNode; title: string; detail?: string; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="hover:bg-control flex w-full items-center gap-3 rounded-md px-2 py-1.5 text-left">
      <span className="text-text-muted flex w-4 justify-center">{icon ?? <AddIcon size={16} />}</span>
      <span className="min-w-0 flex-1">
        <span className="block truncate">{title}</span>
        {detail && <span className="text-text-subtle block truncate text-[12px]">{detail}</span>}
      </span>
    </button>
  );
}

/** Curve and maximum for one slider (or, with slider null, the default for all), with the live position on the graph. */
export function SensitivityEditor({ app, slider, presets }: { app: AppState; slider: number | null; presets: CurvePreset[] }) {
  const sens = effectiveSensitivity(app.config, slider);
  const position = slider === null ? -1 : (app.positions[slider] ?? -1);
  const [curve, changeCurve] = useLocalValue(sens.curve, (v) => setSensitivity(slider, { curve: v }));
  const [maxVolume, changeMax] = useLocalValue(sens.maxVolume, (v) => setSensitivity(slider, { maxVolume: v }));
  const preset = presets.find((p) => Math.abs(p.value - curve) < 0.001);

  return (
    <div className="flex flex-wrap items-start gap-6">
      <div className="flex w-[140px] shrink-0 flex-col gap-2">
        <CurveGraph curve={curve} maxVolume={maxVolume} position={position} size={140} />
        {slider !== null && <LiveReadout position={position} curve={curve} maxVolume={maxVolume} />}
      </div>
      <div className="flex min-w-[260px] flex-1 flex-col gap-3">
        {slider !== null && (
          <div className="flex items-center justify-between gap-4">
            <span>Use the default response</span>
            <Toggle
              label="Use the default response"
              checked={sens.usesDefault}
              onChange={(on) => setSensitivity(slider, on ? { clear: true } : { curve: sens.curve, maxVolume: sens.maxVolume })}
            />
          </div>
        )}
        <div className="flex items-center justify-between gap-4">
          <span>Preset</span>
          <Select label="Curve preset" value={preset?.key ?? "custom"} onChange={(key) => key !== "custom" && changeCurve(presets.find((p) => p.key === key)!.value)}>
            {presets.map((p) => (
              <option key={p.key} value={p.key}>
                {p.name}
              </option>
            ))}
            <option value="custom">Custom</option>
          </Select>
        </div>
        <div className="grid grid-cols-[110px_1fr_48px] items-center gap-3">
          <span>Curve</span>
          <Slider label="Curve" min={0.5} max={4} step={0.05} value={Math.min(4, Math.max(0.5, curve))} onChange={(v) => changeCurve(Math.round(v * 100) / 100)} />
          <span className="text-text-muted text-right tabular-nums">{Number(curve.toFixed(2))}</span>
          <span>Maximum volume</span>
          <Slider label="Maximum volume" min={1} max={100} step={1} value={maxVolume} onChange={(v) => changeMax(Math.round(v))} />
          <span className="text-text-muted text-right tabular-nums">{Math.round(maxVolume)}%</span>
        </div>
        <p className="text-text-subtle text-[12px] leading-4">{preset?.description ?? `volume = ${Math.round(maxVolume)}% × position^${Number(curve.toFixed(2))}`}</p>
      </div>
    </div>
  );
}

function AddSlider({ app, next }: { app: AppState; next: number }) {
  const [picking, setPicking] = useState(false);
  if (!picking)
    return (
      <Button variant="subtle" className="self-start" onClick={() => setPicking(true)}>
        <AddIcon size={16} /> Map slider {next}
      </Button>
    );
  return (
    <Card className="p-4">
      <div className="mb-2 font-semibold">Slider {next}</div>
      <TargetPicker
        app={app}
        exclude={[]}
        onPick={(t) => {
          setSliderTargets(next, [t]);
          setPicking(false);
        }}
        onClose={() => setPicking(false)}
      />
    </Card>
  );
}

