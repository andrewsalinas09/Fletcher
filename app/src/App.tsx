import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
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
        initHistory(snapOf(s.filters));
        refreshPresets();
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
      initHistory(snapOf(s.filters)); // a different preset is a fresh timeline
    }).catch((e) => showNotice(String(e)));

  // Root the history at the first loaded state.
  useEffect(() => {
    if (state && !hist.current) initHistory(snapOf(state.filters));
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
  const [histZoom, setHistZoom] = useState(1);
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

  const jumpTo = (id: number) => {
    const h = hist.current;
    const node = h?.nodes.get(id);
    if (!h || !node) return;
    h.current = id;
    applySnap(node.snap);
    setHistVersion((v) => v + 1);
  };

  const undo = () => {
    const h = hist.current;
    if (!h) return;
    const parent = h.nodes.get(h.current)?.parent;
    if (parent != null) jumpTo(parent);
  };

  const redo = () => {
    const h = hist.current;
    if (!h) return;
    const kids = h.nodes.get(h.current)?.children ?? [];
    if (kids.length) jumpTo(kids[kids.length - 1]); // most recent branch
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
    setHistVersion((v) => v + 1);
  };

  const histRows = useMemo(() => {
    const h = hist.current;
    if (!h) return [];
    const path = new Set<number>();
    let c: number | null | undefined = h.current;
    while (c != null) {
      path.add(c);
      c = h.nodes.get(c)?.parent;
    }
    const depth = new Map<number, number>();
    return [...h.nodes.keys()]
      .sort((a, b) => a - b)
      .map((id) => {
        const n = h.nodes.get(id)!;
        const d = n.parent == null ? 0 : (depth.get(n.parent) ?? 0) + 1;
        depth.set(id, d);
        return {
          id,
          label: n.label,
          ts: n.ts,
          depth: d,
          branchPoint: n.children.length > 1,
          onPath: path.has(id),
          isCurrent: id === h.current,
        };
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [histVersion]);

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
                (() => {
                  const h = hist.current;
                  if (!h) return null;
                  const info = new Map(histRows.map((r) => [r.id, r]));
                  const NODE_W = 84;
                  const LEVEL_H = 62;
                  const PAD = 40;
                  // Tidy family-tree layout: leaves get slots, parents center
                  // over their children.
                  let leaf = 0;
                  const pos = new Map<number, { x: number; y: number }>();
                  const assign = (id: number, depth: number): number => {
                    const n = h.nodes.get(id)!;
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
                  const maxDepth = histRows.reduce((m, r) => Math.max(m, r.depth), 0);
                  const W = Math.max(leaf, 1) * NODE_W + PAD * 2 - NODE_W / 2;
                  const H = (maxDepth + 1) * LEVEL_H + PAD + 26;
                  return (
                    <div className="preset-menu hist-menu">
                      <div
                        className="hist-viewport"
                        onWheel={(e) => {
                          if (!e.ctrlKey) return; // plain wheel scrolls/pans
                          e.preventDefault();
                          setHistZoom((z) =>
                            Math.max(0.4, Math.min(2.5, z * (e.deltaY < 0 ? 1.15 : 1 / 1.15))),
                          );
                        }}
                      >
                        <div className="hist-graph" style={{ width: W * histZoom, height: H * histZoom }}>
                          <div className="hist-scale" style={{ transform: `scale(${histZoom})`, width: W, height: H }}>
                            <svg width={W} height={H}>
                              {histRows.map((r) => {
                                const n = h.nodes.get(r.id);
                                if (!n || n.parent == null) return null;
                                const p = pos.get(n.parent)!;
                                const c = pos.get(r.id)!;
                                const px = p.x + PAD;
                                const py = p.y + PAD;
                                const cxx = c.x + PAD;
                                const cyy = c.y + PAD;
                                return (
                                  <path
                                    key={`e${r.id}`}
                                    d={`M ${px} ${py + 9} C ${px} ${py + 34}, ${cxx} ${cyy - 34}, ${cxx} ${cyy - 9}`}
                                    className={`hist-edge ${r.onPath ? "on" : ""}`}
                                  />
                                );
                              })}
                              {histRows.map((r) => {
                                const c = pos.get(r.id)!;
                                const x = c.x + PAD;
                                const y = c.y + PAD;
                                const inf = info.get(r.id)!;
                                return (
                                  <g
                                    key={`n${r.id}`}
                                    className={`tree-g ${inf.onPath ? "on" : "off"}`}
                                    onClick={() => jumpTo(r.id)}
                                  >
                                    <circle
                                      cx={x}
                                      cy={y}
                                      r={inf.isCurrent ? 9 : 7}
                                      className={`hist-node ${inf.isCurrent ? "current" : ""} ${inf.onPath ? "on" : ""}`}
                                    />
                                    <text x={x} y={y + 22} className="tree-label">
                                      {r.label}
                                    </text>
                                    <text x={x} y={y + 33} className="tree-time">
                                      {new Date(r.ts).toLocaleTimeString([], { timeStyle: "short" })}
                                    </text>
                                    {r.id !== 0 && (
                                      <text
                                        x={x + 12}
                                        y={y - 8}
                                        className="tree-del"
                                        onClick={(e) => {
                                          e.stopPropagation();
                                          deleteNode(r.id);
                                        }}
                                      >
                                        ×
                                      </text>
                                    )}
                                  </g>
                                );
                              })}
                            </svg>
                          </div>
                        </div>
                      </div>
                      <div className="hist-hint mono">
                        click a node to jump (audibly) · ctrl+scroll zoom · scroll pans · × prunes a branch
                      </div>
                    </div>
                  );
                })()}
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
