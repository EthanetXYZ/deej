// The UI's copy of the backend state, kept current by backend events. Components read it with useStore().
//
// Slider positions arrive many times a second; they're coalesced to one update per animation frame. When a hardware
// slider moves, its index and time are recorded so views can highlight what it controls.

import { useSyncExternalStore } from "react";
import { getState, onEvent, type AppState, type UiEvent } from "./ipc";

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  text: string;
}

export interface StoreState {
  app: AppState | null;
  loadError: string | null;
  toasts: Toast[];
  /** When each hardware slider last moved (performance.now()). */
  moved: Record<number, number>;
}

let state: StoreState = { app: null, loadError: null, toasts: [], moved: {} };
const subscribers = new Set<() => void>();

function set(next: Partial<StoreState>) {
  state = { ...state, ...next };
  subscribers.forEach((s) => s());
}

export function useStore<T>(select: (s: StoreState) => T): T {
  return useSyncExternalStore(
    (callback) => {
      subscribers.add(callback);
      return () => subscribers.delete(callback);
    },
    () => select(state),
  );
}

let toastId = 0;
export function toast(kind: Toast["kind"], text: string) {
  const id = ++toastId;
  set({ toasts: [...state.toasts.filter((t) => t.text !== text), { id, kind, text }].slice(-3) });
  setTimeout(() => set({ toasts: state.toasts.filter((t) => t.id !== id) }), kind === "error" ? 6000 : 2500);
}

let refetchTimer: number | undefined;
function refetch() {
  clearTimeout(refetchTimer);
  refetchTimer = window.setTimeout(async () => {
    try {
      set({ app: await getState(), loadError: null });
    } catch (e) {
      set({ loadError: String(e) });
    }
  }, 30);
}

let pendingPositions: number[] | null = null;
function schedulePositions(positions: number[]) {
  const first = pendingPositions === null;
  pendingPositions = positions;
  if (!first) return;

  requestAnimationFrame(() => {
    const positions = pendingPositions!;
    pendingPositions = null;
    if (!state.app) return;

    const now = performance.now();
    const moved = { ...state.moved };
    positions.forEach((p, i) => {
      if (Math.abs(p - (state.app!.positions[i] ?? -1)) >= 0.005) moved[i] = now;
    });
    set({ app: { ...state.app, positions }, moved });
  });
}

function apply(event: UiEvent) {
  const app = state.app;

  switch (event.type) {
    case "sliders":
      schedulePositions(event.positions);
      return;
    case "hardware":
      if (app) set({ app: { ...app, hardware: event.status } });
      return;
    case "volume": {
      if (!app) return;
      const { target, volume, muted } = event;
      const audio = {
        ...app.audio,
        sessions: target.kind === "session" ? app.audio.sessions.map((s) => (s.id === target.id ? { ...s, volume, muted } : s)) : app.audio.sessions,
        endpoints: target.kind === "endpoint" ? app.audio.endpoints.map((e) => (e.id === target.id ? { ...e, volume, muted } : e)) : app.audio.endpoints,
      };
      set({ app: { ...app, audio } });
      return;
    }
    case "audio":
    case "configChanged":
      refetch();
      return;
    case "configReloaded":
      refetch();
      toast("info", "config.yaml changed and was reloaded");
      return;
    case "configError":
      refetch();
      toast("error", `config.yaml has a problem, so the previous settings stay active: ${event.message}`);
      return;
    case "saved":
      toast("success", "Saved");
      return;
    case "saveFailed":
      toast("error", `Couldn't save: ${event.message}`);
      return;
  }
}

let started = false;
export function startStore() {
  if (started) return;
  started = true;
  onEvent(apply).catch((e) => set({ loadError: String(e) }));
  refetch();
}
