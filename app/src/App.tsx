import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type EqFilter = {
  enabled: boolean;
  kind: string;
  fcHz: number;
  gainDb: number;
  q: number;
  responseDb: number[];
  sourceFile: string;
};

type EqState = {
  deviceName: string | null;
  preampDb: number;
  freqs: number[];
  sumDb: number[];
  filters: EqFilter[];
  sourceFiles: string[];
  includes: string[];
};

const OWN_FILE = "fletcher.txt";
const KINDS = ["PK", "LSC", "LS", "HSC", "HS", "LP", "LPQ", "HP", "HPQ", "NO", "BP", "AP"];

// ---- graph geometry (design/Main.dc.html v5) ----
const GW = 1090;
const GH = 330;
const FMIN = 20;
const FMAX = 20000;

const xOf = (f: number) =>
  ((Math.log10(f) - Math.log10(FMIN)) / (Math.log10(FMAX) - Math.log10(FMIN))) * GW;
const fOf = (x: number) =>
  Math.pow(10, Math.log10(FMIN) + (x / GW) * (Math.log10(FMAX) - Math.log10(FMIN)));

const fmtHz = (hz: number) =>
  hz >= 1000 ? `${+(hz / 1000).toFixed(2)}k` : `${+hz.toFixed(0)}`;
const fmtGain = (g: number) => `${g > 0 ? "+" : ""}${+g.toFixed(1)}`;

function pathFrom(freqs: number[], dbs: number[], yOf: (db: number) => number): string {
  return dbs
    .map((db, i) => `${i === 0 ? "M" : "L"}${xOf(freqs[i]).toFixed(1)} ${yOf(db).toFixed(1)}`)
    .join(" ");
}

function TypeGlyph({ kind, boost, dark }: { kind: string; boost: boolean; dark?: boolean }) {
  const color = boost ? (dark ? "var(--boost-dark)" : "var(--boost)") : dark ? "#9db8d2" : "var(--cut)";
  const d = (() => {
    switch (kind) {
      case "LSC":
      case "LS":
        return boost ? "M1 4 L5 4 C9 4 10 9 13 9 L15 9" : "M1 8 L5 8 C9 8 10 3 13 3 L15 3";
      case "HSC":
      case "HS":
        return boost ? "M1 9 L3 9 C6 9 7 4 10 4 L15 4" : "M1 3 L3 3 C6 3 7 8 10 8 L15 8";
      case "LP":
      case "LPQ":
        return "M1 4 L6 4 C10 4 11 10 15 11";
      case "HP":
      case "HPQ":
        return "M1 11 C5 10 6 4 10 4 L15 4";
      case "NO":
        return "M1 4 L5 4 L8 11 L11 4 L15 4";
      case "BP":
        return "M1 11 C5 11 5 3 8 3 C11 3 11 11 15 11";
      default:
        return boost ? "M1 10 C4 10 5 4 8 4 C11 4 12 10 15 10" : "M1 3 C4 3 5 9 8 9 C11 9 12 3 15 3";
    }
  })();
  return (
    <svg width="18" height="12" viewBox="0 0 16 12" fill="none" stroke={color} strokeWidth="1.5">
      <path d={d} />
    </svg>
  );
}

