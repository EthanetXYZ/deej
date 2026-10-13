// Small Fluent-style building blocks shared by every view.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { getAppIcon } from "../ipc";
import { AppIcon } from "./icons";

export function cx(...classes: (string | false | null | undefined)[]) {
  return classes.filter(Boolean).join(" ");
}

export function percent(volume: number) {
  return `${Math.round(volume * 100)}%`;
}

/** A page heading with optional actions on the right. */
export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="mb-5 flex items-end justify-between gap-4">
      <div className="min-w-0">
        <h1 className="font-display text-[28px] leading-9 font-semibold">{title}</h1>
        {subtitle && <p className="text-text-muted mt-0.5 truncate">{subtitle}</p>}
      </div>
      {actions && <div className="flex shrink-0 gap-2">{actions}</div>}
    </header>
  );
}

export function SectionTitle({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="mt-6 mb-2 flex items-center justify-between first:mt-0">
      <h2 className="text-[14px] font-semibold">{children}</h2>
      {aside}
    </div>
  );
}

export function Card({ children, className, ...rest }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cx("bg-card border-border rounded-lg border", className)} {...rest}>
      {children}
    </div>
  );
}

/** A settings row: label and description on the left, control on the right. */
export function SettingRow({ icon, label, description, children }: { icon?: ReactNode; label: ReactNode; description?: ReactNode; children?: ReactNode }) {
  return (
    <div className="border-border flex min-h-[68px] items-center gap-4 border-b px-4 py-3 last:border-b-0">
      {icon && <div className="text-text-muted shrink-0">{icon}</div>}
      <div className="min-w-0 flex-1">
        <div>{label}</div>
        {description && <div className="text-text-subtle text-[12px] leading-4">{description}</div>}
      </div>
      {children && <div className="flex shrink-0 items-center gap-2">{children}</div>}
    </div>
  );
}

export function Button({
  children,
  variant = "standard",
  className,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "standard" | "accent" | "subtle" }) {
  return (
    <button
      type="button"
      className={cx(
        "inline-flex h-8 items-center justify-center gap-2 rounded-[4px] px-3 text-[14px] transition-colors disabled:opacity-40",
        variant === "accent" && "bg-accent text-on-accent hover:bg-accent-hover",
        variant === "standard" && "bg-control border-border-strong hover:bg-control-hover border",
        variant === "subtle" && "hover:bg-control text-text-muted hover:text-text",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

export function IconButton({ label, children, active, className, ...rest }: React.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      aria-pressed={active}
      className={cx(
        "inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-[4px] transition-colors",
        active ? "bg-accent-soft text-accent" : "text-text-muted hover:bg-control hover:text-text",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Toggle({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="group flex items-center gap-3 disabled:opacity-40"
    >
      <span className="text-text-muted text-[14px]">{checked ? "On" : "Off"}</span>
      <span
        className={cx(
          "relative h-5 w-10 rounded-full border transition-colors",
          checked ? "bg-accent border-accent" : "border-text-subtle group-hover:bg-control",
        )}
      >
        <span
          className={cx(
            "absolute top-1/2 h-3 w-3 -translate-y-1/2 rounded-full transition-all group-active:w-4",
            checked ? "bg-on-accent left-[22px]" : "bg-text-muted left-[3px]",
          )}
        />
      </span>
    </button>
  );
}

/** Fluent slider. Reports every change while dragging; `onCommit` fires when the drag ends. */
export function Slider({
  value,
  onChange,
  label,
  disabled,
  step = 0.01,
  min = 0,
  max = 1,
}: {
  value: number;
  onChange: (v: number) => void;
  label: string;
  disabled?: boolean;
  step?: number;
  min?: number;
  max?: number;
}) {
  const fill = ((value - min) / (max - min)) * 100;
  return (
    <input
      type="range"
      className="fluent-range"
      aria-label={label}
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      style={{ "--fill": `${fill}%` } as React.CSSProperties}
      onChange={(e) => onChange(Number(e.target.value))}
    />
  );
}

/**
 * A value the user is editing locally: while they drag (and briefly after), incoming updates from the backend
 * don't fight their hand. Changes are sent at most once per animation frame.
 */
export function useLocalValue(remote: number, send: (v: number) => void, holdMs = 700): [number, (v: number) => void] {
  const [local, setLocal] = useState(remote);
  const editingUntil = useRef(0);
  const frame = useRef<number | null>(null);
  const latest = useRef(remote);

  useEffect(() => {
    if (performance.now() > editingUntil.current) setLocal(remote);
  }, [remote]);

  const change = (v: number) => {
    editingUntil.current = performance.now() + holdMs;
    setLocal(v);
    latest.current = v;
    if (frame.current === null) {
      frame.current = requestAnimationFrame(() => {
        frame.current = null;
        send(latest.current);
      });
    }
  };

  return [local, change];
}

export function Segmented<T extends string>({ value, options, onChange, label }: { value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; label: string }) {
  return (
    <div role="radiogroup" aria-label={label} className="bg-control border-border-strong inline-flex rounded-[4px] border p-0.5">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          onClick={() => onChange(o.value)}
          className={cx(
            "h-7 rounded-[3px] px-3 text-[13px] transition-colors",
            value === o.value ? "bg-accent text-on-accent" : "text-text-muted hover:text-text",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Select({ value, onChange, children, label, className }: { value: string; onChange: (v: string) => void; children: ReactNode; label: string; className?: string }) {
  return (
    <select
      aria-label={label}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className={cx("bg-control border-border-strong hover:bg-control-hover h-8 rounded-[4px] border px-2 text-[14px]", className)}
    >
      {children}
    </select>
  );
}

export function TextInput(props: React.ComponentProps<"input">) {
  return (
    <input
      {...props}
      className={cx(
        "bg-control border-border-strong focus:border-b-accent h-8 rounded-[4px] border border-b-2 px-2 text-[14px] outline-none placeholder:text-text-subtle",
        props.className,
      )}
    />
  );
}

export function Badge({ children, tone = "neutral", title }: { children: ReactNode; tone?: "neutral" | "accent" | "success" | "warning" | "danger"; title?: string }) {
  return (
    <span
      title={title}
      className={cx(
        "inline-flex h-5 shrink-0 items-center gap-1 rounded-full px-2 text-[12px] font-semibold whitespace-nowrap",
        tone === "neutral" && "bg-control text-text-muted",
        tone === "accent" && "bg-accent-soft text-accent",
        tone === "success" && "text-success bg-[color-mix(in_srgb,var(--color-success)_15%,transparent)]",
        tone === "warning" && "text-warning bg-[color-mix(in_srgb,var(--color-warning)_15%,transparent)]",
        tone === "danger" && "text-danger bg-[color-mix(in_srgb,var(--color-danger)_15%,transparent)]",
      )}
    >
      {children}
    </span>
  );
}

/** An app's own icon (from its executable), or a generic one. */
export function AppImage({ exePath, size = 24, fallback }: { exePath: string | null; size?: number; fallback?: ReactNode }) {
  const [src, setSrc] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setSrc(null);
    if (exePath) getAppIcon(exePath).then((icon) => alive && setSrc(icon)).catch(() => {});
    return () => {
      alive = false;
    };
  }, [exePath]);

  if (src) return <img src={src} width={size} height={size} alt="" className="shrink-0" draggable={false} />;
  return (
    <div className="bg-control text-text-muted flex shrink-0 items-center justify-center rounded-md" style={{ width: size, height: size }}>
      {fallback ?? <AppIcon size={size * 0.7} />}
    </div>
  );
}
