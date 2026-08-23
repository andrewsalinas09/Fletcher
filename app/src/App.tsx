import { useEffect, useMemo, useState } from "react";
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
};

type EqState = {
  deviceName: string | null;
  preampDb: number;
  freqs: number[];
  sumDb: number[];
  filters: EqFilter[];
  sourceFiles: string[];
};

// ---- graph geometry (matches design/Main.dc.html v5) ----
const GW = 1090;
const GH = 330;
const FMIN = 20;
const FMAX = 20000;

const xOf = (f: number) =>
  ((Math.log10(f) - Math.log10(FMIN)) / (Math.log10(FMAX) - Math.log10(FMIN))) * GW;

const fmtHz = (hz: number) =>
  hz >= 1000 ? `${+(hz / 1000).toFixed(2)}k` : `${+hz.toFixed(0)}`;
const fmtGain = (g: number) => `${g > 0 ? "+" : ""}${+g.toFixed(1)}`;

function pathFrom(freqs: number[], dbs: number[], yOf: (db: number) => number): string {
  return dbs
    .map((db, i) => `${i === 0 ? "M" : "L"}${xOf(freqs[i]).toFixed(1)} ${yOf(db).toFixed(1)}`)
    .join(" ");
}

// ---- type glyphs: tiny curve icons per filter family ----
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
      default: // PK, AP
        return boost ? "M1 10 C4 10 5 4 8 4 C11 4 12 10 15 10" : "M1 3 C4 3 5 9 8 9 C11 9 12 3 15 3";
    }
  })();
  return (
    <svg width="18" height="12" viewBox="0 0 16 12" fill="none" stroke={color} strokeWidth="1.5">
      <path d={d} />
    </svg>
  );
}

// ---- arc-fill gain gauge ----
function GainGauge({ gainDb, dark }: { gainDb: number; dark?: boolean }) {
  const clamped = Math.max(-12, Math.min(12, gainDb));
  const theta = (Math.abs(clamped) / 12) * (Math.PI * 0.75); // up to 135°
  const px = (ang: number, sign: number) => 11 + sign * 8 * Math.sin(ang);
  const py = (ang: number) => 11 - 8 * Math.cos(ang);
  const boost = clamped >= 0;
  const sign = boost ? 1 : -1;
  const tip = `${px(theta, sign).toFixed(2)} ${py(theta).toFixed(2)}`;
  const fill = boost ? `M11 3 A8 8 0 0 1 ${tip}` : `M${tip} A8 8 0 0 1 11 3`;
  const color = boost ? (dark ? "var(--boost-dark)" : "var(--boost)") : dark ? "#9db8d2" : "var(--cut)";
  return (
    <svg width="26" height="26" viewBox="0 0 22 22" fill="none">
      <line x1="11" y1="1.5" x2="11" y2="4" stroke={dark ? "#55503f" : "var(--line-strong)"} strokeWidth="1.2" />
      <path d="M5.34 16.66 A8 8 0 1 1 16.66 16.66" stroke={dark ? "#3a3e36" : "var(--line)"} strokeWidth="3" />
      {Math.abs(clamped) > 0.05 && (
        <path d={fill} stroke={color} strokeWidth="3.5" strokeLinecap="round" />
      )}
    </svg>
  );
}

