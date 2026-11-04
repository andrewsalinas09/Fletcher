// The pop-out ("satellite") scaffold (Q-20, rule 10). Every new pop-out uses
// this instead of hand-wiring its own events, so none can miss a piece:
// - main owns the state: `useSatelliteHost` sends `<channel>-sync` whenever
//   it changes and answers `<channel>-hello`; the window sends `<channel>-cmd`.
// - Main's listeners are trampolines (refs refreshed each render), so vite's
//   fast refresh can't leave them running stale code or missing (dev law).
// - The window says hello until main answers, so a slow or reloading main
//   window can't leave it "waiting for the main window…" forever.
// - Keys: satellites get no page-level DOM keys; Ctrl+Z/Y reach the undo rail
//   as focus-scoped OS shortcuts, on for every pop-out by default (lib.rs
//   has_undo_keys). Capabilities cover every window ("windows": ["*"]).
import { emit, emitTo, listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";

/** Main window side: keep `<channel>` windows in sync and take their commands. */
export function useSatelliteHost<S, C>(channel: string, state: S, onCmd: (c: C) => void) {
  const stateRef = useRef(state);
  stateRef.current = state;
  const cmdRef = useRef(onCmd);
  cmdRef.current = onCmd;
  const key = JSON.stringify(state);
  useEffect(() => {
    emit(`${channel}-sync`, stateRef.current).catch(() => {});
  }, [channel, key]);
  useEffect(() => {
    const hello = listen(`${channel}-hello`, () => emit(`${channel}-sync`, stateRef.current).catch(() => {}));
    const cmd = listen<C>(`${channel}-cmd`, (e) => cmdRef.current(e.payload));
    return () => {
      hello.then((f) => f());
      cmd.then((f) => f());
    };
  }, [channel]);
}

/** Satellite side: the state main sends, and a way to send commands back. */
export function useSatellite<S, C>(channel: string): { state: S | null; send: (c: C) => void } {
  const [state, setState] = useState<S | null>(null);
  const got = useRef(false);
  useEffect(() => {
    let gone = false;
    let un: (() => void) | undefined;
    let timer: number | undefined;
    // Listen first, then say hello (the reply is a one-shot), and keep saying
    // it once a second until main answers.
    listen<S>(`${channel}-sync`, (e) => {
      if (gone) return;
      got.current = true;
      setState(e.payload);
    }).then((f) => {
      if (gone) return f();
      un = f;
      const hello = () => {
        if (!got.current) emitTo("main", `${channel}-hello`, {}).catch(() => {});
      };
      hello();
      timer = window.setInterval(hello, 1000);
    });
    return () => {
      gone = true;
      un?.();
      if (timer) window.clearInterval(timer);
    };
  }, [channel]);
  return { state, send: (c: C) => void emitTo("main", `${channel}-cmd`, c).catch(() => {}) };
}

/** Open a pop-out, or bring an open one forward. `view` is its `?view=` name. */
export async function openSatellite(label: string, title: string, width: number, height: number) {
  const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const existing = await WebviewWindow.getByLabel(label);
  if (existing) {
    await existing.show().catch(() => {});
    await existing.setFocus();
  } else {
    new WebviewWindow(label, { url: `index.html?view=${label}`, title, width, height });
  }
}
