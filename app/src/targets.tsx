// Targets, measurements and preference bounds: the UI (ADR-0021). One place
// for the feature, so it's polished as a unit. App.tsx only places these:
// HeadphonePicker in an output's settings, TargetPicker / TargetEditor in
// Settings › TARGETS, and GhostsSection in the EQ page's ⚙. Every component
// renders content only; App wraps it in its own modal.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";

export type TargetRef = { kind: "featured"; label: string } | { kind: "autoeq"; file: string } | { kind: "user"; name: string };

export type TargetInfo = {
  target: TargetRef;
  name: string;
  group: "featured" | "autoeq" | "yours";
  rig: string | null;
  pairsWith: string[];
  rigResponse: TargetRef | null;
};

export type TargetLibrary = { featured: TargetInfo[]; yours: TargetInfo[]; featuredError: string | null };

export type HeadphoneBinding = {
  name: string;
  source: string;
  form: string | null;
  rig: string | null;
  autoeq: string | null;
  files: string[];
};

export type HeadphoneRow = { deviceId: string; deviceName: string; connected: boolean; headphone: HeadphoneBinding };

type HeadphoneHit = { name: string; source: string; form: string; rig: string | null; path: string };

export type Overlay = {
  freqs: number[];
  headphone: HeadphoneBinding | null;
  measurementError: string | null;
  measurementLeft: (number | null)[];
  measurementRight: (number | null)[] | null;
  target: (number | null)[];
  targetError: string | null;
  boundsUpper: (number | null)[];
  boundsLower: (number | null)[];
  confidentBelowHz: number;
};

/** A user target as stored (targets/<name>.json). */
export type UserTarget = {
  name: string;
  base: TargetRef | null;
  bassDb: number;
  earDb: number;
  trebleDb: number;
  tiltDbPerOct: number;
  file: string | null;
  bounds: [string, string] | null;
};

/** What the EQ graph shows (per viewer: localStorage). */
export type GhostPrefs = {
  measurement: boolean;
  predicted: boolean;
  target: boolean;
  bounds: boolean;
  smoothing: string;
  targetRef: TargetRef | null;
  targetName: string | null;
};

const GHOSTS_KEY = "fletcher.ghosts";
const NO_GHOSTS: GhostPrefs = {
  measurement: false,
  predicted: false,
  target: false,
  bounds: false,
  smoothing: "1/12",
  targetRef: null,
  targetName: null,
};

export function readGhosts(): GhostPrefs {
  try {
    return { ...NO_GHOSTS, ...JSON.parse(localStorage.getItem(GHOSTS_KEY) ?? "{}") };
  } catch {
    return NO_GHOSTS;
  }
}

export function writeGhosts(p: GhostPrefs) {
  try {
    localStorage.setItem(GHOSTS_KEY, JSON.stringify(p));
  } catch {
    /* per-viewer nicety only */
  }
}

export const anyGhost = (p: GhostPrefs) => p.measurement || p.predicted || p.target || p.bounds;

export const sameTarget = (a: TargetRef | null, b: TargetRef | null) => JSON.stringify(a) === JSON.stringify(b);

/** "Sennheiser HD 650 · oratory1990 · GRAS 45BC-10" */
export const headphoneLine = (h: HeadphoneBinding) =>
  [h.name, h.source === "you" ? "your measurement" : `measured by ${h.source}`, h.rig].filter(Boolean).join(" · ");

/** An SVG path through the points that exist; a gap where a curve has no data. */
export function pathGaps(
  freqs: number[],
  vals: (number | null)[],
  xOf: (f: number) => number,
  yOf: (db: number) => number,
): string {
  let d = "";
  let pen = false;
  vals.forEach((v, i) => {
    if (v == null) {
      pen = false;
      return;
    }
    d += `${pen ? "L" : "M"}${xOf(freqs[i]).toFixed(1)} ${yOf(v).toFixed(1)}`;
    pen = true;
  });
  return d;
}