export default function App() {
  const [state, setState] = useState<EqState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<number | null>(null);

  const refresh = () =>
    invoke<EqState>("eq_state")
      .then((s) => {
        setState(s);
        setError(null);
        setSelected((sel) =>
          sel != null && sel < s.filters.length ? sel : s.filters.length ? 0 : null,
        );
      })
      .catch((e) => setError(String(e)));

  useEffect(() => {
    refresh();
    // Push-based updates: the Rust watcher emits whenever any config file
    // changes on disk (Peace, hand edits, Fletcher itself). No polling.
    const unlisten = listen("apo-config-changed", () => refresh());
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const ordered = useMemo(() => {
    if (!state) return [];
    return state.filters
      .map((f, i) => ({ f, i }))
      .sort((a, b) => a.f.fcHz - b.f.fcHz);
  }, [state]);

  const sel = selected != null && state ? state.filters[selected] : null;

  // Dynamic vertical range: enough headroom for the loudest feature,
  // snapped to 3 dB steps, never tighter than ±12.
  const dbRange = useMemo(() => {
    if (!state) return 12;
    const peaks = [
      ...state.sumDb.map((db) => Math.abs(db - state.preampDb)),
      ...state.filters.filter((f) => f.enabled).map((f) => Math.abs(f.gainDb)),
    ];
    const maxAbs = peaks.length ? Math.max(...peaks) : 0;
    return Math.max(12, Math.ceil((maxAbs + 3) / 3) * 3);
  }, [state]);

  const yOf = (db: number) => GH / 2 - (db / dbRange) * (GH / 2 - 30);
  const gridSteps = useMemo(() => {
    const minor: number[] = [];
    const major: number[] = [];
    for (let db = 3; db < dbRange; db += 3) {
      (db % 6 === 0 ? major : minor).push(db, -db);
    }
    return { minor, major };
  }, [dbRange]);
  const labelSteps = useMemo(() => {
    const step = dbRange > 18 ? 6 : 3;
    const out: number[] = [0];
    for (let db = step; db < dbRange; db += step) out.push(db, -db);
    return out;
  }, [dbRange]);

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
            <span className="preset-chip">
              <b>Live APO config</b>
              <span className="dim-sm">{state.sourceFiles.join(" + ")}</span>
            </span>
            <span className="spacer" />
            <span className="mono dim-sm">auto preamp</span>
            <span className="mono preamp-val">{fmtGain(state.preampDb)} dB</span>
            <button onClick={refresh}>Refresh</button>
          </div>

          <div className="graph-panel">
            <svg viewBox={`0 0 ${GW} ${GH}`} className="graph">
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

              {state.filters.map((f, i) =>
                f.enabled ? (
                  <circle
                    key={i}
                    cx={xOf(f.fcHz)}
                    cy={yOf(f.gainDb)}
                    r={i === selected ? 7 : 5}
                    className={`handle ${f.gainDb >= 0 ? "boost" : "cut"} ${i === selected ? "selected" : ""}`}
                    onClick={() => setSelected(i)}
                  />
                ) : null,
              )}

              {sel && (
                <g className="flag" transform={`translate(${Math.min(xOf(sel.fcHz) + 16, GW - 200)}, ${Math.max(yOf(sel.gainDb) - 34, 6)})`}>
                  <rect width="188" height="24" />
                  <text x="9" y="16">{`${sel.kind} ${fmtHz(sel.fcHz)} Hz ${fmtGain(sel.gainDb)} dB Q ${sel.q}`}</text>
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
              return (
                <div
                  key={i}
                  className={`cell ${isSel ? "selected" : ""} ${f.enabled ? "" : "off"}`}
                  onClick={() => setSelected(i)}
                >
                  <span className="cell-type">
                    <span className="mono type-name">{f.kind}</span>
                    <TypeGlyph kind={f.kind} boost={boost} dark={isSel} />
                    <svg width="8" height="8" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" className="caret">
                      <path d="M4 6l4 4 4-4" />
                    </svg>
                  </span>
                  <GainGauge gainDb={f.gainDb} dark={isSel} />
                  <span className={`cell-gain mono ${boost ? "boost" : "cut"} ${isSel ? "on-dark" : ""}`}>
                    {fmtGain(f.gainDb)}
                  </span>
                  <span className="cell-fc mono">{fmtHz(f.fcHz)}</span>
                  <span className="cell-q">Q {f.q}</span>
                </div>
              );
            })}
            <div className="cell add" title="Editing arrives with the fletcher.txt writer UI">
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
