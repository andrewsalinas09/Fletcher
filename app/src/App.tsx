import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { emit, emitTo, listen } from "@tauri-apps/api/event";
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

/** Click-to-edit numeric value: span until clicked, input until committed. */
function ValueEdit({
  display,
  value,
  onCommit,
  disabled,
  className,
}: {
  display: string;
  value: number;
  onCommit: (v: number) => void;
  disabled?: boolean;
  className?: string;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  if (editing == null) {
    return (
      <span
        className={`${className ?? ""} ${disabled ? "" : "clickable-val"}`}
        onClick={(e) => {
          if (disabled) return;
          e.stopPropagation();
          setEditing(String(value));
        }}
        title={disabled ? undefined : "click to type a value"}
      >
        {display}
      </span>
    );
  }
  const commit = () => {
    const v = parseFloat(editing.replace(",", "."));
    if (!Number.isNaN(v)) onCommit(v);
    setEditing(null);
  };
  return (
    <input
      className="value-input mono"
      autoFocus
      value={editing}
      onClick={(e) => e.stopPropagation()}
      onFocus={(e) => e.target.select()}
      onChange={(e) => setEditing(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") setEditing(null);
      }}
      onBlur={commit}
    />
  );
}

// This bundle serves both windows; the query param picks the personality.
const IS_HISTORY_WINDOW = new URLSearchParams(window.location.search).get("view") === "history";

type HistTreeNode = { id: number; parent: number | null; children: number[]; label: string; ts: number };
type HistTreeData = { nodes: HistTreeNode[]; current: number };

/** The family-tree canvas: cursor-anchored zoom, drag pan, armed deletes.
 *  Shared by the in-app panel and the pop-out window. */
function HistoryTree({
  data,
  onJump,
  onDelete,
}: {
  data: HistTreeData;
  onJump: (id: number) => void;
  onDelete: (id: number) => void;
}) {
  const [zoom, setZoom] = useState(1);
  const [armed, setArmed] = useState<{ id: number; count: number } | null>(null);
  const armedT = useRef<number | null>(null);
  const vpRef = useRef<HTMLDivElement | null>(null);
  const pan = useRef<{ x: number; y: number; sl: number; st: number } | null>(null);
  const pending = useRef<{ ox: number; oy: number; sl: number; st: number; prevZ: number } | null>(null);

  const map = useMemo(() => new Map(data.nodes.map((n) => [n.id, n])), [data]);

  const rows = useMemo(() => {
    const path = new Set<number>();
    let c: number | null | undefined = data.current;
    while (c != null) {
      path.add(c);
      c = map.get(c)?.parent;
    }
    const depth = new Map<number, number>();
    return [...map.keys()]
      .sort((a, b) => a - b)
      .map((id) => {
        const n = map.get(id)!;
        const d = n.parent == null ? 0 : (depth.get(n.parent) ?? 0) + 1;
        depth.set(id, d);
        return { id, label: n.label, ts: n.ts, depth: d, onPath: path.has(id), isCurrent: id === data.current };
      });
  }, [data, map]);

  const subCount = (id: number) => {
    let count = 0;
    const stack = [id];
    while (stack.length) {
      const n = stack.pop()!;
      count++;
      stack.push(...(map.get(n)?.children ?? []));
    }
    return count;
  };

  const requestDelete = (id: number) => {
    const count = subCount(id);
    if (count <= 1 || armed?.id === id) {
      onDelete(id);
      setArmed(null);
      return;
    }
    setArmed({ id, count });
    if (armedT.current != null) window.clearTimeout(armedT.current);
    armedT.current = window.setTimeout(() => setArmed(null), 3000);
  };

  // Wheel zoom, non-passive; the scroll correction lands after resize.
  useEffect(() => {
    const vp = vpRef.current;
    if (!vp) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = vp.getBoundingClientRect();
      const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15;
      setZoom((z) => {
        const z2 = Math.max(0.3, Math.min(3, z * factor));
        if (z2 !== z) {
          pending.current = {
            ox: e.clientX - rect.left,
            oy: e.clientY - rect.top,
            sl: vp.scrollLeft,
            st: vp.scrollTop,
            prevZ: z,
          };
        }
        return z2;
      });
    };
    vp.addEventListener("wheel", onWheel, { passive: false });
    return () => vp.removeEventListener("wheel", onWheel);
  }, []);

  useLayoutEffect(() => {
    const p = pending.current;
    const vp = vpRef.current;
    if (!p || !vp) return;
    pending.current = null;
    const ratio = zoom / p.prevZ;
    vp.scrollLeft = (p.sl + p.ox) * ratio - p.ox;
    vp.scrollTop = (p.st + p.oy) * ratio - p.oy;
  }, [zoom]);

  useLayoutEffect(() => {
    const vp = vpRef.current;
    if (!vp) return;
    vp.scrollLeft = (vp.scrollWidth - vp.clientWidth) / 2;
    vp.scrollTop = 0;
  }, []);

  if (!map.has(0)) return null;

  const NODE_W = 130;
  const LEVEL_H = 130;
  const PAD = 90;
  let leaf = 0;
  const pos = new Map<number, { x: number; y: number }>();
  const assign = (id: number, depth: number): number => {
    const n = map.get(id)!;
    let x: number;
    if (n.children.length === 0) {
      x = leaf++ * NODE_W;
    } else {
      const xs = n.children.map((c) => assign(c, depth + 1));
      x = (Math.min(...xs) + Math.max(...xs)) / 2;
    }
    pos.set(id, { x, y: depth * LEVEL_H });
    return x;
  };
  assign(0, 0);
  const maxDepth = rows.reduce((m, r) => Math.max(m, r.depth), 0);
  const W = Math.max(Math.max(leaf, 1) * NODE_W + PAD * 2, 1400);
  const H = Math.max((maxDepth + 1) * LEVEL_H + PAD * 2, 900);
  const PADX = (W - Math.max(leaf - 1, 0) * NODE_W) / 2;

  return (
    <div
      className="hist-body"
      onPointerDownCapture={(e) => {
        // Click-away disarms a pending delete confirm.
        if (!(e.target as Element).closest(".tree-del, .tree-confirm")) setArmed(null);
      }}
    >
      <div
        className="hist-viewport"
        ref={vpRef}
        onPointerDown={(e) => {
          if ((e.target as Element).closest(".tree-g")) return;
          const vp = vpRef.current;
          if (!vp) return;
          pan.current = { x: e.clientX, y: e.clientY, sl: vp.scrollLeft, st: vp.scrollTop };
          (e.currentTarget as Element).setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          const p = pan.current;
          const vp = vpRef.current;
          if (!p || !vp) return;
          vp.scrollLeft = p.sl - (e.clientX - p.x);
          vp.scrollTop = p.st - (e.clientY - p.y);
        }}
        onPointerUp={() => {
          pan.current = null;
        }}
      >
        <div className="hist-graph" style={{ width: W * zoom, height: H * zoom }}>
          <div className="hist-scale" style={{ transform: `scale(${zoom})`, width: W, height: H }}>
            <svg width={W} height={H}>
              {rows.map((r) => {
                const n = map.get(r.id);
                if (!n || n.parent == null) return null;
                const p = pos.get(n.parent)!;
                const c = pos.get(r.id)!;
                const px = p.x + PADX;
                const py = p.y + PAD;
                const cxx = c.x + PADX;
                const cyy = c.y + PAD;
                return (
                  <path
                    key={`e${r.id}`}
                    d={`M ${px} ${py + 26} C ${px} ${py + 70}, ${cxx} ${cyy - 70}, ${cxx} ${cyy - 26}`}
                    className={`hist-edge ${r.onPath ? "on" : ""}`}
                  />
                );
              })}
              {rows.map((r) => {
                const c = pos.get(r.id)!;
                const x = c.x + PADX;
                const y = c.y + PAD;
                const isArmed = armed?.id === r.id;
                return (
                  <g key={`n${r.id}`} className={`tree-g ${r.onPath ? "on" : "off"}`} onClick={() => onJump(r.id)}>
                    <circle cx={x} cy={y} r={44} fill="transparent" />
                    <circle
                      cx={x}
                      cy={y}
                      r={r.isCurrent ? 28 : 24}
                      className={`hist-node ${r.isCurrent ? "current" : ""} ${r.onPath ? "on" : ""}`}
                    />
                    <text x={x} y={y - 1} className={`tree-label ${r.isCurrent ? "inv" : ""}`}>
                      {r.label}
                    </text>
                    <text x={x} y={y + 11} className={`tree-time ${r.isCurrent ? "inv" : ""}`}>
                      {new Date(r.ts).toLocaleTimeString([], { timeStyle: "short" })}
                    </text>
                    {r.id !== 0 && !isArmed && (
                      <text
                        x={x + 26}
                        y={y - 22}
                        className="tree-del"
                        onClick={(e) => {
                          e.stopPropagation();
                          requestDelete(r.id);
                        }}
                      >
                        ×
                      </text>
                    )}
                    {isArmed && (
                      <g
                        className="tree-confirm"
                        onClick={(e) => {
                          e.stopPropagation();
                          requestDelete(r.id);
                        }}
                      >
                        <rect x={x - 52} y={y - 56} width={104} height={22} />
                        <text x={x} y={y - 41}>{`delete ${armed.count} steps?`}</text>
                      </g>
                    )}
                  </g>
                );
              })}
            </svg>
          </div>
        </div>
      </div>
      <div className="hist-hint mono">
        click a node to jump (audibly) · scroll = zoom · drag empty space = pan · × prunes (asks first for whole branches)
      </div>
    </div>
  );
}

