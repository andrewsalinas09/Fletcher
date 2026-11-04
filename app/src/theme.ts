// Light, dark, or match Windows (ADR-0020). Imported before anything renders,
// so no window ever paints in the wrong theme first. The choice lives in
// localStorage, which every Fletcher window shares. A change is also
// broadcast as an app event (the `storage` event between webviews isn't
// something to rely on), so pop-outs, scopes and the hotkey popup all follow
// without any wiring of their own.
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

export type ThemeChoice = "light" | "dark" | "system";
const KEY = "fletcher.theme";
const dark = window.matchMedia("(prefers-color-scheme: dark)");

export function themeChoice(): ThemeChoice {
  try {
    const v = localStorage.getItem(KEY);
    return v === "dark" || v === "system" ? v : "light";
  } catch {
    return "light";
  }
}

const resolve = (c: ThemeChoice): "light" | "dark" =>
  c === "system" ? (dark.matches ? "dark" : "light") : c;

/** Canvas can't read CSS variables, so drawing code asks here. Cached until
 *  the theme changes. */
const cache = new Map<string, string>();
export function tok(name: string): string {
  let v = cache.get(name);
  if (v === undefined) {
    v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    cache.set(name, v);
  }
  return v;
}

/** A hex token at an alpha, for canvas strokes and labels. */
export function tokA(name: string, alpha: number): string {
  const n = parseInt(tok(name).slice(1), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}

/** Changes whenever the theme does: a canvas lists it in its draw effect's
 *  dependencies so it redraws in the new colors. */
export function useThemeTick(): number {
  const [n, setN] = useState(0);
  useEffect(() => {
    const bump = () => setN((x) => x + 1);
    window.addEventListener("fletcher-theme", bump);
    return () => window.removeEventListener("fletcher-theme", bump);
  }, []);
  return n;
}

/** The window's theme as last told to Windows ("system" = left to Windows). */
let told: ThemeChoice | null = null;

function apply() {
  const c = themeChoice();
  // The title bar is Windows'. Dark forces it dark to match; Light and Match
  // Windows leave it to Windows, as Fletcher always did (the owner keeps a
  // dark title bar over the light theme). Forcing it also forces what the
  // page's prefers-color-scheme reports, so Match Windows must hand it back,
  // or the query echoes the last forced theme (TB-62). Handing it back fires
  // the query's change event, which runs this again with Windows' setting.
  if (told !== c) {
    told = c;
    getCurrentWindow()
      .setTheme(c === "dark" ? "dark" : null)
      .catch(() => {});
  }
  const t = resolve(c);
  if (document.documentElement.dataset.theme !== t) {
    document.documentElement.dataset.theme = t;
    cache.clear();
    window.dispatchEvent(new Event("fletcher-theme"));
  }
}

export function setThemeChoice(c: ThemeChoice) {
  try {
    localStorage.setItem(KEY, c);
  } catch {
    /* the choice just isn't remembered */
  }
  apply();
  emit("theme-changed").catch(() => {});
}

apply();
dark.addEventListener("change", apply);
window.addEventListener("storage", (e) => {
  if (e.key === KEY) apply();
});
listen("theme-changed", apply).catch(() => {});
