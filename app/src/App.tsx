// The window: navigation on the left, the current page on the right, toasts at the bottom.

import { useEffect, useState } from "react";
import { getUiPrefs, reportUiReady, setUiPrefs, type Theme } from "./ipc";
import { appGroups, sliderIndexes } from "./model";
import { startStore, useStore } from "./store";
import { cx } from "./components/ui";
import { BoardIcon, CheckIcon, InfoIcon, MixerIcon, SettingsIcon, WarningIcon } from "./components/icons";
import Hardware, { statusText } from "./views/Hardware";
import Sliders from "./views/Sliders";
import Settings, { type Section } from "./views/Settings";

type Page = "sliders" | "hardware" | "settings";

const PAGES: { id: Page; label: string; icon: React.ReactNode }[] = [
  { id: "sliders", label: "Sliders", icon: <MixerIcon /> },
  { id: "hardware", label: "Hardware", icon: <BoardIcon /> },
  { id: "settings", label: "Settings", icon: <SettingsIcon /> },
];

function applyTheme(theme: Theme) {
  const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

function useTheme(): [Theme, (t: Theme) => void] {
  const [theme, setTheme] = useState<Theme>("dark");

  useEffect(() => {
    getUiPrefs()
      .then((p) => setTheme(p.theme))
      .catch(() => {});
  }, []);

  useEffect(() => {
    applyTheme(theme);
    if (theme !== "system") return;
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const follow = () => applyTheme("system");
    query.addEventListener("change", follow);
    return () => query.removeEventListener("change", follow);
  }, [theme]);

  return [
    theme,
    (t) => {
      setTheme(t);
      setUiPrefs({ theme: t }).catch(() => {});
    },
  ];
}

export default function App() {
  const app = useStore((s) => s.app);
  const loadError = useStore((s) => s.loadError);
  const [page, setPage] = useState<Page>("sliders");
  const [section, setSection] = useState<Section>("General");
  const [theme, setTheme] = useTheme();

  useEffect(startStore, []);

  // once, after the first render with real data
  const reported = app !== null;
  useEffect(() => {
    if (reported && app) reportUiReady(page, app.audio.endpoints.length, appGroups(app.audio.sessions).length, sliderIndexes(app).length);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reported]);

  return (
    <div className="flex h-full">
      <nav className="bg-sidebar flex w-[200px] shrink-0 flex-col gap-0.5 px-2 pt-4 pb-3" aria-label="Pages">
        <div className="font-display mb-4 flex items-center gap-2 px-3 text-[15px] font-semibold">
          <MixerIcon className="text-accent" /> deej
        </div>
        {PAGES.map((p) => (
          <button
            key={p.id}
            type="button"
            onClick={() => setPage(p.id)}
            aria-current={page === p.id ? "page" : undefined}
            className={cx(
              "relative flex h-9 items-center gap-3 rounded-[4px] px-3 text-left transition-colors",
              page === p.id ? "bg-control text-text" : "text-text-muted hover:bg-control hover:text-text",
            )}
          >
            {page === p.id && <span className="bg-accent absolute top-2 bottom-2 left-0 w-[3px] rounded-full" />}
            {p.icon}
            {p.label}
          </button>
        ))}
        <div className="flex-1" />
        {app && <ConnectionFooter onClick={() => setPage("hardware")} />}
      </nav>

      <main className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-[920px] px-8 py-6">
          {!app && !loadError && <p className="text-text-muted">Loading…</p>}
          {loadError && !app && (
            <p className="text-danger">
              Can't reach deej's background service: {loadError}
            </p>
          )}
          {app && page === "sliders" && <Sliders app={app} />}
          {app && page === "hardware" && <Hardware app={app} />}
          {app && page === "settings" && (
            <Settings app={app} section={section} onSection={setSection} theme={theme} onTheme={setTheme} onOpenMappings={() => setPage("sliders")} />
          )}
        </div>
      </main>

      <Toasts />
    </div>
  );
}

function ConnectionFooter({ onClick }: { onClick: () => void }) {
  const hardware = useStore((s) => s.app?.hardware ?? null);
  const configError = useStore((s) => s.app?.configError ?? null);
  const { text, tone } = statusText(hardware);

  return (
    <button type="button" onClick={onClick} className="hover:bg-control flex flex-col gap-1 rounded-[4px] px-3 py-2 text-left">
      <span className="flex items-center gap-2 text-[12px]">
        <span
          className={cx(
            "h-2 w-2 shrink-0 rounded-full",
            tone === "success" && "bg-success",
            tone === "warning" && "bg-warning",
            tone === "danger" && "bg-danger",
            tone === "neutral" && "bg-text-subtle",
          )}
        />
        <span className="text-text-muted truncate">{text}</span>
      </span>
      {configError && <span className="text-danger text-[12px]">config.yaml has an error</span>}
    </button>
  );
}

function Toasts() {
  const toasts = useStore((s) => s.toasts);
  return (
    <div className="pointer-events-none fixed right-6 bottom-6 flex flex-col items-end gap-2" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className="toast-in bg-card border-border-strong flex max-w-[420px] items-start gap-2 rounded-lg border px-3 py-2 shadow-lg">
          {t.kind === "success" && <CheckIcon size={16} className="text-success mt-0.5 shrink-0" />}
          {t.kind === "info" && <InfoIcon size={16} className="text-accent mt-0.5 shrink-0" />}
          {t.kind === "error" && <WarningIcon size={16} className="text-danger mt-0.5 shrink-0" />}
          <span className="text-[13px]">{t.text}</span>
        </div>
      ))}
    </div>
  );
}