function GainGauge({ gainDb, dark }: { gainDb: number; dark?: boolean }) {
  const clamped = Math.max(-12, Math.min(12, gainDb));
  const theta = (Math.abs(clamped) / 12) * (Math.PI * 0.75);
  const boost = clamped >= 0;
  const sign = boost ? 1 : -1;
  const tip = `${(11 + sign * 8 * Math.sin(theta)).toFixed(2)} ${(11 - 8 * Math.cos(theta)).toFixed(2)}`;
  const fill = boost ? `M11 3 A8 8 0 0 1 ${tip}` : `M${tip} A8 8 0 0 1 11 3`;
  const color = boost ? (dark ? "var(--boost-dark)" : "var(--boost)") : dark ? "#9db8d2" : "var(--cut)";
  return (
    <svg width="26" height="26" viewBox="0 0 22 22" fill="none">
      <line x1="11" y1="1.5" x2="11" y2="4" stroke={dark ? "#55503f" : "var(--line-strong)"} strokeWidth="1.2" />
      <path d="M5.34 16.66 A8 8 0 1 1 16.66 16.66" stroke={dark ? "#3a3e36" : "var(--line)"} strokeWidth="3" />
      {Math.abs(clamped) > 0.05 && <path d={fill} stroke={color} strokeWidth="3.5" strokeLinecap="round" />}
    </svg>
  );
}

type PresetsState = { presets: string[]; active: string | null };