/** The pop-out window: renders the shared tree, fed by the main window over events. */
function PopoutHistory() {
  const [data, setData] = useState<HistTreeData | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);

  const handleKey = (e: { ctrlKey: boolean; shiftKey: boolean; key: string; preventDefault: () => void }) => {
    const k = e.key.toLowerCase();
    if (e.ctrlKey && !e.shiftKey && k === "z") {
      e.preventDefault();
      emitTo("main", "hist-cmd", { type: "undo", id: 0 });
    } else if ((e.ctrlKey && k === "y") || (e.ctrlKey && e.shiftKey && k === "z")) {
      e.preventDefault();
      emitTo("main", "hist-cmd", { type: "redo", id: 0 });
    }
  };

  useEffect(() => {
    const un = listen<HistTreeData>("hist-sync", (e) => setData(e.payload));
    emitTo("main", "hist-hello", {});
    const onKey = (e: KeyboardEvent) => handleKey(e);
    // Capture phase + document: maximum chance of delivery in a secondary webview.
    document.addEventListener("keydown", onKey, true);
    const grabFocus = () => rootRef.current?.focus();
    window.addEventListener("focus", grabFocus);
    grabFocus();
    return () => {
      un.then((f) => f());
      document.removeEventListener("keydown", onKey, true);
      window.removeEventListener("focus", grabFocus);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div
      className="hist-panel full"
      ref={rootRef}
      tabIndex={0}
      onKeyDown={handleKey}
      onPointerDown={() => rootRef.current?.focus()}
      style={{ outline: "none" }}
    >
      <div className="hist-head">
        <span className="mono hist-title">HISTORY</span>
        <span className="spacer" />
        <span className="mono dim-sm">live-synced with the main window</span>
      </div>
      {data ? (
        <HistoryTree
          data={data}
          onJump={(id) => emitTo("main", "hist-cmd", { type: "jump", id })}
          onDelete={(id) => emitTo("main", "hist-cmd", { type: "del", id })}
        />
      ) : (
        <p className="dim-sm" style={{ padding: 20 }}>
          waiting for the main window…
        </p>
      )}
    </div>
  );
}

type PresetsState = { presets: string[]; active: string | null };
type AbInfo = { side: string; matchDb: number };
type Device = { id: string; name: string; isDefault: boolean };
type AutoeqEntry = { name: string; path: string; note: string };

type AbxState = {
  active: boolean;
  aName: string;
  planned: number;
  answered: number;
  audition: string;
  runningCorrect: number | null;
};
type AbxTrial = { xWasA: boolean; answeredA: boolean; correct: boolean };
type AbxResult = {
  id: string;
  aName: string;
  trials: number;
  correct: number;
  pValue: number;
  statsViewed: number[];
  log: AbxTrial[];
  startedMs: number;
};

const fmtP = (p: number) => (p < 0.001 ? "< 0.001" : `= ${p.toFixed(3)}`);
const fmtWhen = (ms: number) => new Date(ms).toLocaleString([], { dateStyle: "short", timeStyle: "short" });
const verdictOf = (r: AbxResult) =>
  r.pValue <= 0.05
    ? { text: `you heard it — ${r.correct}/${r.trials} correct · p ${fmtP(r.pValue)}`, good: true }
    : { text: `couldn't reliably tell — ${r.correct}/${r.trials} · p ${fmtP(r.pValue)} (guessing gets there ${Math.round(r.pValue * 100)}% of the time)`, good: false };

// Deep teaching copy for the tooltip layer. A lot of detail, gated behind a
// deliberate 1.5 s still-hover so it never gets in the way.
const TYPE_INFO: Record<string, { name: string; desc: string }> = {
  PK: {
    name: "Peaking bell",
    desc: "Boosts or cuts a band centered on Fc. Q sets the width — low Q (0.5–1.5) is broad and musical, high Q (4+) is surgical and audible as ringing if overdone. The workhorse: most corrections are peaks.",
  },
  LSC: {
    name: "Low shelf · corner form",
    desc: "Raises or lowers everything below Fc by the gain, flattening out at the extremes. Q sets how sharply it bends at the corner (0.7 ≈ gentle; higher overshoots slightly before settling). AutoEQ's standard shelf — bass warmth and bass cuts live here.",
  },
  LS: {
    name: "Low shelf · fixed slope",
    desc: "Like LSC but with a fixed, gentle transition and no Q control. Fewer knobs, harder to make ugly.",
  },
  HSC: {
    name: "High shelf · corner form",
    desc: "The treble mirror of LSC: everything above Fc moves by the gain, with Q shaping the corner. Taming sizzle or adding air is usually one of these.",
  },
  HS: {
    name: "High shelf · fixed slope",
    desc: "High shelf with a fixed transition and no Q control.",
  },
  LP: {
    name: "Low pass",
    desc: "Removes everything above Fc (−3 dB at Fc, falling at 12 dB/octave). Gain is ignored. Fixed 0.707 Q — no resonance.",
  },
  LPQ: {
    name: "Low pass · with Q",
    desc: "Low pass with adjustable Q: above 0.707 it resonates — a peak right at the cutoff before the roll-off. Synth-filter territory.",
  },
  HP: {
    name: "High pass",
    desc: "Removes everything below Fc at 12 dB/octave. Gain is ignored. The classic rumble/sub-sonic cleaner.",
  },
  HPQ: {
    name: "High pass · with Q",
    desc: "High pass with adjustable Q for resonance at the cutoff.",
  },
  NO: {
    name: "Notch",
    desc: "A deep, narrow kill at Fc — gain is ignored, Q sets how narrow. For hum, whistles, and ringing resonances you want gone, not reduced.",
  },
  BP: {
    name: "Band pass",
    desc: "Keeps only the band around Fc, removing everything else. Gain is ignored. Rarely for correction — great for isolating a band to hear what lives there.",
  },
  AP: {
    name: "All pass",
    desc: "Changes phase without changing level — the magnitude curve stays flat. Advanced tool for time/phase alignment; you won't hear it on its own.",
  },
};

export default function App() {
  // The history pop-out window renders only the tree (fed over events).
  // Constant per window lifetime, so the early return is hook-safe.
  if (IS_HISTORY_WINDOW) return <PopoutHistory />;
  // eslint-disable-next-line react-hooks/rules-of-hooks
  return <MainApp />;
}

function MainApp() {
  const [state, setState] = useState<EqState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [presets, setPresets] = useState<PresetsState>({ presets: [], active: null });
  const [menuOpen, setMenuOpen] = useState(false);
  const [newName, setNewName] = useState("");
  const [typeMenu, setTypeMenu] = useState<{ i: number; x: number; y: number } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const noticeTimer = useRef<number | null>(null);

  // ---- deliberate-hover tooltip layer: 1.5 s still hover, motion re-arms ----
  const [tip, setTip] = useState<{ content: React.ReactNode; x: number; y: number } | null>(null);
  const tipShown = useRef(false);
  const tipTimer = useRef<number | null>(null);
  const tipAnchor = useRef<{ x: number; y: number } | null>(null);

  const clearTip = () => {
    if (tipTimer.current != null) window.clearTimeout(tipTimer.current);
    tipTimer.current = null;
    tipAnchor.current = null;
    tipShown.current = false;
    setTip(null);
  };

  const armTip = (content: React.ReactNode, x: number, y: number) => {
    tipAnchor.current = { x, y };
    if (tipTimer.current != null) window.clearTimeout(tipTimer.current);
    tipTimer.current = window.setTimeout(() => {
      tipShown.current = true;
      setTip({ content, x, y });
    }, 1500);
  };

  const tipProps = (content: React.ReactNode) => ({
    onMouseEnter: (e: React.MouseEvent) => armTip(content, e.clientX, e.clientY),
    onMouseMove: (e: React.MouseEvent) => {
      if (tipShown.current) return; // once shown, small motion keeps it
      const a = tipAnchor.current;
      if (!a || Math.hypot(e.clientX - a.x, e.clientY - a.y) > 6) {
        armTip(content, e.clientX, e.clientY); // moved → restart the clock
      }
    },
    onMouseLeave: clearTip,
    onMouseDown: clearTip,
  });

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

  const [ab, setAb] = useState<AbInfo>({ side: "a", matchDb: 0 });
  const [devices, setDevices] = useState<Device[]>([]);
  const [deviceMenu, setDeviceMenu] = useState(false);
  const [renaming, setRenaming] = useState<{ from: string; value: string } | null>(null);
  const [aeq, setAeq] = useState("");
  const [aeqResults, setAeqResults] = useState<AutoeqEntry[]>([]);
  const [importing, setImporting] = useState(false);

  const commitRename = () => {
    if (!renaming) return;
    const to = renaming.value.trim();
    if (!to || to === renaming.from) {
      setRenaming(null);
      return;
    }
    invoke<PresetsState>("preset_rename", { from: renaming.from, to })
      .then((p) => {
        setPresets(p);
        setRenaming(null);
      })
      .catch((e) => showNotice(String(e)));
  };

  // Debounced AutoEQ search (fetches the cached index; network only when stale).
  useEffect(() => {
    if (aeq.trim().length < 2) {
      setAeqResults([]);
      return;
    }
    const t = window.setTimeout(() => {
      invoke<AutoeqEntry[]>("autoeq_search", { query: aeq })
        .then(setAeqResults)
        .catch((e) => showNotice(String(e)));
    }, 300);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [aeq]);

  const importAeq = (entry: AutoeqEntry) => {
    setImporting(true);
    invoke<EqState>("autoeq_import", { name: entry.name, path: entry.path })
      .then((s) => {
        setState(s);
        refreshPresets();
        invoke<PresetsState>("presets_state")
          .then((p) => loadOrInitHistory(p.active ?? "chain", snapOf(s.filters)))
          .catch(() => initHistory(snapOf(s.filters)));
        setMenuOpen(false);
        setAeq("");
        setAeqResults([]);
        showNotice(`imported “${entry.name}” from AutoEQ — it's your active preset now`);
      })
      .catch((e) => showNotice(String(e)))
      .finally(() => setImporting(false));
  };

  // ---- Listening Lab state ----
  const [view, setView] = useState<"eq" | "lab">("eq");
  const [abx, setAbx] = useState<AbxState | null>(null);
  const abxRef = useRef<AbxState | null>(null);
  abxRef.current = abx;
  const [abxResult, setAbxResult] = useState<AbxResult | null>(null);
  const [sessions, setSessions] = useState<AbxResult[]>([]);
  const [trials, setTrials] = useState(16);
  const [expanded, setExpanded] = useState<string | null>(null);

  const loadSessions = () =>
    invoke<AbxResult[]>("abx_sessions").then(setSessions).catch(() => {});

  const startAbx = (n?: number) => {
    invoke<AbxState>("abx_start", { trials: n ?? trials })
      .then((s) => {
        setAbx(s);
        setAbxResult(null);
      })
      .catch((e) => showNotice(String(e)));
  };

  const abxAudition = (target: "a" | "b" | "x") => {
    invoke<AbxState>("abx_audition", { target })
      .then(setAbx)
      .catch((e) => showNotice(String(e)));
  };

  const abxVote = (xIsA: boolean) => {
    invoke<{ done: boolean; state?: AbxState; result?: AbxResult }>("abx_vote", { xIsA })
      .then((r) => {
        if (r.done && r.result) {
          setAbx(null);
          setAbxResult(r.result);
          loadSessions();
          refresh();
        } else if (r.state) {
          setAbx(r.state);
        }
      })
      .catch((e) => showNotice(String(e)));
  };

  const abxReveal = () => {
    invoke<AbxState>("abx_reveal").then(setAbx).catch(() => {});
  };

  const abxCancel = () => {
    invoke("abx_cancel").finally(() => {
      setAbx(null);
      refresh();
    });
  };

  // Trial-room keyboard: a/b/x audition, arrows vote, Esc leaves.
  useEffect(() => {
    if (!abx) return;
    const onKey = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      if (k === "a" || k === "b" || k === "x") {
        e.preventDefault();
        abxAudition(k as "a" | "b" | "x");
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        abxVote(true);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        abxVote(false);
      } else if (e.key === "Escape") {
        e.preventDefault();
        abxCancel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [abx != null]);

  const openDeviceMenu = () => {
    invoke<Device[]>("devices_list").then(setDevices).catch(() => {});
    setDeviceMenu((o) => !o);
  };

  const chooseDevice = (id: string) => {
    invoke<EqState>("device_set_default", { id })
      .then((s) => {
        setState(s);
        setDeviceMenu(false);
      })
      .catch((e) => showNotice(String(e)));
  };

  const refreshPresets = () => {
    invoke<PresetsState>("presets_state").then(setPresets).catch(() => {});
    invoke<AbInfo>("ab_info").then(setAb).catch(() => {});
  };

  useEffect(() => {
    refresh();
    refreshPresets();
    // Push-based updates from the Rust config watcher — no polling. Ignored
    // mid-drag (our own writes fire it; the drag state is fresher).
    const unlisten = listen("apo-config-changed", () => {
      if (!dragging.current) {
        refresh();
        invoke<AbInfo>("ab_info").then(setAb).catch(() => {});
      }
    });
    // Hotkey / tray flips land here.
    const unlistenAb = listen<string>("ab-changed", (e) => {
      setAb((cur) => ({ ...cur, side: e.payload }));
    });
    // During a session the hotkey cycles the audition target.
    const unlistenAbx = listen<string>("abx-audition", (e) => {
      setAbx((cur) => (cur ? { ...cur, audition: e.payload } : cur));
    });
    loadSessions();
    return () => {
      unlisten.then((f) => f());
      unlistenAb.then((f) => f());
      unlistenAbx.then((f) => f());
    };
  }, []);

  const setSide = (side: "a" | "b") => {
    if (side === ab.side) return;
    invoke<EqState>("ab_set", { side })
      .then((s) => {
        setState(s);
        setAb((cur) => ({ ...cur, side }));
      })
      .catch((e) => showNotice(String(e)));
  };

  const presetAction = (p: Promise<EqState>) =>
    p.then((s) => {
      setState(s);
      refreshPresets();
      setMenuOpen(false);
      setNewName("");
      invoke<PresetsState>("presets_state")
        .then((p) => loadOrInitHistory(p.active ?? "chain", snapOf(s.filters)))
        .catch(() => initHistory(snapOf(s.filters))); // each preset keeps its own timeline
    }).catch((e) => showNotice(String(e)));

  // Root the history at the first loaded state.
  useEffect(() => {
    if (state && !hist.current) {
      invoke<PresetsState>("presets_state")
        .then((p) => loadOrInitHistory(p.active ?? "chain", snapOf(state.filters)))
        .catch(() => initHistory(snapOf(state.filters)));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state != null]);


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

  // ---- the undo graph (Q-17 v1): nodes are COMPLETED gestures ----
  // A snapshot lands only when a gesture finishes: mouse drop, Enter commit,
  // or a wheel burst settling for 250 ms. Undoing then editing doesn't discard
  // the future — it branches. Session-scoped for now.
  type ChainSnap = { enabled: boolean; kind: string; fcHz: number; gainDb: number; q: number }[];
  type HistNode = {
    id: number;
    parent: number | null;
    children: number[];
    snap: ChainSnap;
    label: string;
    ts: number;
  };
  const hist = useRef<{ nodes: Map<number, HistNode>; current: number; next: number } | null>(null);
  const [histVersion, setHistVersion] = useState(0);
  const [histOpen, setHistOpen] = useState(false);
  const wheelCommit = useRef<number | null>(null);

  const snapOf = (filters: EqFilter[]): ChainSnap =>
    filters
      .filter((f) => f.sourceFile === OWN_FILE)
      .map(({ enabled, kind, fcHz, gainDb, q }) => ({ enabled, kind, fcHz, gainDb, q }));

  const ownSnap = (): ChainSnap => snapOf(stateRef.current?.filters ?? []);

  const initHistory = (snap: ChainSnap) => {
    hist.current = {
      nodes: new Map([
        [0, { id: 0, parent: null, children: [], snap, label: "start", ts: Date.now() }],
      ]),
      current: 0,
      next: 1,
    };
    rail.current = [0];
    setHistVersion((v) => v + 1);
  };

  /** A gesture finished: record the resulting state as a new node. */
  const commitGesture = (label: string) => {
    const h = hist.current;
    if (!h) return;
    const snap = ownSnap();
    const cur = h.nodes.get(h.current)!;
    if (JSON.stringify(snap) === JSON.stringify(cur.snap)) return; // no-op gesture
    const node: HistNode = {
      id: h.next++,
      parent: cur.id,
      children: [],
      snap,
      label,
      ts: Date.now(),
    };
    cur.children.push(node.id);
    h.nodes.set(node.id, node);
    h.current = node.id;
    buildRail(node.id);
    setHistVersion((v) => v + 1);
  };

  const settleWheelGesture = () => {
    if (wheelCommit.current != null) window.clearTimeout(wheelCommit.current);
    wheelCommit.current = window.setTimeout(() => {
      wheelCommit.current = null;
      commitGesture("Q scroll");
    }, 1000);
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

  // The RAIL: clicking a node fixes the whole undo/redo track — its ancestry
  // up to root, its descent to a leaf via latest-created children. Ctrl+Z/Y
  // walk this fixed rail; only jumps and new gestures re-lay it.
  const rail = useRef<number[]>([]);

  const buildRail = (id: number) => {
    const h = hist.current;
    if (!h || !h.nodes.has(id)) return;
    const up: number[] = [];
    let c: number | null = id;
    while (c != null) {
      up.push(c);
      c = h.nodes.get(c)?.parent ?? null;
    }
    up.reverse();
    let cur = id;
    for (;;) {
      const kids = h.nodes.get(cur)?.children ?? [];
      if (!kids.length) break;
      const latest = kids[kids.length - 1];
      up.push(latest);
      cur = latest;
    }
    rail.current = up;
  };

  /** Move along the existing rail without re-laying it. */
  const moveTo = (id: number) => {
    const h = hist.current;
    const node = h?.nodes.get(id);
    if (!h || !node) return;
    h.current = id;
    applySnap(node.snap);
    setHistVersion((v) => v + 1);
  };

  /** A deliberate jump (click): move AND re-lay the rail through this node. */
  const jumpTo = (id: number) => {
    buildRail(id);
    moveTo(id);
  };

  const railIndex = (): number => {
    const h = hist.current;
    if (!h) return -1;
    let idx = rail.current.indexOf(h.current);
    if (idx < 0) {
      buildRail(h.current);
      idx = rail.current.indexOf(h.current);
    }
    return idx;
  };

  const undo = () => {
    const idx = railIndex();
    if (idx > 0) moveTo(rail.current[idx - 1]);
  };

  const redo = () => {
    const idx = railIndex();
    if (idx >= 0 && idx < rail.current.length - 1) moveTo(rail.current[idx + 1]);
  };

  /** Delete a node and its whole subtree (a branch prune). Root is immortal. */
  const deleteNode = (id: number) => {
    const h = hist.current;
    if (!h || id === 0) return;
    const node = h.nodes.get(id);
    if (!node || node.parent == null) return;
    const doomed: number[] = [];
    const stack = [id];
    while (stack.length) {
      const n = stack.pop()!;
      doomed.push(n);
      stack.push(...(h.nodes.get(n)?.children ?? []));
    }
    const parent = h.nodes.get(node.parent)!;
    parent.children = parent.children.filter((c) => c !== id);
    const wasInside = doomed.includes(h.current);
    doomed.forEach((d) => h.nodes.delete(d));
    if (wasInside) {
      h.current = parent.id;
      applySnap(parent.snap);
    }
    buildRail(h.current);
    setHistVersion((v) => v + 1);
  };

  

  // ---- history persistence: trees survive restarts; files are the export ----
  const serializeHistory = (): string | null => {
    const h = hist.current;
    if (!h) return null;
    return JSON.stringify({
      version: 1,
      current: h.current,
      nodes: [...h.nodes.values()].map((n) => ({
        id: n.id,
        parent: n.parent,
        children: [...n.children],
        label: n.label,
        ts: n.ts,
        snap: n.snap,
      })),
    });
  };

  const installHistory = (raw: string): boolean => {
    try {
      const d = JSON.parse(raw);
      if (d?.version !== 1 || !Array.isArray(d.nodes)) return false;
      const nodes = new Map<number, HistNode>();
      for (const n of d.nodes) {
        nodes.set(n.id, {
          id: n.id,
          parent: n.parent ?? null,
          children: [...(n.children ?? [])],
          snap: n.snap ?? [],
          label: String(n.label ?? "step"),
          ts: n.ts ?? Date.now(),
        });
      }
      if (!nodes.has(0)) return false;
      const next = Math.max(...nodes.keys()) + 1;
      const current = nodes.has(d.current) ? d.current : 0;
      hist.current = { nodes, current, next };
      buildRail(current);
      setHistVersion((v) => v + 1);
      return true;
    } catch {
      return false;
    }
  };

  /** Load a preset's saved tree, or root a fresh one. If the live chain has
   *  drifted from the tree's current node, append a "resumed" node. */
  const loadOrInitHistory = (name: string, currentSnap: ChainSnap) => {
    invoke<string | null>("history_load", { preset: name })
      .then((raw) => {
        if (raw && installHistory(raw)) {
          const h = hist.current!;
          const nodeSnap = h.nodes.get(h.current)!.snap;
          if (JSON.stringify(nodeSnap) !== JSON.stringify(currentSnap)) {
            const cur = h.nodes.get(h.current)!;
            const node: HistNode = {
              id: h.next++,
              parent: cur.id,
              children: [],
              snap: currentSnap,
              label: "resumed",
              ts: Date.now(),
            };
            cur.children.push(node.id);
            h.nodes.set(node.id, node);
            h.current = node.id;
            buildRail(node.id);
            setHistVersion((v) => v + 1);
          }
        } else {
          initHistory(currentSnap);
        }
      })
      .catch(() => initHistory(currentSnap));
  };

  // Debounced autosave of the tree, keyed by the active preset.
  useEffect(() => {
    if (!hist.current) return;
    const t = window.setTimeout(() => {
      const data = serializeHistory();
      if (data) {
        invoke("history_save", { preset: presets.active ?? "chain", data }).catch(() => {});
      }
    }, 400);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [histVersion, presets.active]);

  const exportHistory = async () => {
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({
        defaultPath: `${presets.active ?? "chain"} history.json`,
        filters: [{ name: "Fletcher history", extensions: ["json"] }],
      });
      if (!path) return;
      const data = serializeHistory();
      if (!data) return;
      await invoke("history_export", { path, data });
      showNotice(`history exported to ${path}`);
    } catch (e) {
      showNotice(String(e));
    }
  };

  const importHistory = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({
        multiple: false,
        filters: [{ name: "Fletcher history", extensions: ["json"] }],
      });
      if (typeof path !== "string") return;
      const raw = await invoke<string>("history_import", { path });
      if (installHistory(raw)) {
        const h = hist.current!;
        applySnap(h.nodes.get(h.current)!.snap);
        showNotice("history imported — you're on its latest node, audibly");
      } else {
        showNotice("that file isn't a Fletcher history");
      }
    } catch (e) {
      showNotice(String(e));
    }
  };

  // Serializable tree (snapshots stripped) for the shared canvas + pop-out sync.
  const treeData = useMemo<HistTreeData | null>(() => {
    const h = hist.current;
    if (!h) return null;
    return {
      nodes: [...h.nodes.values()].map((n) => ({
        id: n.id,
        parent: n.parent,
        children: [...n.children],
        label: n.label,
        ts: n.ts,
      })),
      current: h.current,
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [histVersion]);
  const treeDataRef = useRef<HistTreeData | null>(null);
  treeDataRef.current = treeData;

  // Live-sync the pop-out window; answer its hello; act on its commands.
  useEffect(() => {
    if (treeData) emit("hist-sync", treeData);
  }, [treeData]);
  useEffect(() => {
    const u1 = listen("hist-hello", () => {
      if (treeDataRef.current) emit("hist-sync", treeDataRef.current);
    });
    const u2 = listen<{ type: string; id: number }>("hist-cmd", (e) => {
      if (e.payload.type === "jump") jumpTo(e.payload.id);
      else if (e.payload.type === "del") deleteNode(e.payload.id);
      else if (e.payload.type === "undo") undo();
      else if (e.payload.type === "redo") redo();
    });
    return () => {
      u1.then((f) => f());
      u2.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const popOutHistory = async () => {
    try {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const existing = await WebviewWindow.getByLabel("history");
      if (existing) {
        await existing.show().catch(() => {});
        await existing.setFocus();
      } else {
        new WebviewWindow("history", {
          url: "index.html?view=history",
          title: "Fletcher — History",
          width: 1050,
          height: 760,
        });
      }
      setHistOpen(false);
    } catch (e) {
      showNotice(String(e));
    }
  };

  const histStats = useMemo(() => {
    const h = hist.current;
    if (!h) return { edits: 0, branches: 0 };
    let branches = 0;
    h.nodes.forEach((n) => {
      if (n.children.length > 1) branches++;
    });
    return { edits: h.nodes.size - 1, branches };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [histVersion]);

  // Filter clipboard: Ctrl+C writes the selected filter as a real APO line
  // to the OS clipboard (shareable anywhere); Ctrl+V parses clipboard text
  // through the engine — one filter, many lines, or a whole pasted preset.
  const filterClipboard = useRef<{ enabled: boolean; kind: string; fcHz: number; gainDb: number; q: number } | null>(null);
  const selectedRef = useRef<number | null>(null);
  selectedRef.current = selected;

  const copySelectedFilter = () => {
    const cur = stateRef.current;
    const i = selectedRef.current;
    const f = i != null ? cur?.filters[i] : null;
    if (!f) return;
    filterClipboard.current = {
      enabled: f.enabled,
      kind: f.kind,
      fcHz: f.fcHz,
      gainDb: f.gainDb,
      q: f.q,
    };
    const line = `Filter: ${f.enabled ? "ON" : "OFF"} ${f.kind} Fc ${f.fcHz} Hz Gain ${f.gainDb} dB Q ${f.q}`;
    navigator.clipboard?.writeText(line).catch(() => {});
    showNotice(`copied ${f.kind} ${fmtHz(f.fcHz)} Hz — Ctrl+V pastes it into any preset`);
  };

  const pasteFilters = async () => {
    let pasted: { enabled: boolean; kind: string; fcHz: number; gainDb: number; q: number }[] = [];
    try {
      const text = await navigator.clipboard.readText();
      if (text?.trim()) {
        pasted = await invoke("parse_filters", { text });
      }
    } catch {
      /* clipboard unreadable — fall through to the internal copy */
    }
    if (pasted.length === 0 && filterClipboard.current) pasted = [filterClipboard.current];
    if (pasted.length === 0) return;
    const cur = stateRef.current;
    if (!cur) return;
    const fresh: EqFilter[] = pasted.map((p) => ({
      ...p,
      responseDb: [],
      sourceFile: OWN_FILE,
    }));
    const filters = [...cur.filters, ...fresh];
    setState({ ...cur, filters });
    setSelected(filters.length - 1);
    pushChain(filters, true);
    window.setTimeout(() => commitGesture(pasted.length === 1 ? "paste filter" : `paste ${pasted.length} filters`), 0);
    showNotice(`pasted ${pasted.length} filter${pasted.length > 1 ? "s" : ""}`);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return;
      const k = e.key.toLowerCase();
      if (e.ctrlKey && !e.shiftKey && k === "z") {
        e.preventDefault();
        undo();
      } else if ((e.ctrlKey && k === "y") || (e.ctrlKey && e.shiftKey && k === "z")) {
        e.preventDefault();
        redo();
      } else if (e.ctrlKey && k === "c" && !window.getSelection()?.toString()) {
        copySelectedFilter();
      } else if (e.ctrlKey && k === "v") {
        pasteFilters();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const mutateFilter = (i: number, patch: Partial<EqFilter>, immediate = false) => {
    const cur = stateRef.current;
    if (!cur) return;
    const filters = cur.filters.map((f, j) => (j === i ? { ...f, ...patch } : f));
    setState({ ...cur, filters });
    pushChain(filters, immediate);
  };

  const deleteFilter = (i: number) => {
    const cur = stateRef.current;
    if (!cur) return;
    const filters = cur.filters.filter((_, j) => j !== i);
    setState({ ...cur, filters });
    setSelected(null);
    pushChain(filters, true);
    window.setTimeout(() => commitGesture("delete"), 0);
  };

  const addFilter = () => {
    const cur = stateRef.current;
    if (!cur) return;
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
    window.setTimeout(() => commitGesture("add filter"), 0);
  };

  const ordered = useMemo(() => {
    if (!state) return [];
    return state.filters.map((f, i) => ({ f, i })).sort((a, b) => a.f.fcHz - b.f.fcHz);
  }, [state]);

  const sel = selected != null && state ? state.filters[selected] : null;


  // ---- y scale: explicit and interaction-inert (the Pro-Q resolution) ----
  // auto re-fits ONLY at non-interactive moments; a chosen scale never moves.
  const SCALES = [
    { label: "±6", v: 6 },
    { label: "±12", v: 12 },
    { label: "±18", v: 18 },
    { label: "±30", v: 33 }, // margin so a ±30 dot isn't pinned to the pixel edge
  ];
  const [yScale, setYScale] = useState<number | "auto">(() => {
    try {
      const s = localStorage.getItem("fletcher.yscale");
      return s === null || s === "auto" ? "auto" : Number(s);
    } catch {
      return "auto";
    }
  });
  const pickScale = (v: number | "auto") => {
    setYScale(v);
    try {
      localStorage.setItem("fletcher.yscale", String(v));
    } catch {
      /* per-viewer nicety only */
    }
  };

  const autoFit = useMemo(() => {
    if (!state) return 12;
    const peaks = [
      ...state.sumDb.map((db) => Math.abs(db - state.preampDb)),
      ...state.filters.filter((f) => f.enabled).map((f) => Math.abs(f.gainDb)),
    ];
    const maxAbs = peaks.length ? Math.max(...peaks) : 0;
    return SCALES.find((s) => s.v >= maxAbs + 1.5)?.v ?? 33;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state]);

  // Freeze the mapping during a drag even in auto mode: the fit is only
  // allowed to change while nothing is being held.
  const frozenRange = useRef(12);
  if (!dragging.current) frozenRange.current = yScale === "auto" ? autoFit : yScale;
  const dbRange = frozenRange.current;

  const dbOfR = (y: number, r: number) => ((GH / 2 - y) / (GH / 2 - 30)) * r;
  const yOf = (db: number) => GH / 2 - (db / dbRange) * (GH / 2 - 30);

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

  const filterTip = (f: EqFilter) => {
    const info = TYPE_INFO[f.kind] ?? { name: f.kind, desc: "" };
    const editable = f.sourceFile === OWN_FILE;
    return (
      <div>
        <div className="t-title">{`${f.kind} — ${info.name}`}</div>
        <p>{info.desc}</p>
        <p className="t-vals mono">
          {`Fc ${fmtHz(f.fcHz)} Hz · ${fmtGain(f.gainDb)} dB · Q ${f.q}${f.enabled ? "" : " · bypassed"}`}
        </p>
        {editable ? (
          <p className="t-keys">
            drag handle — move Fc &amp; gain · scroll — Q · green dot — bypass ·
            select the cell for type dropdown &amp; delete
          </p>
        ) : (
          <p className="t-keys">
            {`Read-only: this filter lives in ${f.sourceFile}, which belongs to another
            tool. Fletcher never edits files it doesn't own — duplicate From live
            (or copy the external entry in the preset menu) to make an editable version.`}
          </p>
        )}
      </div>
    );
  };

  /** Client → SVG user coordinates via the SVG's own transform — exact under
   *  any letterboxing/scaling (a rect-ratio mapping drifts toward the edges). */
  const svgPoint = (e: React.PointerEvent) => {
    const ctm = svgRef.current?.getScreenCTM();
    if (!ctm) return { x: 0, y: 0 };
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(ctm.inverse());
    return { x: p.x, y: p.y };
  };

  const grabOffset = useRef({ dx: 0, dy: 0 });

  const startDrag = (i: number) => (e: React.PointerEvent) => {
    const f = state?.filters[i];
    if (f?.sourceFile !== OWN_FILE) {
      setSelected(i);
      return;
    }
    setSelected(i);
    dragging.current = true;
    // Constant grab offset: wherever on the dot you grabbed, that relationship
    // holds for the whole drag (a few px at most — no warping, no magic).
    const p = svgPoint(e);
    grabOffset.current = { dx: xOf(f.fcHz) - p.x, dy: yOf(f.gainDb) - p.y };
    (e.target as Element).setPointerCapture(e.pointerId);
  };

  const onDragMove = (i: number) => (e: React.PointerEvent) => {
    if (!dragging.current || selected !== i) return;
    const p = svgPoint(e);
    const x = Math.max(0, Math.min(GW, p.x + grabOffset.current.dx));
    const f = fOf(x);
    const db = dbOfR(p.y + grabOffset.current.dy, dbRange);
    const clamped = Math.max(-dbRange, Math.min(dbRange, db));
    mutateFilter(i, {
      fcHz: +f.toFixed(f < 100 ? 1 : 0),
      gainDb: +clamped.toFixed(1),
    });
  };

  const endDrag = (_i: number) => () => {
    if (!dragging.current) return;
    dragging.current = false;
    const cur = stateRef.current;
    if (cur) pushChain(cur.filters, true);
    window.setTimeout(() => commitGesture("move"), 0);
  };

  const onWheelQ = (i: number) => (e: React.WheelEvent) => {
    const f = state?.filters[i];
    if (!f || f.sourceFile !== OWN_FILE) return;
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    mutateFilter(i, { q: +Math.max(0.05, Math.min(50, f.q * factor)).toFixed(2) }, true);
    settleWheelGesture(); // the burst becomes one history node 250ms after it stops
  };

  return (
    <div className="frame">
      <header>
        <span className="wordmark">FLETCHER</span>
        <nav>
          <span className={`tab ${view === "eq" ? "active" : ""}`} onClick={() => setView("eq")}>EQ</span>
          <span className={`tab ${view === "lab" ? "active" : ""}`} onClick={() => setView("lab")}>LISTENING LAB</span>
          <span className="tab disabled" title="Annotate tracks, build clip libraries — coming with the track engine">CLIP STUDIO</span>
          <span className="tab advanced" title="Measure and match headphones. Advanced: needs a measurement microphone.">FINGERPRINTS</span>
          <span className="tab disabled" title="Coming soon">SETTINGS</span>
        </nav>
        <span className="spacer" />
        <span className="preset-wrap">
          <span
            className="device-chip"
            onClick={openDeviceMenu}
            {...tipProps(
              <div>
                <div className="t-title">Output device</div>
                <p>
                  The Windows default playback device — where all audio (and the EQ) goes.
                  Click to switch. Equalizer APO must be installed on a device for EQ to
                  apply there (APO's Configurator handles that).
                </p>
              </div>,
            )}
          >
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4">
              <path d="M3 6v4h3l4 3V3L6 6H3z" />
              <path d="M12 6c1 1 1 3 0 4" />
            </svg>
            {state?.deviceName ?? "no device"}
            <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6">
              <path d="M4 6l4 4 4-4" />
            </svg>
          </span>
          {deviceMenu && (
            <div className="preset-menu device-menu">
              {devices.map((d) => (
                <div
                  key={d.id}
                  className={`preset-row ${d.isDefault ? "current" : ""}`}
                  onClick={() => chooseDevice(d.id)}
                >
                  <span>{d.name}</span>
                  {d.isDefault && <span className="dim-sm">current</span>}
                </div>
              ))}
              {devices.length === 0 && <div className="preset-row">no devices found</div>}
            </div>
          )}
        </span>
      </header>

      {notice && (
        <div className="notice" onClick={() => setNotice(null)}>
          {notice}
          <span className="dim-sm"> · dismiss</span>
        </div>
      )}

      {abx &&
        createPortal(
          <div className="trial-room">
            <div className="trial-top mono">
              <span className="ink">ABX</span>
              <span>{`${abx.aName} vs Flat`}</span>
              <span className="spacer" />
              <span>{`trial ${Math.min(abx.answered + 1, abx.planned)} of ${abx.planned}`}</span>
              <span className="trial-leave" onClick={abxCancel}>esc · leave</span>
            </div>
            <div className="trial-center">
              <div className="trial-q">Listen freely. Then answer: <b>which one is X?</b></div>
              <div className="trial-targets">
                {(["a", "b", "x"] as const).map((t) => (
                  <div
                    key={t}
                    className={`target ${t === "x" ? "mystery" : ""} ${abx.audition === t ? "playing" : ""}`}
                    onClick={() => abxAudition(t)}
                  >
                    <span className="target-letter">{t.toUpperCase()}</span>
                    <span className="mono target-sub">
                      {abx.audition === t ? "● playing" : t === "x" ? "the mystery" : " "}
                    </span>
                  </div>
                ))}
              </div>
              <span className="mono dim-sm">press A · B · X to switch — seamless, level-matched · Ctrl·Shift·A cycles</span>
              <div className="trial-vote">
                <button className="vote-btn" onClick={() => abxVote(true)}>X is A <span className="mono dim-sm">←</span></button>
                <button className="vote-btn" onClick={() => abxVote(false)}>X is B <span className="mono dim-sm">→</span></button>
              </div>
              <div className="trial-dots">
                {Array.from({ length: abx.planned }, (_, i) => (
                  <span key={i} className={`t-dot ${i < abx.answered ? "done" : i === abx.answered ? "now" : ""}`} />
                ))}
              </div>
            </div>
            <div className="trial-foot mono">
              <span className="ok-text">● levels matched</span>
              <span>every trial recorded — replay with labels afterwards</span>
              <span className="spacer" />
              {abx.runningCorrect != null ? (
                <span>{`running score ${abx.runningCorrect}/${abx.answered}`}</span>
              ) : (
                <span className="reveal-link" onClick={abxReveal}>
                  show running score — viewing interim results can bias your remaining trials
                </span>
              )}
            </div>
          </div>,
          document.body,
        )}

      {abxResult &&
        createPortal(
          <div className="trial-room">
            <div className="trial-center result-center">
              <span className="mono dim-sm">SESSION COMPLETE</span>
              <div className={`result-verdict ${verdictOf(abxResult).good ? "good" : "meh"}`}>
                {verdictOf(abxResult).good ? "You heard it." : "You couldn't reliably tell."}
              </div>
              <div className="mono result-stats">
                {`${abxResult.correct}/${abxResult.trials} correct · p ${fmtP(abxResult.pValue)}`}
              </div>
              <p className="result-note">
                {verdictOf(abxResult).good
                  ? "Guessing alone would score this well less than 5% of the time — the difference is real to your ears."
                  : "This result is within the range of guessing. That's not failure — it's information most listeners never get."}
              </p>
              <div className="trial-vote">
                <button onClick={() => { setAbxResult(null); setExpanded(abxResult.id); setView("lab"); }}>
                  Replay labeled
                </button>
                <button onClick={() => { const n = abxResult.trials; setAbxResult(null); startAbx(n); }}>
                  {`Run again · ${abxResult.trials}`}
                </button>
                <button className="primary" onClick={() => { setAbxResult(null); setView("lab"); }}>Done</button>
              </div>
            </div>
          </div>,
          document.body,
        )}

      {typeMenu &&
        state &&
        createPortal(
          <>
            <div className="type-backdrop" onClick={() => setTypeMenu(null)} />
            <div
              className="type-menu"
              style={{
                left: Math.min(typeMenu.x, window.innerWidth - 250),
                top: Math.min(typeMenu.y, window.innerHeight - 380),
              }}
            >
              {KINDS.map((k) => {
                const info = TYPE_INFO[k];
                const cur = state.filters[typeMenu.i];
                const boost = (cur?.gainDb ?? 0) >= 0;
                return (
                  <div
                    key={k}
                    className={`type-row ${cur?.kind === k ? "current" : ""}`}
                    onClick={() => {
                      mutateFilter(typeMenu.i, { kind: k }, true);
                      setTypeMenu(null);
                      window.setTimeout(() => commitGesture(`type → ${k}`), 0);
                    }}
                    {...tipProps(
                      <div>
                        <div className="t-title">{`${k} — ${info.name}`}</div>
                        <p>{info.desc}</p>
                      </div>,
                    )}
                  >
                    <span className="mono type-code">{k}</span>
                    <span className="type-desc">{info.name}</span>
                    <span className="spacer" />
                    <TypeGlyph kind={k} boost={boost} dark={cur?.kind === k} />
                  </div>
                );
              })}
            </div>
          </>,
          document.body,
        )}

      {tip &&
        createPortal(
          <div
            className="tooltip"
            style={{
              left: Math.min(tip.x + 14, window.innerWidth - 356),
              top: Math.min(tip.y + 18, window.innerHeight - 190),
            }}
          >
            {tip.content}
          </div>,
          document.body,
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

      {view === "lab" && (
        <div className="lab">
          <div className="lab-setup">
            <span className="mono lab-label">NEW TEST</span>
            <div className="lab-field">
              <span className="mono lab-key">A</span>
              <span className="lab-val">{presets.active ?? "no preset active"}</span>
            </div>
            <div className="lab-field">
              <span className="mono lab-key">B</span>
              <span className="lab-val">{`Flat · matched ${fmtGain(ab.matchDb)} dB`}</span>
            </div>
            <div className="lab-field">
              <span className="mono lab-key">TRIALS</span>
              <div className="trials-ctl">
                {[8, 16, 24].map((n) => (
                  <span key={n} className={`scale-opt ${trials === n ? "on" : ""}`} onClick={() => setTrials(n)}>
                    {n}
                  </span>
                ))}
                <input
                  className="trials-input mono"
                  type="number"
                  min={4}
                  max={100}
                  value={trials}
                  onChange={(e) => setTrials(Math.max(4, Math.min(100, +e.target.value || 16)))}
                />
              </div>
            </div>
            <p className="dim-sm lab-note">
              You'll hear A, B, and a mystery X — switch freely, then answer which one X is.
              {" "}{trials} trials; the score stays hidden until the end unless you ask.
            </p>
            <span className="spacer" />
            <button
              className="primary lab-begin"
              disabled={!presets.active}
              title={presets.active ? undefined : "activate a preset first"}
              onClick={() => startAbx()}
            >
              Begin — enters focus mode
            </button>
          </div>

          <div className="lab-record">
            <span className="mono lab-label">THE RECORD</span>
            {sessions.length === 0 && (
              <p className="dim-sm">No sessions yet. Your first blind test writes history here.</p>
            )}
            {sessions.map((r) => {
              const v = verdictOf(r);
              return (
                <div key={r.id} className="session">
                  <div className="session-head" onClick={() => setExpanded(expanded === r.id ? null : r.id)}>
                    <span className="mono badge-abx">ABX</span>
                    <div className="session-main">
                      <div className="session-title">{`${r.aName} vs Flat`}</div>
                      <div className={`session-verdict ${v.good ? "good" : "meh"}`}>{v.text}</div>
                      {r.statsViewed.length > 0 && (
                        <div className="dim-sm">{`score viewed mid-session at trial ${r.statsViewed.join(", ")}`}</div>
                      )}
                    </div>
                    <span className="mono dim-sm">{fmtWhen(r.startedMs)}</span>
                  </div>
                  {expanded === r.id && (
                    <div className="session-detail">
                      <div className="trial-grid">
                        {r.log.map((t, i) => (
                          <span key={i} className={`trial-chip ${t.correct ? "hit" : "miss"}`}
                            title={`trial ${i + 1}: X was ${t.xWasA ? "A" : "B"}, you said ${t.answeredA ? "A" : "B"}`}>
                            {`${i + 1}·X=${t.xWasA ? "A" : "B"} ${t.correct ? "✓" : "✗"}`}
                          </span>
                        ))}
                      </div>
                      <div className="session-actions">
                        <span className="dim-sm">listen again, labels on:</span>
                        <button onClick={() => setSide("a")}>{`A · ${r.aName}`}</button>
                        <button onClick={() => setSide("b")}>B · Flat</button>
                        <span className="spacer" />
                        <button onClick={() => startAbx(r.trials)}>{`Run again · ${r.trials} trials`}</button>
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {state && view === "eq" && (
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
                    {...tipProps(
                      <div>
                        <div className="t-title">Flat</div>
                        <p>
                          Empties Fletcher's chain (fletcher.txt) so Fletcher adds nothing to the
                          signal. External EQs like Peace are unaffected — if their filters are on,
                          you'll still hear those.
                        </p>
                      </div>,
                    )}
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
                          {...tipProps(
                            <div>
                              <div className="t-title">{`${inc} — external EQ`}</div>
                              <p>
                                {`config.txt includes ${inc}, but it belongs to another tool
                                (Peace). Fletcher never edits or toggles files it doesn't own,
                                so it can't switch this on or off — do that in the other tool.`}
                              </p>
                              <p>
                                ⧉ copies its whole chain (preamp included) into your presets as an
                                editable copy — and it works even while the other tool's EQ is
                                toggled off, by reading its saved state.
                              </p>
                              <p className="t-vals mono">
                                {count ? `currently contributing ${count} filters` : "currently inactive"}
                              </p>
                            </div>,
                          )}
                        >
                          <span>{inc.replace(/\.txt$/i, "")}</span>
                          <span className="ext-badge">external</span>
                          <span className="spacer" />
                          <span className="dim-sm">
                            {count ? `${count} filters on` : "inactive"}
                          </span>
                          <span
                            className="row-act"
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
                      {...tipProps(
                        <div>
                          <div className="t-title">{p}</div>
                          <p>
                            Click to activate: this chain is written to fletcher.txt with a freshly
                            computed auto-preamp, and APO applies it instantly. While active, every
                            edit you make saves back into the preset automatically.
                          </p>
                          <p className="t-keys">⧉ duplicate · × delete (permanent — no undo)</p>
                        </div>,
                      )}
                    >
                      {renaming?.from === p ? (
                        <input
                          className="rename-input"
                          autoFocus
                          value={renaming.value}
                          onClick={(e) => e.stopPropagation()}
                          onChange={(e) => setRenaming({ from: p, value: e.target.value })}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") commitRename();
                            if (e.key === "Escape") setRenaming(null);
                          }}
                          onBlur={commitRename}
                        />
                      ) : (
                        <span>{p}</span>
                      )}
                      <span className="spacer" />
                      <span
                        className="row-act"
                        title="rename"
                        onClick={(e) => {
                          e.stopPropagation();
                          setRenaming({ from: p, value: p });
                        }}
                      >
                        ✎
                      </span>
                      <span
                        className="row-act"
                        onClick={(e) => {
                          e.stopPropagation();
                          duplicatePreset(p);
                        }}
                      >
                        ⧉
                      </span>
                      <span
                        className="row-act"
                        onClick={(e) => {
                          e.stopPropagation();
                          deletePreset(p);
                        }}
                      >
                        ×
                      </span>
                    </div>
                  ))}
                  <div className="aeq-block">
                    <span className="mono aeq-label">GET A HEADPHONE PRESET — AUTOEQ</span>
                    <input
                      className="aeq-input"
                      placeholder="search 5,000+ headphones… e.g. HD 650"
                      value={aeq}
                      onClick={(e) => e.stopPropagation()}
                      onChange={(e) => setAeq(e.target.value)}
                    />
                    {importing && <div className="preset-row"><span className="dim-sm">importing…</span></div>}
                    {!importing &&
                      aeqResults.map((r) => (
                        <div key={`${r.path}/${r.name}`} className="preset-row" onClick={() => importAeq(r)}>
                          <span>{r.name}</span>
                          <span className="spacer" />
                          <span className="dim-sm">{r.note.replace(/^by /, "")}</span>
                        </div>
                      ))}
                    {!importing && aeq.trim().length >= 2 && aeqResults.length === 0 && (
                      <div className="preset-row"><span className="dim-sm">no matches</span></div>
                    )}
                  </div>
                  <div className="preset-new">
                    <input
                      placeholder="name (optional)…"
                      value={newName}
                      onChange={(e) => setNewName(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && createPreset(true)}
                    />
                    <button onClick={() => createPreset(true)} title="copy everything currently audible — including filters owned by Peace — into an editable preset">
                      From live
                    </button>
                    <button onClick={() => createPreset(false)} title="start from flat — no filters, build your own">From flat</button>
                  </div>
                </div>
              )}
            </span>
            <span className="preset-wrap">
              <span
                className="hist-chip mono"
                onClick={() => setHistOpen((o) => !o)}
                {...tipProps(
                  <div>
                    <div className="t-title">The undo graph</div>
                    <p>
                      Every finished gesture (a drag, a typed value, a settled scroll) is a
                      node. Ctrl+Z walks up, Ctrl+Y walks down the newest branch — and
                      editing after an undo doesn't erase the future, it forks a new
                      branch. Click any node to jump straight there, audibly.
                    </p>
                  </div>,
                )}
              >
                {`⟲ ${histStats.edits}${histStats.branches ? ` · ⑂${histStats.branches}` : ""}`}
              </span>
              {histOpen &&
                treeData &&
                createPortal(
                  <>
                    <div className="hist-backdrop" onClick={() => setHistOpen(false)} />
                    <div className="hist-panel">
                      <div className="hist-head">
                        <span className="mono hist-title">{`HISTORY — ${presets.active ?? "chain"}`}</span>
                        <span className="mono dim-sm">{`${histStats.edits} steps${histStats.branches ? ` · ${histStats.branches} branch points` : ""}`}</span>
                        <span className="spacer" />
                        <button onClick={importHistory} title="load a history file — its tree replaces this one and you land on its current node">
                          import
                        </button>
                        <button onClick={exportHistory} title="save this tree (with every branch and snapshot) as a shareable file">
                          export
                        </button>
                        <button onClick={popOutHistory} title="open the history tree in its own window — stays live-synced">
                          ⇱ pop out
                        </button>
                        <span className="hist-close" onClick={() => setHistOpen(false)}>×</span>
                      </div>
                      <HistoryTree data={treeData} onJump={jumpTo} onDelete={deleteNode} />
                    </div>
                  </>,
                  document.body,
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
            <div className="scale-ctl">
              <span
                className={`scale-opt ${yScale === "auto" ? "on" : ""}`}
                onClick={() => pickScale("auto")}
                {...tipProps(
                  <div>
                    <div className="t-title">Auto scale</div>
                    <p>
                      Fits the view to your loudest filter or curve peak — but only re-fits
                      between interactions, never while you're dragging. Pick a fixed scale
                      when you want big sweeps: the mapping then never changes at all.
                    </p>
                  </div>,
                )}
              >
                auto
              </span>
              {SCALES.map((s) => (
                <span
                  key={s.v}
                  className={`scale-opt ${yScale === s.v ? "on" : ""}`}
                  onClick={() => pickScale(s.v)}
                >
                  {s.label}
                </span>
              ))}
            </div>
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
                  {...tipProps(filterTip(f))}
                />
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
                  {...tipProps(filterTip(f))}
                >
                  <span className="cell-type">
                    <span
                      className={`toggle-dot ${f.enabled ? "on" : ""}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        if (editable) {
                          mutateFilter(i, { enabled: !f.enabled }, true);
                          window.setTimeout(() => commitGesture(f.enabled ? "bypass" : "enable"), 0);
                        }
                      }}
                    />
                    <span
                      className={`type-open ${editable ? "openable" : ""}`}
                      onClick={(e) => {
                        if (!editable) return;
                        e.stopPropagation();
                        setSelected(i);
                        clearTip();
                        const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                        setTypeMenu(typeMenu?.i === i ? null : { i, x: r.left, y: r.bottom + 4 });
                      }}
                    >
                      <span className="mono type-name">{f.kind}</span>
                      <TypeGlyph kind={f.kind} boost={boost} dark={isSel} />
                      {editable && (
                        <svg width="8" height="8" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" className="caret">
                          <path d="M4 6l4 4 4-4" />
                        </svg>
                      )}
                    </span>
                  </span>
                  <GainGauge gainDb={f.gainDb} dark={isSel} />
                  <ValueEdit
                    className={`cell-gain mono ${boost ? "boost" : "cut"} ${isSel ? "on-dark" : ""}`}
                    display={fmtGain(f.gainDb)}
                    value={f.gainDb}
                    disabled={!editable}
                    onCommit={(v) => {
                      mutateFilter(i, { gainDb: Math.max(-30, Math.min(30, v)) }, true);
                      window.setTimeout(() => commitGesture("gain typed"), 0);
                    }}
                  />
                  <ValueEdit
                    className="cell-fc mono"
                    display={fmtHz(f.fcHz)}
                    value={f.fcHz}
                    disabled={!editable}
                    onCommit={(v) => {
                      mutateFilter(i, { fcHz: Math.max(10, Math.min(24000, v)) }, true);
                      window.setTimeout(() => commitGesture("Fc typed"), 0);
                    }}
                  />
                  <ValueEdit
                    className="cell-q"
                    display={`Q ${f.q}`}
                    value={f.q}
                    disabled={!editable}
                    onCommit={(v) => {
                      mutateFilter(i, { q: Math.max(0.05, Math.min(50, v)) }, true);
                      window.setTimeout(() => commitGesture("Q typed"), 0);
                    }}
                  />
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
          <span
            className={`ab-side ${ab.side === "a" ? "active" : ""}`}
            onClick={() => setSide("a")}
            {...tipProps(
              <div>
                <div className="t-title">A — your chain</div>
                <p>
                  {`The active preset (${presets.active ?? "none — Fletcher adds nothing"}).
                  Flip with Ctrl+Shift+A from anywhere — Fletcher listens even while hidden
                  in the tray.`}
                </p>
              </div>,
            )}
          >
            {`A · ${presets.active ?? "Fletcher chain"}`}
          </span>
          <span
            className={`ab-side ${ab.side === "b" ? "active" : ""}`}
            onClick={() => setSide("b")}
            {...tipProps(
              <div>
                <div className="t-title">B — flat, at the reference level</div>
                <p>
                  No filters — but not naive bypass: everything in Fletcher (flat, every
                  preset, your chain mid-edit) is normalized to one global reference
                  loudness, so flipping compares tone, not volume. Louder always sounds
                  better; matched is honest. The reference will be calibratable in
                  Settings.
                </p>
                <p className="t-vals mono">{`reference ${fmtGain(ab.matchDb)} dB`}</p>
              </div>,
            )}
          >
            B · Flat
          </span>
        </div>
        <span className="mono dim-sm">Ctrl·Shift·A</span>
        <span className="matched mono">
          <span className="dot" />
          {`matched · B ${fmtGain(ab.matchDb)} dB`}
        </span>
        <span className="spacer" />
      </footer>
    </div>
  );
}
