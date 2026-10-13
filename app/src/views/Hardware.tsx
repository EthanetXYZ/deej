// Hardware: the board's connection, port and baud rate, and diagnostics.

import { useEffect, useState } from "react";
import { getHardwareInfo, reconnectHardware, setSetting, type AppState, type HardwareInfo, type HardwareStatus } from "../ipc";
import { Badge, Button, Card, PageHeader, SectionTitle, Select, SettingRow } from "../components/ui";
import { BoardIcon, LinkIcon, RefreshIcon } from "../components/icons";

const BAUD_RATES = [9600, 19200, 38400, 57600, 115200];

export function statusText(status: HardwareStatus | null): { text: string; tone: "success" | "warning" | "danger" | "neutral" } {
  if (!status) return { text: "Starting…", tone: "neutral" };
  switch (status.state) {
    case "connected":
      return { text: `Connected on ${status.port}`, tone: "success" };
    case "connecting":
      return { text: `Connecting to ${status.port}…`, tone: "neutral" };
    case "notFound":
      return { text: `${status.port} not found`, tone: "warning" };
    case "busy":
      return { text: `${status.port} is in use by another program`, tone: "danger" };
    case "lost":
      return { text: `Lost connection to ${status.port}`, tone: "warning" };
    case "failed":
      return { text: `Can't open ${status.port}: ${status.message ?? "unknown error"}`, tone: "danger" };
  }
}

export function useHardwareInfo(intervalMs = 1000) {
  const [info, setInfo] = useState<HardwareInfo | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () => getHardwareInfo().then((i) => alive && setInfo(i)).catch(() => {});
    load();
    const timer = setInterval(load, intervalMs);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [intervalMs]);
  return info;
}

export default function Hardware({ app }: { app: AppState }) {
  const info = useHardwareInfo();
  const status = statusText(app.hardware);
  const d = info?.diagnostics;

  return (
    <>
      <PageHeader
        title="Hardware"
        subtitle={<Badge tone={status.tone}>{status.text}</Badge>}
        actions={
          <Button onClick={() => reconnectHardware()}>
            <RefreshIcon size={16} /> Reconnect
          </Button>
        }
      />

      <Card>
        <SettingRow icon={<LinkIcon />} label="Serial port" description="The port your board shows up as. deej reconnects by itself when it's unplugged.">
          <PortSelect app={app} info={info} />
        </SettingRow>
        <SettingRow icon={<BoardIcon />} label="Baud rate" description="Must match Serial.begin(…) in the board's sketch (9600 for the standard deej sketch).">
          <Select label="Baud rate" value={String(app.config.baud_rate)} onChange={(v) => setSetting("baud_rate", Number(v))}>
            {[...new Set([...BAUD_RATES, app.config.baud_rate])].sort((a, b) => a - b).map((b) => (
              <option key={b} value={b}>
                {b}
              </option>
            ))}
          </Select>
        </SettingRow>
      </Card>

      <SectionTitle>Diagnostics</SectionTitle>
      <Card className="p-4">
        {!d ? (
          <p className="text-text-muted">Loading…</p>
        ) : (
          <>
            <dl className="grid grid-cols-2 gap-x-8 gap-y-2 sm:grid-cols-3">
              <Stat label="Valid readings" value={d.validLines.toLocaleString()} />
              <Stat label="Ignored lines" value={(d.invalidLines + d.unconfirmedLines).toLocaleString()} hint="Garbled or partial lines, normal right after connecting" />
              <Stat label="Last reading" value={d.msSinceLastLine === null ? "—" : d.msSinceLastLine < 1000 ? "just now" : `${Math.round(d.msSinceLastLine / 1000)} s ago`} />
              <Stat label="Data received" value={`${(d.bytes / 1024).toFixed(0)} KB`} />
              <Stat label="Connections" value={String(d.connects)} />
              <Stat label="Last error" value={d.lastError ?? "none"} />
            </dl>
            <div className="mt-4">
              <div className="text-text-subtle mb-1 text-[12px]">Latest lines from the board</div>
              <pre className="bg-surface border-border text-text-muted max-h-32 overflow-auto rounded-md border p-2 font-mono text-[12px] leading-4 select-text">
                {d.recentLines.slice(-8).join("\n") || "—"}
              </pre>
            </div>
          </>
        )}
      </Card>
    </>
  );
}

function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div title={hint}>
      <dt className="text-text-subtle text-[12px]">{label}</dt>
      <dd className="truncate tabular-nums">{value}</dd>
    </div>
  );
}

function PortSelect({ app, info }: { app: AppState; info: HardwareInfo | null }) {
  const ports = info?.ports ?? [];
  const names = ports.map((p) => p.name);
  const current = app.config.com_port;

  return (
    <Select label="Serial port" value={current} onChange={(v) => setSetting("com_port", v)} className="max-w-[320px]">
      {!names.includes(current) && <option value={current}>{current} (not connected)</option>}
      {ports.map((p) => (
        <option key={p.name} value={p.name}>
          {p.name}
          {p.product ? ` — ${p.product.replace(` (${p.name})`, "")}` : ""}
          {p.usb ? ` [${p.usb[0].toString(16).padStart(4, "0")}:${p.usb[1].toString(16).padStart(4, "0")}]` : ""}
        </option>
      ))}
    </Select>
  );
}