export default function App() {
  const [state, setState] = useState<EqState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [presets, setPresets] = useState<PresetsState>({ presets: [], active: null });
  const [menuOpen, setMenuOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const noticeTimer = useRef<number | null>(null);

  const showNotice = (msg: string) => {
    setNotice(msg);
    if (noticeTimer.current != null) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(null), 6000);
  };
  const svgRef = useRef<SVGSVGElement>(null);
  const stateRef = useRef<EqState | null>(null);
  stateRef.current = state;
  const dragging = useRef(false);
  const pushTimer = useRef<number | null>(null);

  const refresh = () =>
    invoke<EqState>("eq_state")
      .then((s) => {
        setState(s);
        setError(null);
      })
      .catch((e) => setError(String(e)));

  const refreshPresets = () =>
    invoke<PresetsState>("presets_state").then(setPresets).catch(() => {});

  useEffect(() => {
    refresh();
    refreshPresets();
    // Push-based updates from the Rust config watcher — no polling. Ignored
    // mid-drag (our own writes fire it; the drag state is fresher).
    const unlisten = listen("apo-config-changed", () => {
      if (!dragging.current) refresh();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const presetAction = (p: Promise<EqState>) =>
    p.then((s) => {
      setState(s);
      refreshPresets();
      setMenuOpen(false);
      setNewName("");
    }).catch((e) => showNotice(String(e)));

  const switchPreset = (name: string | null) =>
    presetAction(invoke<EqState>("preset_switch", { name }));
  const createPreset = (fromLive: boolean) => {
    let name = newName.trim();
    if (!name) {
      // No typed name → generate one; the button must always do something.
      const base = fromLive ? "Live copy" : "New preset";
      name = base;
      for (let n = 2; presets.presets.includes(name); n++) name = `${base} ${n}`;
    }
    presetAction(invoke<EqState>("preset_create", { name, fromLive }));
  };
  const duplicatePreset = (from: string) => {
    invoke<PresetsState>("preset_duplicate", { from, to: `${from} copy` })
      .then(setPresets)
      .catch((e) => showNotice(String(e)));
  };
  const copyFromSource = (source: string) => {
    const stem = source.replace(/\.txt$/i, "");
    let name = `${stem} copy`;
    for (let n = 2; presets.presets.includes(name); n++) name = `${stem} copy ${n}`;
    invoke<PresetsState>("preset_copy_from_source", { source, name })
      .then((p) => {
        setPresets(p);
        showNotice(`copied to “${name}” — select it in the list to activate`);
      })
      .catch((e) => showNotice(String(e)));
  };
  const deletePreset = (name: string) =>
    presetAction(invoke<EqState>("preset_delete", { name }));

  /** Send Fletcher's own chain to the backend (throttled during drags). */
  const pushChain = (filters: EqFilter[], immediate = false) => {
    const own = filters
      .filter((f) => f.sourceFile === OWN_FILE)
      .map(({ enabled, kind, fcHz, gainDb, q }) => ({ enabled, kind, fcHz, gainDb, q }));
    const send = () =>
      invoke<EqState>("set_fletcher_chain", { filters: own })
        .then((s) => {
          // Mid-drag, the response reflects an already-stale position; letting
          // it win snaps the handle back and forth under the cursor. Keep the
          // fresh local values for every filter, take the server's curves.
          const local = stateRef.current;
          if (dragging.current && local) {
            setState({
              ...s,
              filters: s.filters.map((f, i) =>
                local.filters[i]
                  ? {
                      ...f,
                      fcHz: local.filters[i].fcHz,
                      gainDb: local.filters[i].gainDb,
                      q: local.filters[i].q,
                    }
                  : f,
              ),
            });
          } else {
            setState(s);
          }
        })
        .catch((e) => showNotice(String(e)));
    if (immediate) {
      if (pushTimer.current != null) window.clearTimeout(pushTimer.current);
      pushTimer.current = null;
      send();
    } else if (pushTimer.current == null) {
      pushTimer.current = window.setTimeout(() => {
        pushTimer.current = null;
        send();
      }, 60);
    }
  };

  // ---- undo/redo: snapshots of Fletcher's own chain, gesture-coalesced ----
  type ChainSnap = { enabled: boolean; kind: string; fcHz: number; gainDb: number; q: number }[];
  const undoStack = useRef<ChainSnap[]>([]);
  const redoStack = useRef<ChainSnap[]>([]);
  const lastRecord = useRef(0);

  const ownSnap = (): ChainSnap =>
    (stateRef.current?.filters ?? [])
      .filter((f) => f.sourceFile === OWN_FILE)
      .map(({ enabled, kind, fcHz, gainDb, q }) => ({ enabled, kind, fcHz, gainDb, q }));

  /** Record the pre-change state. Changes within 500 ms coalesce into one step. */
  const recordHistory = (force = false) => {
    const now = Date.now();
    if (!force && now - lastRecord.current < 500) return;
    lastRecord.current = now;
    undoStack.current.push(ownSnap());
    if (undoStack.current.length > 200) undoStack.current.shift();
    redoStack.current = [];
  };

  const applySnap = (snap: ChainSnap) => {
    const cur = stateRef.current;
    if (!cur) return;
    const foreign = cur.filters.filter((f) => f.sourceFile !== OWN_FILE);
    const own: EqFilter[] = snap.map((s) => ({
      ...s,
      responseDb: [],
      sourceFile: OWN_FILE,
    }));
    const filters = [...foreign, ...own];
    setState({ ...cur, filters });
    setSelected(null);
    pushChain(filters, true);
  };

  const undo = () => {
    const snap = undoStack.current.pop();
    if (snap == null) return;
    redoStack.current.push(ownSnap());
    lastRecord.current = 0;
    applySnap(snap);
  };

  const redo = () => {
    const snap = redoStack.current.pop();
    if (snap == null) return;
    undoStack.current.push(ownSnap());
    lastRecord.current = 0;
    applySnap(snap);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return;
      if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === "z") {
        e.preventDefault();
        undo();
      } else if ((e.ctrlKey && e.key.toLowerCase() === "y") || (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "z")) {
        e.preventDefault();
        redo();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const mutateFilter = (i: number, patch: Partial<EqFilter>, immediate = false) => {
    const cur = stateRef.current;
    if (!cur) return;
    if (cur.filters[i]?.sourceFile === OWN_FILE) recordHistory();
    const filters = cur.filters.map((f, j) => (j === i ? { ...f, ...patch } : f));
    setState({ ...cur, filters });
    pushChain(filters, immediate);
  };

  const deleteFilter = (i: number) => {
    const cur = stateRef.current;
    if (!cur) return;
    recordHistory(true);
    const filters = cur.filters.filter((_, j) => j !== i);
    setState({ ...cur, filters });
    setSelected(null);
    pushChain(filters, true);
  };

  const addFilter = () => {
    const cur = stateRef.current;
    if (!cur) return;
    recordHistory(true);
    const fresh: EqFilter = {
      enabled: true,
      kind: "PK",
      fcHz: 1000,
      gainDb: 0,
      q: 1,
      responseDb: cur.freqs.map(() => 0),
      sourceFile: OWN_FILE,
    };
    const filters = [...cur.filters, fresh];
    setState({ ...cur, filters });
    setSelected(filters.length - 1);
    pushChain(filters, true);
  };

  const ordered = useMemo(() => {
    if (!state) return [];
    return state.filters.map((f, i) => ({ f, i })).sort((a, b) => a.f.fcHz - b.f.fcHz);
  }, [state]);

  const sel = selected != null && state ? state.filters[selected] : null;


  const autoRange = useMemo(() => {
    if (!state) return 12;
    const peaks = [
      ...state.sumDb.map((db) => Math.abs(db - state.preampDb)),
      ...state.filters.filter((f) => f.enabled).map((f) => Math.abs(f.gainDb)),
    ];
    const maxAbs = peaks.length ? Math.max(...peaks) : 0;
    return Math.max(12, Math.ceil((maxAbs + 3) / 3) * 3);
  }, [state]);

  // While dragging the scale is frozen (no moving target); hitting the edge
  // doubles it in one jump; releasing re-engages auto-fit.
  const [dragRange, setDragRange] = useState<number | null>(null);
  const dbRange = dragRange ?? autoRange;

  const yOf = (db: number) => GH / 2 - (db / dbRange) * (GH / 2 - 30);
  const dbOf = (y: number) => ((GH / 2 - y) / (GH / 2 - 30)) * dbRange;

  const gridSteps = useMemo(() => {
    const minor: number[] = [];
    const major: number[] = [];
    for (let db = 3; db < dbRange; db += 3) (db % 6 === 0 ? major : minor).push(db, -db);
    return { minor, major };
  }, [dbRange]);
  const labelSteps = useMemo(() => {
    const step = dbRange > 18 ? 6 : 3;
    const out: number[] = [0];
    for (let db = step; db < dbRange; db += step) out.push(db, -db);
    return out;
  }, [dbRange]);

  // The flag sits above everything drawn: the curve's highest point and the
  // highest handle, whichever is taller. It never covers the plot.
  const flagY = useMemo(() => {
    if (!state) return 6;
    const ys = [
      ...state.sumDb.map((db) => yOf(db - state.preampDb)),
      ...state.filters.map((f) => yOf(f.gainDb)),
    ];
    const top = ys.length ? Math.min(...ys) : GH / 2;
    return Math.max(4, top - 34);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state, dbRange]);

  const graphPoint = (e: React.PointerEvent) => {
    const rect = svgRef.current!.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * GW;
    const y = ((e.clientY - rect.top) / rect.height) * GH;
    return { f: fOf(Math.max(0, Math.min(GW, x))), db: dbOf(y) };
  };

  const startDrag = (i: number) => (e: React.PointerEvent) => {
    if (state?.filters[i]?.sourceFile !== OWN_FILE) {
      setSelected(i);
      return;
    }
    setSelected(i);
    dragging.current = true;
    setDragRange(dbRange); // freeze the scale for the whole drag
    (e.target as Element).setPointerCapture(e.pointerId);
  };

  const onDragMove = (i: number) => (e: React.PointerEvent) => {
    if (!dragging.current || selected !== i) return;
    const { f, db } = graphPoint(e);
    const clamped = Math.max(-dbRange, Math.min(dbRange, db));
    if (Math.abs(clamped) >= dbRange * 0.95 && dbRange < 48) {
      setDragRange(dbRange * 2); // hit the peak → double, in one jump
    }
    mutateFilter(i, {
      fcHz: +f.toFixed(f < 100 ? 1 : 0),
      gainDb: +clamped.toFixed(1),
    });
  };

  const endDrag = (_i: number) => () => {
    if (!dragging.current) return;
    dragging.current = false;
    setDragRange(null); // release → auto-fit
    const cur = stateRef.current;
    if (cur) pushChain(cur.filters, true);
  };

  const onWheelQ = (i: number) => (e: React.WheelEvent) => {
    const f = state?.filters[i];
    if (!f || f.sourceFile !== OWN_FILE) return;
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    mutateFilter(i, { q: +Math.max(0.05, Math.min(50, f.q * factor)).toFixed(2) }, true);
  };

  return (
    <div className="frame">
      <header>
        <span className="wordmark">FLETCHER</span>
        <nav>
          <span className="tab active">EQ</span>
          <span className="tab disabled" title="Blind tests and statistics — coming with the test engine">LISTENING LAB</span>
          <span className="tab disabled" title="Annotate tracks, build clip libraries — coming with the track engine">CLIP STUDIO</span>
          <span className="tab advanced" title="Measure and match headphones. Advanced: needs a measurement microphone.">FINGERPRINTS</span>
          <span className="tab disabled" title="Coming soon">SETTINGS</span>
        </nav>
        <span className="spacer" />
        <span className="device-chip">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4">
            <path d="M3 6v4h3l4 3V3L6 6H3z" />
            <path d="M12 6c1 1 1 3 0 4" />
          </svg>
          {state?.deviceName ?? "no device"}
        </span>
      </header>

      {notice && (
        <div className="notice" onClick={() => setNotice(null)}>
          {notice}
          <span className="dim-sm"> · dismiss</span>
        </div>
      )}

      {error && (
        <section className="alert">
          <h2>Equalizer APO problem</h2>
          <p>{error}</p>
          <p className="dim">
            Install it from sourceforge.net/projects/equalizerapo, enable it for your
            output device, then <button onClick={refresh}>retry</button>.
          </p>
        </section>
      )}

      {state && (
        <>
          <div className="preset-line">
            <span className="preset-wrap">
              <span className="preset-chip" onClick={() => setMenuOpen((o) => !o)}>
                <b>{presets.active ?? "No preset"}</b>
                <span className="dim-sm">{state.sourceFiles.join(" + ")}</span>
                <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6">
                  <path d="M4 6l4 4 4-4" />
                </svg>
              </span>
              {menuOpen && (
                <div className="preset-menu">
                  <div
                    className={`preset-row ${presets.active == null ? "current" : ""}`}
                    onClick={() => switchPreset(null)}
                  >
                    <span>Flat — no Fletcher filters</span>
                  </div>
                  {state.includes
                    .filter((inc) => inc.toLowerCase() !== OWN_FILE)
                    .map((inc) => {
                      const count = state.filters.filter(
                        (f) => f.sourceFile === inc && f.enabled,
                      ).length;
                      return (
                        <div
                          key={inc}
                          className={`preset-row external ${count ? "" : "inactive"}`}
                          title={`External — managed by another tool (${inc}). Turn it on or off there, or duplicate From live to make it yours.`}
                        >
                          <span>{inc.replace(/\.txt$/i, "")}</span>
                          <span className="ext-badge">external</span>
                          <span className="spacer" />
                          <span className="dim-sm">
                            {count ? `${count} filters on` : "inactive"}
                          </span>
                          <span
                            className="row-act"
                            title="copy this chain into your presets (does not change it or switch to it)"
                            onClick={(e) => {
                              e.stopPropagation();
                              copyFromSource(inc);
                            }}
                          >
                            ⧉
                          </span>
                        </div>
                      );
                    })}
                  {presets.presets.map((p) => (
                    <div
                      key={p}
                      className={`preset-row ${presets.active === p ? "current" : ""}`}
                      onClick={() => switchPreset(p)}
                    >
                      <span>{p}</span>
                      <span className="spacer" />
                      <span
                        className="row-act"
                        title="duplicate"
                        onClick={(e) => {
                          e.stopPropagation();
                          duplicatePreset(p);
                        }}
                      >
                        ⧉
                      </span>
                      <span
                        className="row-act"
                        title="delete"
                        onClick={(e) => {
                          e.stopPropagation();
                          deletePreset(p);
                        }}
                      >
                        ×
                      </span>
                    </div>
                  ))}
                  <div className="preset-new">
                    <input
                      placeholder="name (optional)…"
                      autoFocus
                      value={newName}
                      onChange={(e) => setNewName(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && createPreset(true)}
                    />
                    <button onClick={() => createPreset(true)} title="copy everything currently audible — including filters owned by Peace — into an editable preset">
                      From live
                    </button>
                    <button onClick={() => createPreset(false)}>Empty</button>
                  </div>
                </div>
              )}
            </span>
            <span className="dim-sm">
              drag handles · scroll for Q · locked filters: duplicate from live to edit
            </span>
            <span className="spacer" />
            {state.filters.some((f) => f.enabled && f.sourceFile !== OWN_FILE) &&
              state.filters.some((f) => f.enabled && f.sourceFile === OWN_FILE) && (
                <span className="warn-chip" title="Filters from another tool (e.g. Peace) are active alongside Fletcher's — you may be hearing both EQs stacked.">
                  ⚠ another EQ is also active
                </span>
              )}
            <span className="mono dim-sm">auto preamp</span>
            <span className="mono preamp-val">{fmtGain(state.preampDb)} dB</span>
          </div>

          <div className="graph-panel">
            <svg viewBox={`0 0 ${GW} ${GH}`} className="graph" ref={svgRef}>
              {gridSteps.minor.map((db) => (
                <line key={`n${db}`} x1={0} x2={GW} y1={yOf(db)} y2={yOf(db)} className="grid-minor" />
              ))}
              {gridSteps.major.map((db) => (
                <line key={`j${db}`} x1={0} x2={GW} y1={yOf(db)} y2={yOf(db)} className="grid-major" />
              ))}
              {[30, 50, 100, 200, 500, 1000, 2000, 5000, 10000].map((f) => (
                <line key={f} x1={xOf(f)} x2={xOf(f)} y1={0} y2={GH - 24} className="grid-vert" />
              ))}
              <line x1={0} x2={GW} y1={yOf(0)} y2={yOf(0)} className="grid-zero" />

              {sel && sel.enabled && (
                <path d={pathFrom(state.freqs, sel.responseDb, yOf)} className={`sel-curve ${sel.gainDb >= 0 ? "boost" : "cut"}`} />
              )}
              <path d={pathFrom(state.freqs, state.sumDb.map((db) => db - state.preampDb), yOf)} className="sum-curve" />

              {state.filters.map((f, i) => (
                <circle
                  key={i}
                  cx={xOf(f.fcHz)}
                  cy={yOf(f.gainDb)}
                  r={i === selected ? 7 : 5}
                  className={`handle ${f.gainDb >= 0 ? "boost" : "cut"} ${i === selected ? "selected" : ""} ${
                    f.sourceFile === OWN_FILE ? "editable" : "locked"
                  } ${f.enabled ? "" : "bypassed"}`}
                  onPointerDown={startDrag(i)}
                  onPointerMove={onDragMove(i)}
                  onPointerUp={endDrag(i)}
                  onWheel={onWheelQ(i)}
                >
                  <title>
                    {`${f.enabled ? "" : "bypassed · "}${
                      f.sourceFile === OWN_FILE ? "drag to move · scroll for Q" : `managed by ${f.sourceFile}`
                    }`}
                  </title>
                </circle>
              ))}

              {sel && (
                <g
                  className="flag"
                  transform={`translate(${Math.max(4, Math.min(xOf(sel.fcHz) - 94, GW - 216))}, ${flagY})`}
                >
                  <rect width="212" height="24" />
                  <text x="9" y="16">
                    {`${sel.kind} ${fmtHz(sel.fcHz)} Hz ${fmtGain(sel.gainDb)} dB Q ${sel.q}${sel.enabled ? "" : " · off"}`}
                  </text>
                </g>
              )}

              <g className="axis">
                {[30, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000].map((f) => (
                  <text key={f} x={xOf(f) - 8} y={GH - 8}>{fmtHz(f)}</text>
                ))}
                {labelSteps.map((db) => (
                  <text key={db} x={6} y={yOf(db) + 4}>{db === 0 ? "0 dB" : fmtGain(db)}</text>
                ))}
              </g>
            </svg>
          </div>

          <div className="strip">
            <div className="cell pre">
              <span className="cell-type mono dim-sm">PRE · AUTO</span>
              <GainGauge gainDb={state.preampDb} />
              <span className="cell-gain mono">{fmtGain(state.preampDb)}</span>
              <span className="cell-fc mono">amp</span>
              <span className="cell-q">no clip</span>
            </div>
            {ordered.map(({ f, i }) => {
              const isSel = i === selected;
              const boost = f.gainDb >= 0;
              const editable = f.sourceFile === OWN_FILE;
              return (
                <div
                  key={i}
                  className={`cell ${isSel ? "selected" : ""} ${f.enabled ? "" : "off"} ${editable ? "" : "locked"}`}
                  onClick={() => setSelected(i)}
                  onWheel={onWheelQ(i)}
                  title={editable ? undefined : `managed by ${f.sourceFile} — read-only`}
                >
                  <span className="cell-type">
                    <span
                      className={`toggle-dot ${f.enabled ? "on" : ""}`}
                      title={editable ? (f.enabled ? "click: bypass this filter" : "click: re-enable") : undefined}
                      onClick={(e) => {
                        e.stopPropagation();
                        if (editable) mutateFilter(i, { enabled: !f.enabled }, true);
                      }}
                    />
                    {isSel && editable ? (
                      <select
                        className="type-select"
                        value={f.kind}
                        onClick={(e) => e.stopPropagation()}
                        onChange={(e) => mutateFilter(i, { kind: e.target.value }, true)}
                      >
                        {KINDS.map((k) => (
                          <option key={k} value={k}>{k}</option>
                        ))}
                      </select>
                    ) : (
                      <span className="mono type-name">{f.kind}</span>
                    )}
                    <TypeGlyph kind={f.kind} boost={boost} dark={isSel} />
                  </span>
                  <GainGauge gainDb={f.gainDb} dark={isSel} />
                  <span className={`cell-gain mono ${boost ? "boost" : "cut"} ${isSel ? "on-dark" : ""}`}>
                    {fmtGain(f.gainDb)}
                  </span>
                  <span className="cell-fc mono">{fmtHz(f.fcHz)}</span>
                  <span className="cell-q">Q {f.q}</span>
                  {isSel && editable && (
                    <span
                      className="cell-delete"
                      title="delete filter"
                      onClick={(e) => {
                        e.stopPropagation();
                        deleteFilter(i);
                      }}
                    >
                      ×
                    </span>
                  )}
                </div>
              );
            })}
            <div className="cell add" title="add a filter to fletcher.txt" onClick={addFilter}>
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
                <path d="M8 3v10M3 8h10" />
              </svg>
            </div>
          </div>
        </>
      )}

      <footer className="ab-bar">
        <div className="ab-toggle">
          <span className="ab-side active">A · Live config</span>
          <span className="ab-side">B · Flat</span>
        </div>
        <span className="mono dim-sm">Ctrl·Shift·A</span>
        <span className="matched mono">
          <span className="dot" />
          levels matched
        </span>
        <span className="spacer" />
        <button className="ghost" disabled title="Track engine — Phase 3">
          Load a track
        </button>
        <button className="primary" disabled title="Test engine — Phase 2">
          Blind test
        </button>
      </footer>
    </div>
  );
}