/** A band between two edges (bounds), as one closed path per stretch of data. */
export function bandPath(
  freqs: number[],
  upper: (number | null)[],
  lower: (number | null)[],
  xOf: (f: number) => number,
  yOf: (db: number) => number,
): string {
  let d = "";
  let run: number[] = [];
  const flush = () => {
    if (run.length > 1) {
      d += run.map((i, k) => `${k ? "L" : "M"}${xOf(freqs[i]).toFixed(1)} ${yOf(upper[i]!).toFixed(1)}`).join("");
      d += [...run].reverse().map((i) => `L${xOf(freqs[i]).toFixed(1)} ${yOf(lower[i]!).toFixed(1)}`).join("") + "Z";
    }
    run = [];
  };
  freqs.forEach((_, i) => (upper[i] != null && lower[i] != null ? run.push(i) : flush()));
  flush();
  return d;
}

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

// ---------------- an output's headphone ----------------

/** The Headphone section of an output's settings: what's bound, and how to
 *  change it (search AutoEq, import your own file(s), none). */
export function HeadphonePicker<D>({
  deviceId,
  binding,
  onDevices,
}: {
  deviceId: string;
  binding: HeadphoneBinding | null;
  onDevices: (list: D[]) => void;
}) {
  const [mode, setMode] = useState<"idle" | "search" | "import">("idle");
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<HeadphoneHit[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [files, setFiles] = useState<string[]>([]);
  const [name, setName] = useState("");
  const q = useDebounced(query, 250);

  useEffect(() => {
    if (mode !== "search" || q.trim().length < 2) return setHits([]);
    let live = true;
    invoke<HeadphoneHit[]>("headphone_search", { query: q })
      .then((h) => live && setHits(h))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [q, mode]);

  const run = (p: Promise<D[]>) => {
    setBusy(true);
    setError(null);
    p.then((list) => {
      onDevices(list);
      setMode("idle");
      setQuery("");
      setFiles([]);
    })
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  };

  const pickFiles = async () => {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({
      multiple: true,
      filters: [{ name: "Frequency response (REW text, AutoEq CSV)", extensions: ["txt", "csv", "frd"] }],
    });
    const list = (Array.isArray(picked) ? picked : picked ? [picked] : []).slice(0, 2);
    if (!list.length) return;
    setFiles(list);
    setMode("import");
    const stem = list[0].split(/[\\/]/).pop()!.replace(/\.[^.]+$/, "").replace(/\s+[LR]$/i, "");
    setName((n) => n || stem);
  };

  return (
    <div className="hp-picker">
      <div className="hp-current">
        {binding ? (
          <>
            <b>{binding.name}</b>
            <span className="dim-sm">{headphoneLine(binding).slice(binding.name.length + 3)}</span>
          </>
        ) : (
          <span className="dim-sm">No headphone set. The EQ page can show a headphone's measurement once one is.</span>
        )}
      </div>
      <div className="room-row">
        <button onClick={() => setMode(mode === "search" ? "idle" : "search")} disabled={busy}>
          search AutoEq…
        </button>
        <button onClick={pickFiles} disabled={busy}>
          import a measurement…
        </button>
        {binding && (
          <button onClick={() => run(invoke<D[]>("device_headphone_clear", { id: deviceId }))} disabled={busy}>
            none
          </button>
        )}
      </div>
      {mode === "search" && (
        <div className="hp-search">
          <input
            autoFocus
            placeholder="headphone name…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <div className="hp-hits">
            {hits.map((h) => (
              <div
                key={h.path}
                className="preset-row"
                onClick={() => !busy && run(invoke<D[]>("device_headphone_autoeq", { id: deviceId, path: h.path }))}
              >
                <span>{h.name}</span>
                <span className="spacer" />
                <span className="dim-sm">{[h.source, h.rig].filter(Boolean).join(" · ")}</span>
              </div>
            ))}
            {q.trim().length >= 2 && !hits.length && <span className="dim-sm">no matches</span>}
          </div>
        </div>
      )}
      {mode === "import" && (
        <div className="hp-search">
          <span className="dim-sm">
            {files.length === 2
              ? `Left: ${files[0].split(/[\\/]/).pop()} · Right: ${files[1].split(/[\\/]/).pop()}`
              : `${files[0]?.split(/[\\/]/).pop()} (both ears; pick two files for left and right)`}
          </span>
          <div className="room-row">
            <input placeholder="headphone name" value={name} onChange={(e) => setName(e.target.value)} />
            <button
              disabled={busy || !name.trim()}
              onClick={() =>
                run(
                  invoke<D[]>("device_headphone_import", {
                    id: deviceId,
                    name: name.trim(),
                    left: files[0],
                    right: files[1] ?? null,
                  }),
                )
              }
            >
              use it
            </button>
          </div>
        </div>
      )}
      {busy && <span className="dim-sm">fetching…</span>}
      {error && <span className="dim-sm warn-text">{error}</span>}
      <span className="dim-sm">
        A measurement is one rig's average over a few seatings, not your ears; above 10 kHz it depends more on
        fit than on the headphone.
      </span>
    </div>
  );
}

// ---------------- targets ----------------

/** Choose a target: yours, AutoEq's featured set, or (typing 2+ letters)
 *  any AutoEq target. Each row names the rig it's made for, when known. */
export function TargetPicker({
  value,
  library,
  onPick,
  allowFlat = false,
}: {
  value: TargetRef | null;
  library: TargetLibrary | null;
  onPick: (t: TargetInfo | null) => void;
  allowFlat?: boolean;
}) {
  const [query, setQuery] = useState("");
  const [found, setFound] = useState<TargetInfo[]>([]);
  const q = useDebounced(query.trim().toLowerCase(), 250);
  useEffect(() => {
    if (q.length < 2) return setFound([]);
    let live = true;
    invoke<TargetInfo[]>("targets_search", { query: q })
      .then((r) => live && setFound(r))
      .catch(() => live && setFound([]));
    return () => {
      live = false;
    };
  }, [q]);
  const match = (t: TargetInfo) => !q || t.name.toLowerCase().includes(q);
  const row = (t: TargetInfo) => (
    <div
      key={`${t.group}:${t.name}`}
      className={`preset-row ${sameTarget(t.target, value) ? "current" : ""}`}
      onClick={() => onPick(t)}
      title={t.pairsWith.length ? `AutoEq pairs it with: ${t.pairsWith.join(", ")}` : undefined}
    >
      <span>{t.name}</span>
      <span className="spacer" />
      {t.rig && <span className="dim-sm">{t.rig}</span>}
    </div>
  );
  const yours = (library?.yours ?? []).filter(match);
  const featured = (library?.featured ?? []).filter(match);
  return (
    <div className="target-picker">
      <input placeholder="search targets…" value={query} onChange={(e) => setQuery(e.target.value)} />
      <div className="tp-list">
        {allowFlat && !q && (
          <div className={`preset-row ${value == null ? "current" : ""}`} onClick={() => onPick(null)}>
            <span>Flat</span>
          </div>
        )}
        {yours.length > 0 && <span className="mono lab-label c-sect">YOURS</span>}
        {yours.map(row)}
        {featured.length > 0 && <span className="mono lab-label c-sect">FROM AUTOEQ</span>}
        {featured.map(row)}
        {library?.featuredError && !featured.length && <span className="dim-sm">{library.featuredError}</span>}
        {found.length > 0 && <span className="mono lab-label c-sect">ALL AUTOEQ TARGETS</span>}
        {found.map(row)}
      </div>
    </div>
  );
}

/** A small graph of one or two target curves (the editor's preview). */
function TargetPreview({ curve, ghost }: { curve: { freqs: number[]; db: (number | null)[] } | null; ghost?: { freqs: number[]; db: (number | null)[] } | null }) {
  const W = 520;
  const H = 180;
  const [x0, x1, y0, y1] = [34, W - 8, 8, H - 20];
  const vals = [...(curve?.db ?? []), ...(ghost?.db ?? [])].filter((v): v is number => v != null);
  const span = Math.max(6, ...vals.map(Math.abs)) + 2;
  const top = Math.ceil(span / 3) * 3;
  const xOf = (f: number) => x0 + (Math.log10(f / 20) / 3) * (x1 - x0);
  const yOf = (v: number) => y0 + ((top - v) / (2 * top)) * (y1 - y0);
  const ticks = [-top, -top / 2, 0, top / 2, top].map((v) => Math.round(v));
  return (
    <svg viewBox={`0 0 ${W} ${H}`} className="target-preview">
      <rect x={x0} y={y0} width={x1 - x0} height={y1 - y0} className="plot-bg" />
      {[20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000].map((f) => (
        <g key={f}>
          <line x1={xOf(f)} x2={xOf(f)} y1={y0} y2={y1} className="grid-minor" />
          <text x={xOf(f)} y={H - 6} className="tp-axis" textAnchor="middle">
            {f >= 1000 ? `${f / 1000}k` : f}
          </text>
        </g>
      ))}
      {ticks.map((v) => (
        <g key={v}>
          <line x1={x0} x2={x1} y1={yOf(v)} y2={yOf(v)} className={v === 0 ? "grid-zero" : "grid-minor"} />
          <text x={x0 - 5} y={yOf(v) + 3} className="tp-axis" textAnchor="end">
            {v > 0 ? `+${v}` : v}
          </text>
        </g>
      ))}
      {ghost && <path d={pathGaps(ghost.freqs, ghost.db, xOf, yOf)} className="ghost-meas" />}
      {curve && <path d={pathGaps(curve.freqs, curve.db, xOf, yOf)} className="ghost-target" />}
    </svg>
  );
}

const KNOBS = [
  { key: "bassDb", label: "Bass", min: -12, max: 18, step: 0.5, unit: "dB", note: "low shelf, 105 Hz" },
  { key: "earDb", label: "Ear gain", min: -10, max: 10, step: 0.5, unit: "dB", note: "peak, 2.75 kHz" },
  { key: "trebleDb", label: "Treble", min: -12, max: 12, step: 0.5, unit: "dB", note: "high shelf, 2.5 kHz" },
  { key: "tiltDbPerOct", label: "Tilt", min: -2, max: 2, step: 0.05, unit: "dB/oct", note: "the whole range" },
] as const;

/** Create or edit one of your targets: a base plus four knobs, previewed live. */
export function TargetEditor({
  initial,
  library,
  onSaved,
  onClose,
}: {
  initial: UserTarget | null;
  library: TargetLibrary | null;
  onSaved: (lib: TargetLibrary) => void;
  onClose: () => void;
}) {
  const [t, setT] = useState<UserTarget>(
    initial ?? {
      name: "",
      base: library?.featured.find((f) => f.name.toLowerCase().includes("harman over-ear"))?.target ?? null,
      bassDb: 0,
      earDb: 0,
      trebleDb: 0,
      tiltDbPerOct: 0,
      file: null,
      bounds: null,
    },
  );
  const [picking, setPicking] = useState(false);
  const [preview, setPreview] = useState<{ freqs: number[]; db: (number | null)[] } | null>(null);
  const [rigCurve, setRigCurve] = useState<{ freqs: number[]; db: (number | null)[] } | null>(null);
  const [showRig, setShowRig] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const live = useDebounced(t, 120);
  const allInfos = useMemo(() => [...(library?.yours ?? []), ...(library?.featured ?? [])], [library]);
  const baseInfo = allInfos.find((i) => sameTarget(i.target, t.base)) ?? null;
  const baseName = t.base ? (baseInfo?.name ?? ("file" in t.base ? t.base.file.replace(/\.csv$/, "") : "")) : "Flat";
  const imported = !!t.file;

  useEffect(() => {
    const args = imported
      ? { target: { kind: "user", name: initial!.name }, base: null, knobs: null }
      : { target: null, base: live.base, knobs: live };
    invoke<{ freqs: number[]; db: (number | null)[] }>("target_preview", args)
      .then(setPreview)
      .catch((e) => setError(String(e)));
  }, [live, imported, initial]);

  useEffect(() => {
    if (!showRig || !baseInfo?.rigResponse) return setRigCurve(null);
    invoke<{ freqs: number[]; db: (number | null)[] }>("target_preview", { target: baseInfo.rigResponse })
      .then(setRigCurve)
      .catch(() => setRigCurve(null));
  }, [showRig, baseInfo]);

  const save = () =>
    invoke<TargetLibrary>("target_save", { target: { ...t, name: t.name.trim() }, previous: initial?.name ?? null })
      .then((lib) => {
        onSaved(lib);
        onClose();
      })
      .catch((e) => setError(String(e)));

  const nameRef = useRef<HTMLInputElement>(null);
  return (
    <div className="target-editor">
      <div className="dev-sect">
        <span className="room-key">Name</span>
        <input ref={nameRef} value={t.name} placeholder="My target" onChange={(e) => setT({ ...t, name: e.target.value })} />
      </div>
      {!imported && (
        <div className="dev-sect">
          <span className="room-key">Base</span>
          <div className="room-row">
            <span>{baseName}</span>
            <button onClick={() => setPicking((p) => !p)}>{picking ? "done" : "change"}</button>
          </div>
          {baseInfo?.rig && (
            <span className="dim-sm">
              Made for {baseInfo.rig}
              {baseInfo.rigResponse && (
                <>
                  {" · "}
                  <span className="link-like" onClick={() => setShowRig((s) => !s)}>
                    {showRig ? "hide" : "show"} that rig's ear response
                  </span>
                </>
              )}
            </span>
          )}
          {picking && (
            <TargetPicker
              value={t.base}
              library={library}
              allowFlat
              onPick={(i) => {
                if (i?.target.kind === "user" && i.target.name === t.name) return;
                setT({ ...t, base: i?.target ?? null });
                setPicking(false);
              }}
            />
          )}
        </div>
      )}
      {!imported && (
        <div className="dev-sect">
          <span className="room-key">Adjust</span>
          {KNOBS.map((k) => (
            <label key={k.key} className="knob-row" title={k.note}>
              <span>{k.label}</span>
              <input
                type="range"
                min={k.min}
                max={k.max}
                step={k.step}
                value={t[k.key]}
                onChange={(e) => setT({ ...t, [k.key]: +e.target.value })}
              />
              <span className="mono">
                {t[k.key] > 0 ? "+" : ""}
                {t[k.key].toFixed(k.step < 0.5 ? 2 : 1)} {k.unit}
              </span>
            </label>
          ))}
          <span className="dim-sm">Bass: low shelf 105 Hz · Ear gain: peak 2.75 kHz · Treble: high shelf 2.5 kHz · Tilt: across the whole range</span>
        </div>
      )}
      {imported && <span className="dim-sm">An imported file: shown as it is.</span>}
      <TargetPreview curve={preview} ghost={showRig ? rigCurve : null} />
      {error && <span className="dim-sm warn-text">{error}</span>}
      <div className="room-row">
        <button onClick={save} disabled={!t.name.trim()}>
          save
        </button>
        {initial && (
          <button
            onClick={() =>
              invoke<TargetLibrary>("target_delete", { name: initial.name })
                .then((lib) => {
                  onSaved(lib);
                  onClose();
                })
                .catch((e) => setError(String(e)))
            }
          >
            delete
          </button>
        )}
      </div>
    </div>
  );
}

// ---------------- the EQ page's ghosts ----------------

const SMOOTHINGS: [string, string][] = [
  ["none", "none"],
  ["1/24", "1/24"],
  ["1/12", "1/12"],
  ["1/6", "1/6"],
  ["1/3", "1/3"],
  ["erb", "ERB"],
];

/** Everything the EQ graph can show behind your curve, in one section. */
export function GhostsSection({
  prefs,
  setPrefs,
  overlay,
  library,
  otherEar,
  deviceName,
  onOpenDevice,
}: {
  prefs: GhostPrefs;
  setPrefs: (p: GhostPrefs) => void;
  overlay: Overlay | null;
  library: TargetLibrary | null;
  /** The other-ear ghost (per-ear editing), owned by App. */
  otherEar: { on: boolean; enabled: boolean; toggle: () => void };
  deviceName: string | null;
  onOpenDevice: () => void;
}) {
  const [picking, setPicking] = useState(false);
  const set = (patch: Partial<GhostPrefs>) => setPrefs({ ...prefs, ...patch });
  const sw = (on: boolean, enabled: boolean, toggle: () => void, label: string, note?: string) => (
    <div className="room-row">
      <div className={`switch ${on ? "on" : ""} ${enabled ? "" : "disabled"}`} onClick={() => enabled && toggle()}>
        <div className="knob" />
      </div>
      <span className="room-key">{label}</span>
      {note && <span className="dim-sm">{note}</span>}
    </div>
  );
  const hp = overlay?.headphone ?? null;
  const hasBounds = !!overlay && overlay.boundsUpper.some((v) => v != null);
  return (
    <>
      <span className="mono lab-label">GHOSTS</span>
      {sw(otherEar.on, otherEar.enabled, otherEar.toggle, "Other ear")}
      {sw(prefs.measurement, true, () => set({ measurement: !prefs.measurement }), "Measurement", hp ? hp.name : undefined)}
      {sw(prefs.predicted, true, () => set({ predicted: !prefs.predicted }), "Measurement + EQ")}
      {(prefs.measurement || prefs.predicted) && !hp && (
        <span className="dim-sm room-note">
          {`No headphone set for ${deviceName ?? "this output"}. `}
          <span className="link-like" onClick={onOpenDevice}>
            Set one in its settings
          </span>
        </span>
      )}
      {overlay?.measurementError && (prefs.measurement || prefs.predicted) && (
        <span className="dim-sm room-note warn-text">{overlay.measurementError}</span>
      )}
      {sw(prefs.target, true, () => set({ target: !prefs.target }), "Target", prefs.targetName ?? "none chosen")}
      {prefs.target && (
        <div className="room-row">
          <span className="fe-btn mono" onClick={() => setPicking((p) => !p)}>
            {picking ? "done" : "choose…"}
          </span>
          {overlay?.targetError && <span className="dim-sm warn-text">{overlay.targetError}</span>}
        </div>
      )}
      {prefs.target && picking && (
        <TargetPicker
          value={prefs.targetRef}
          library={library}
          onPick={(i) => {
            set({ targetRef: i?.target ?? null, targetName: i?.name ?? null });
            setPicking(false);
          }}
        />
      )}
      {sw(prefs.bounds, hasBounds || prefs.bounds, () => set({ bounds: !prefs.bounds }), "Bounds", hasBounds ? undefined : "this target has none")}
      <div className="room-row">
        <span className="room-key">Smoothing</span>
        <div className="seg seg-sm">
          {SMOOTHINGS.map(([v, label]) => (
            <span key={v} className={`seg-opt ${prefs.smoothing === v ? "on" : ""}`} onClick={() => set({ smoothing: v })}>
              {label}
            </span>
          ))}
        </div>
      </div>
      <span className="dim-sm room-note">
        Lined up at 0 dB at 500 Hz; the target is moved onto the measurement over 100 Hz–10 kHz. A measurement is one
        rig's average, not your ears; a target is a population's preference.
      </span>
    </>
  );
}
