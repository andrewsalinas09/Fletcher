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

// This bundle serves every window; the query param picks the personality.
const IS_HISTORY_WINDOW = new URLSearchParams(window.location.search).get("view") === "history";
const IS_DIFF_WINDOW = new URLSearchParams(window.location.search).get("view") === "diff";
const IS_SCOPE_SPEC_WINDOW = new URLSearchParams(window.location.search).get("view") === "scope-spec";
const IS_SCOPE_FFT_WINDOW = new URLSearchParams(window.location.search).get("view") === "scope-fft";

type ChainSnap = { enabled: boolean; kind: string; fcHz: number; gainDb: number; q: number }[];
type HistTreeNode = {
  id: number;
  parent: number | null;
  children: number[];
  label: string;
  ts: number;
  snap: ChainSnap;
  note?: string;
  pinned?: boolean;
};
type HistTreeData = { nodes: HistTreeNode[]; current: number; name?: string };
type NodePatch = { label?: string; note?: string; pinned?: boolean };

/** One filter as a real APO config line — shared by filter Ctrl+C and the
 *  inspector's copy-node (one formatter, never two drifting ones). */
const apoLine = (f: ChainSnap[number]) =>
  `Filter: ${f.enabled ? "ON" : "OFF"} ${f.kind} Fc ${f.fcHz} Hz Gain ${f.gainDb} dB Q ${f.q}`;

// ---- inspector curve math (Q-24): all responses come from Rust, TS only
// subtracts. "Audible" = response + matched preamp — what actually plays.
type ChainCurvesResp = { freqs: number[]; curves: { responseDb: number[]; matchedPreampDb: number }[] };
type CurveMap = { freqs: number[]; byId: Map<number, { responseDb: number[]; preamp: number }> };

const audibleOf = (c: { responseDb: number[]; preamp: number }) => c.responseDb.map((db) => db + c.preamp);
const diffOf = (a: number[], b: number[]) => a.map((v, i) => v - (b[i] ?? 0));
const meanAbs = (xs: number[]) => (xs.length ? xs.reduce((s, x) => s + Math.abs(x), 0) / xs.length : 0);

/** Series colors for multi-node compare — legend, rings, and curves agree. */
const CMP_COLORS = ["#c85a13", "#3f6d9e", "#3d7d43", "#7d5a9e", "#a3552e", "#4f7d7d"];

type DiffRow =
  | { t: "changed"; a: ChainSnap[number]; b: ChainSnap[number] }
  | { t: "added"; b: ChainSnap[number] }
  | { t: "removed"; a: ChainSnap[number] };

/** Parametric diff base→node. Filters have no stable identity across nodes,
 *  so pair greedily: same kind + nearest log-Fc first, then leftovers at
 *  (nearly) the same Fc pair as type changes; the rest are added/removed. */
function diffChains(base: ChainSnap, node: ChainSnap): { rows: DiffRow[]; unchanged: number } {
  const usedN = new Set<number>();
  const pair = new Map<number, number>();
  const claim = (bi: number, sameKind: boolean, maxD: number) => {
    const bf = base[bi];
    let best = -1;
    let bestD = Infinity;
    node.forEach((nf, j) => {
      if (usedN.has(j)) return;
      if (sameKind && nf.kind !== bf.kind) return;
      const d = Math.abs(Math.log(nf.fcHz / bf.fcHz));
      if (d < bestD) {
        bestD = d;
        best = j;
      }
    });
    if (best >= 0 && bestD <= maxD) {
      usedN.add(best);
      pair.set(bi, best);
    }
  };
  base.forEach((_, bi) => claim(bi, true, Infinity));
  base.forEach((_, bi) => {
    if (!pair.has(bi)) claim(bi, false, 0.12);
  });
  const rows: DiffRow[] = [];
  let unchanged = 0;
  base.forEach((bf, bi) => {
    const ni = pair.get(bi);
    if (ni == null) {
      rows.push({ t: "removed", a: bf });
      return;
    }
    const nf = node[ni];
    const same =
      nf.kind === bf.kind &&
      nf.enabled === bf.enabled &&
      Math.abs(Math.log(nf.fcHz / bf.fcHz)) < 1e-6 &&
      Math.abs(nf.gainDb - bf.gainDb) < 0.005 &&
      Math.abs(nf.q - bf.q) < 0.005;
    if (same) unchanged++;
    else rows.push({ t: "changed", a: bf, b: nf });
  });
  node.forEach((nf, j) => {
    if (!usedN.has(j)) rows.push({ t: "added", b: nf });
  });
  const fcOf = (r: DiffRow) => (r.t === "removed" ? r.a.fcHz : r.b.fcHz);
  rows.sort((r1, r2) => fcOf(r1) - fcOf(r2));
  return { rows, unchanged };
}

/** Mini spectral plot for the inspector: difference curves (single diff gets
 *  boost/cut shading) over optional faint context curves. Own geometry —
 *  independent of the main graph's fixed width. */
function DiffPlot({
  freqs,
  diffs,
  faint,
  range,
  w = 300,
  h = 132,
}: {
  freqs: number[];
  diffs: { dbs: number[]; color: string }[];
  faint?: number[][];
  range: number;
  w?: number;
  h?: number;
}) {
  const PW = w;
  const PH = h;
  const px = (f: number) =>
    ((Math.log10(f) - Math.log10(FMIN)) / (Math.log10(FMAX) - Math.log10(FMIN))) * PW;
  const py = (db: number) => PH / 2 - (db / range) * (PH / 2 - 6);
  const path = (dbs: number[]) =>
    dbs
      .map(
        (db, i) =>
          `${i === 0 ? "M" : "L"}${px(freqs[i]).toFixed(1)} ${Math.max(0, Math.min(PH, py(db))).toFixed(1)}`,
      )
      .join(" ");
  const zero = py(0);
  const area = (dbs: number[]) => `${path(dbs)} L${PW} ${zero.toFixed(1)} L0 ${zero.toFixed(1)} Z`;
  return (
    <svg viewBox={`0 0 ${PW} ${PH}`} className="diff-plot">
      <clipPath id="dp-up">
        <rect x={0} y={0} width={PW} height={zero} />
      </clipPath>
      <clipPath id="dp-dn">
        <rect x={0} y={zero} width={PW} height={PH - zero} />
      </clipPath>
      {[100, 1000, 10000].map((f) => (
        <line key={f} x1={px(f)} x2={px(f)} y1={0} y2={PH} className="dp-grid" />
      ))}
      {[range / 2, -range / 2].map((db) => (
        <line key={db} x1={0} x2={PW} y1={py(db)} y2={py(db)} className="dp-grid" />
      ))}
      <line x1={0} x2={PW} y1={zero} y2={zero} className="dp-zero" />
      {faint?.map((dbs, k) => (
        <path key={`f${k}`} d={path(dbs)} className="dp-faint" />
      ))}
      {diffs.length === 1 && (
        <>
          <path d={area(diffs[0].dbs)} clipPath="url(#dp-up)" className="dp-fill boost" />
          <path d={area(diffs[0].dbs)} clipPath="url(#dp-dn)" className="dp-fill cut" />
        </>
      )}
      {diffs.map((d, k) => (
        <path key={k} d={path(d.dbs)} className="dp-line" style={{ stroke: d.color }} />
      ))}
      <text x={3} y={11} className="dp-label">{`+${range}`}</text>
      <text x={3} y={PH - 4} className="dp-label">{`−${range}`}</text>
      {[100, 1000, 10000].map((f) => (
        <text key={f} x={px(f) + 3} y={PH - 4} className="dp-label">
          {fmtHz(f)}
        </text>
      ))}
    </svg>
  );
}

/** One parametric-diff line: what changed on a filter, boost/cut colored. */
function DiffRowView({ row }: { row: DiffRow }) {
  if (row.t !== "changed") {
    const f = row.t === "added" ? row.b : row.a;
    return (
      <div className={`diff-row ${row.t}`}>
        <span className="mono d-tag">{row.t === "added" ? "+add" : "−rem"}</span>
        <TypeGlyph kind={f.kind} boost={f.gainDb >= 0} />
        <span className="mono d-txt">{`${f.kind} ${fmtHz(f.fcHz)} ${fmtGain(f.gainDb)} dB Q ${f.q}`}</span>
      </div>
    );
  }
  const { a, b } = row;
  const parts: React.ReactNode[] = [];
  if (b.kind !== a.kind) parts.push(<span key="k" className="d-delta">{`${a.kind}→${b.kind}`}</span>);
  if (Math.abs(Math.log(b.fcHz / a.fcHz)) > 1e-6)
    parts.push(<span key="f" className="d-delta">{`Fc ${fmtHz(a.fcHz)}→${fmtHz(b.fcHz)}`}</span>);
  const dg = b.gainDb - a.gainDb;
  if (Math.abs(dg) >= 0.005)
    parts.push(
      <span key="g" className={`d-delta ${dg >= 0 ? "boost" : "cut"}`}>{`${fmtGain(dg)} dB`}</span>,
    );
  if (Math.abs(b.q - a.q) >= 0.005)
    parts.push(<span key="q" className="d-delta">{`Q ${a.q}→${b.q}`}</span>);
  if (b.enabled !== a.enabled)
    parts.push(<span key="e" className="d-delta">{b.enabled ? "enabled" : "bypassed"}</span>);
  return (
    <div className="diff-row changed">
      <span className="mono d-tag">Δ</span>
      <TypeGlyph kind={b.kind} boost={b.gainDb >= 0} />
      <span className="mono d-fc">{fmtHz(b.fcHz)}</span>
      <span className="d-parts">{parts}</span>
    </div>
  );
}

/** The family-tree canvas + the node inspector (Q-24): cursor-anchored zoom,
 *  drag pan, armed deletes, and a right-side panel that answers "what is this
 *  node, and how does it differ from that one". Shared by the in-app panel
 *  and the pop-out window; the inspector never claims audibility — the blind
 *  test button is the only arbiter. */
function HistoryTree({
  data,
  onJump,
  onDelete,
  onEdit,
  onPreview,
  onAbx,
  onPromote,
  onCompare,
  onPopoutDiff,
  onGraft,
  notify,
}: {
  data: HistTreeData;
  onJump: (id: number) => void;
  onDelete: (id: number) => void;
  onEdit: (id: number, patch: NodePatch) => void;
  onPreview: (snap: ChainSnap | null) => void;
  onAbx: (aId: number, bId: number) => void;
  onPromote: (id: number) => void;
  /** Drag-graft: copy `id`'s exact sound as one clean step under `ontoId`. */
  onGraft?: (id: number, ontoId: number) => void;
  /** Reports the inspector's compare (sel/base/cmp, nulls = follow current/parent)
   *  so the difference pop-out can mirror it. */
  onCompare?: (spec: { sel: number | null; base: number | null; cmp: number[] }) => void;
  onPopoutDiff?: () => void;
  notify?: (msg: string) => void;
}) {
  const [zoom, setZoom] = useState(1);
  const [armed, setArmed] = useState<{ id: number; count: number } | null>(null);
  const armedT = useRef<number | null>(null);
  const vpRef = useRef<HTMLDivElement | null>(null);
  const pan = useRef<{ x: number; y: number; sl: number; st: number } | null>(null);
  const pending = useRef<{ ox: number; oy: number; sl: number; st: number; prevZ: number } | null>(null);

  const map = useMemo(() => new Map(data.nodes.map((n) => [n.id, n])), [data]);

  // ---- inspector state ----
  const [selId, setSelId] = useState<number | null>(null);
  const [baseSel, setBaseSel] = useState<number | null>(null); // null = parent of selected
  const [pickMode, setPickMode] = useState(false);
  const [cmp, setCmp] = useState<ReadonlySet<number>>(new Set()); // extra compare members
  const [previewing, setPreviewing] = useState<"node" | "base" | null>(null);
  const [editingLabel, setEditingLabel] = useState<string | null>(null);
  const [diffScale, setDiffScale] = useState<number | "auto">(loadDiffScale);
  const [curves, setCurves] = useState<CurveMap | null>(null);

  // A preview must never outlive the panel (or the selection that started it).
  const onPreviewRef = useRef(onPreview);
  onPreviewRef.current = onPreview;
  useEffect(() => () => onPreviewRef.current(null), []);

  // Undo/redo are keyboard clicks on the graph: whenever the current node
  // moves (Ctrl+Z/Y, pop-out commands, prunes), the inspector follows it,
  // exactly as if the node had been clicked. Only Alt+click (silent inspect)
  // deliberately diverges selection from current — until the next move.
  useEffect(() => {
    setSelId(null); // effective selection falls back to data.current
    setCmp(new Set());
    setEditingLabel(null);
    setPreviewing(null);
  }, [data.current]);

  // Keep the difference pop-out mirroring this inspector's compare.
  const onCompareRef = useRef(onCompare);
  onCompareRef.current = onCompare;
  useEffect(() => {
    onCompareRef.current?.({ sel: selId, base: baseSel, cmp: [...cmp] });
  }, [selId, baseSel, cmp]);

  // All node curves in one batched call — feeds the diff plot AND the edge
  // weights. Trees only change on completed gestures, so this stays cheap.
  useEffect(() => {
    const nodes = data.nodes;
    const t = window.setTimeout(() => {
      invoke<ChainCurvesResp>("chain_curves", { chains: nodes.map((n) => n.snap ?? []) })
        .then((r) =>
          setCurves({
            freqs: r.freqs,
            byId: new Map(
              nodes.map((n, k) => [
                n.id,
                { responseDb: r.curves[k].responseDb, preamp: r.curves[k].matchedPreampDb },
              ]),
            ),
          }),
        )
        .catch(() => {});
    }, 150);
    return () => window.clearTimeout(t);
  }, [data]);

  const selNodeId = selId != null && map.has(selId) ? selId : data.current;
  const selNode = map.get(selNodeId);
  const baseId =
    baseSel != null && map.has(baseSel) && baseSel !== selNodeId ? baseSel : selNode?.parent ?? null;
  const baseNode = baseId != null ? map.get(baseId) ?? null : null;
  const group = useMemo(
    () => [selNodeId, ...[...cmp].filter((id) => id !== selNodeId && map.has(id))],
    [selNodeId, cmp, map],
  );
  const multi = group.length >= 2;

  const audible = (id: number) => {
    const c = curves?.byId.get(id);
    return c ? audibleOf(c) : null;
  };
  const madBetween = (i: number, j: number) => {
    const a = audible(i);
    const b = audible(j);
    return a && b ? meanAbs(diffOf(a, b)) : null;
  };
  const edgeW = (child: number, parent: number) => {
    const mad = madBetween(child, parent);
    return mad == null ? 1.5 : 1.2 + Math.min(4.8, mad * 1.6);
  };

  const stopPreview = () => {
    if (previewing) {
      onPreviewRef.current(null);
      setPreviewing(null);
    }
  };

  // ---- drag-graft state: a node dragged onto another copies itself there ----
  const svgElRef = useRef<SVGSVGElement | null>(null);
  const dragStart = useRef<{ id: number; sx: number; sy: number } | null>(null);
  const suppressClick = useRef(false);
  const [ghost, setGhost] = useState<{ id: number; x: number; y: number; target: number | null } | null>(
    null,
  );

  const nodeClick = (e: React.MouseEvent, id: number) => {
    stopPreview();
    if (pickMode) {
      setBaseSel(id === selNodeId ? null : id);
      setPickMode(false);
      return;
    }
    if (e.ctrlKey) {
      setCmp((prev) => {
        const n = new Set(prev);
        if (n.has(id)) n.delete(id);
        else n.add(id);
        n.delete(selNodeId); // the selected node is an implicit member
        return n;
      });
      return;
    }
    setCmp(new Set());
    setEditingLabel(null);
    setSelId(id);
    if (!e.altKey) onJump(id);
  };

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

  // ---- inspector derivations ----
  const fmtWhenShort = (ts: number) =>
    new Date(ts).toLocaleString([], { dateStyle: "short", timeStyle: "short" });
  const selCurve = selNode ? curves?.byId.get(selNode.id) : null;
  const baseCurve = baseNode ? curves?.byId.get(baseNode.id) : null;

  let pairDiff: number[] | null = null;
  let faint: number[][] | undefined;
  if (!multi && selCurve && baseCurve) {
    const a = audibleOf(selCurve);
    const b = audibleOf(baseCurve);
    pairDiff = diffOf(a, b);
    // Context curves centered on their joint mean: shapes AND their relative
    // offset survive; absolute level (≈ the reference) is not the story here.
    const center = (a.reduce((s, x) => s + x, 0) + b.reduce((s, x) => s + x, 0)) / (a.length + b.length);
    faint = [a.map((x) => x - center), b.map((x) => x - center)];
  }
  const multiDiffs =
    multi && baseNode
      ? group.map((id, k) => {
          const a = audible(id);
          const b = audible(baseNode.id);
          return { id, color: CMP_COLORS[k % CMP_COLORS.length], dbs: a && b ? diffOf(a, b) : null };
        })
      : [];
  const autoRange = (() => {
    const peaks = [
      ...(pairDiff ? [Math.max(...pairDiff.map(Math.abs))] : []),
      ...multiDiffs.filter((d) => d.dbs).map((d) => Math.max(...d.dbs!.map(Math.abs))),
    ];
    const m = peaks.length ? Math.max(...peaks) : 0;
    // 33 = the "±30" chip, with the main graph's edge margin trick.
    return [3, 6, 12, 18, 33].find((r) => r >= m + 0.4) ?? 33;
  })();
  const range = diffScale === "auto" ? autoRange : diffScale;
  const par = !multi && selNode && baseNode ? diffChains(baseNode.snap ?? [], selNode.snap ?? []) : null;

  const commitLabel = () => {
    if (editingLabel == null || !selNode) return;
    const v = editingLabel.trim();
    if (v && v !== selNode.label) onEdit(selNode.id, { label: v });
    setEditingLabel(null);
  };
  const doPreview = (which: "node" | "base") => {
    const target = which === "node" ? selNode : baseNode;
    if (!target) return;
    if (previewing === which) {
      setPreviewing(null);
      onPreview(null);
    } else {
      setPreviewing(which);
      onPreview(target.snap ?? []);
    }
  };
  const copyNode = () => {
    const snap = selNode?.snap ?? [];
    if (!snap.length) return;
    navigator.clipboard?.writeText(snap.map(apoLine).join("\r\n")).catch(() => {});
    notify?.(`copied ${snap.length} filter${snap.length === 1 ? "" : "s"} as APO text`);
  };
  const pickDiffScale = (v: number | "auto") => {
    setDiffScale(v);
    storeDiffScale(v);
  };
  const scaleChips = (
    <div className="diff-scale">
      <span className={`scale-opt ${diffScale === "auto" ? "on" : ""}`} onClick={() => pickDiffScale("auto")}>
        auto
      </span>
      {DIFF_SCALES.map((s) => (
        <span
          key={s.v}
          className={`scale-opt ${diffScale === s.v ? "on" : ""}`}
          onClick={() => pickDiffScale(s.v)}
        >
          {s.label}
        </span>
      ))}
      <span className="spacer" />
      {onPopoutDiff && (
        <span
          className="row-act"
          title="pop the difference graph into its own window — stays live while you edit"
          onClick={onPopoutDiff}
        >
          ⇱
        </span>
      )}
    </div>
  );
  const baseLabel = baseNode
    ? baseSel == null || baseId === selNode?.parent
      ? `parent · #${baseId}`
      : `#${baseId} ${baseNode.label}`
    : "— (root)";

  // ---- drag-graft handlers (need pos/PADX/PAD, so they live after layout) ----
  const svgPt = (e: { clientX: number; clientY: number }) => {
    const el = svgElRef.current;
    if (!el) return { x: 0, y: 0 };
    const r = el.getBoundingClientRect(); // reflects the zoom transform
    return { x: (e.clientX - r.left) / zoom, y: (e.clientY - r.top) / zoom };
  };
  const nodeDown = (id: number) => (e: React.PointerEvent) => {
    // Delete glyphs keep their own click path — capturing would retarget it.
    if (e.button !== 0 || (e.target as Element).closest(".tree-del, .tree-confirm")) return;
    dragStart.current = { id, sx: e.clientX, sy: e.clientY };
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
  };
  const dropTarget = (id: number, e: { clientX: number; clientY: number }) => {
    const p = svgPt(e);
    let target: number | null = null;
    let best = 48;
    pos.forEach((c, nid) => {
      if (nid === id) return;
      const dist = Math.hypot(c.x + PADX - p.x, c.y + PAD - p.y);
      if (dist < best) {
        best = dist;
        target = nid;
      }
    });
    return { target, p };
  };
  const nodeMove = (id: number) => (e: React.PointerEvent) => {
    const d = dragStart.current;
    if (!d || d.id !== id) return;
    if (!ghost && Math.hypot(e.clientX - d.sx, e.clientY - d.sy) < 6) return;
    const { target, p } = dropTarget(id, e);
    setGhost({ id, x: p.x, y: p.y, target });
  };
  const nodeUp = (id: number) => (e: React.PointerEvent) => {
    const d = dragStart.current;
    dragStart.current = null;
    setGhost(null);
    if (!d || d.id !== id) return;
    // Decide from the UP event itself — never from possibly-stale ghost state.
    if (Math.hypot(e.clientX - d.sx, e.clientY - d.sy) < 6) return; // a click; onClick handles it
    suppressClick.current = true;
    const { target } = dropTarget(id, e);
    if (target != null) onGraft?.(id, target);
  };

  return (
    <div
      className="hist-body"
      onPointerDownCapture={(e) => {
        // Click-away disarms a pending delete confirm.
        if (!(e.target as Element).closest(".tree-del, .tree-confirm")) setArmed(null);
      }}
    >
      <div className="hist-main">
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
            <svg width={W} height={H} ref={svgElRef}>
              {rows.map((r) => {
                const n = map.get(r.id);
                if (!n || n.parent == null) return null;
                const p = pos.get(n.parent)!;
                const c = pos.get(r.id)!;
                const px = p.x + PADX;
                const py = p.y + PAD;
                const cxx = c.x + PADX;
                const cyy = c.y + PAD;
                // Edge thickness = mean |audible diff| across it: the tree's
                // shape shows where the big sonic moves happened.
                return (
                  <path
                    key={`e${r.id}`}
                    d={`M ${px} ${py + 26} C ${px} ${py + 70}, ${cxx} ${cyy - 70}, ${cxx} ${cyy - 26}`}
                    className={`hist-edge ${r.onPath ? "on" : ""}`}
                    style={{ strokeWidth: edgeW(r.id, n.parent) }}
                  />
                );
              })}
              {rows.map((r) => {
                const c = pos.get(r.id)!;
                const x = c.x + PADX;
                const y = c.y + PAD;
                const isArmed = armed?.id === r.id;
                const nodeR = r.isCurrent ? 28 : 24;
                const gi = group.indexOf(r.id);
                // Long (renamed) labels overflow the circle — truncate on the
                // canvas to what the circle actually fits (the current node is
                // larger); the full name lives on hover and in the inspector.
                const cap = r.isCurrent ? 12 : 9;
                const shortLabel = r.label.length > cap ? `${r.label.slice(0, cap - 1)}…` : r.label;
                const note = map.get(r.id)?.note;
                return (
                  <g
                    key={`n${r.id}`}
                    className={`tree-g ${r.onPath ? "on" : "off"}`}
                    onClick={(e) => {
                      if (suppressClick.current) {
                        suppressClick.current = false;
                        return;
                      }
                      nodeClick(e, r.id);
                    }}
                    onPointerDown={nodeDown(r.id)}
                    onPointerMove={nodeMove(r.id)}
                    onPointerUp={nodeUp(r.id)}
                  >
                    <circle cx={x} cy={y} r={44} fill="transparent" />
                    {!multi && r.id === selNodeId && (
                      <circle cx={x} cy={y} r={nodeR + 5} className="sel-ring" />
                    )}
                    {multi && gi >= 0 && (
                      <circle
                        cx={x}
                        cy={y}
                        r={nodeR + 5}
                        className="cmp-ring"
                        style={{ stroke: CMP_COLORS[gi % CMP_COLORS.length] }}
                      />
                    )}
                    {r.id === baseId && <circle cx={x} cy={y} r={nodeR + 9} className="base-ring" />}
                    <circle
                      cx={x}
                      cy={y}
                      r={nodeR}
                      className={`hist-node ${r.isCurrent ? "current" : ""} ${r.onPath ? "on" : ""}`}
                    />
                    <text x={x} y={y - 13} className={`tree-id ${r.isCurrent ? "inv" : ""}`}>
                      {`#${r.id}`}
                    </text>
                    {map.get(r.id)?.pinned && (
                      <text x={x - 26} y={y - 22} className="tree-pin">
                        ★
                      </text>
                    )}
                    <title>{note ? `${r.label} — ${note}` : r.label}</title>
                    <text x={x} y={y - 1} className={`tree-label ${r.isCurrent ? "inv" : ""}`}>
                      {shortLabel}
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
              {ghost && (
                <g className="graft-ghost">
                  {ghost.target != null &&
                    (() => {
                      const t = pos.get(ghost.target)!;
                      return <circle cx={t.x + PADX} cy={t.y + PAD} r={36} className="graft-target" />;
                    })()}
                  <circle cx={ghost.x} cy={ghost.y} r={20} className="graft-drag" />
                  <text x={ghost.x} y={ghost.y + 3} className="tree-label">
                    {`#${ghost.id}`}
                  </text>
                </g>
              )}
            </svg>
          </div>
        </div>
      </div>
      <div className="hist-hint mono">
        {pickMode
          ? "pick-mode: click the node to compare against · the chip cancels"
          : ghost
            ? ghost.target != null
              ? `drop: copy #${ghost.id} under #${ghost.target} as one step`
              : "drop on a node to copy this sound there as one clean step"
            : "click = jump (audible) · Alt+click = inspect silently · Ctrl+click = add to compare · drag node onto node = copy it there as one step · thicker edge = bigger sound change · scroll = zoom · × prunes"}
      </div>
      </div>

      <div className="hist-inspector">
        <span className="mono lab-label">{multi ? `NODE #${selNodeId} · +${group.length - 1} compared` : `NODE #${selNodeId}`}</span>
        <div className="insp-head">
          {editingLabel != null ? (
            <input
              className="rename-input"
              autoFocus
              value={editingLabel}
              onChange={(e) => setEditingLabel(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitLabel();
                if (e.key === "Escape") setEditingLabel(null);
              }}
              onBlur={commitLabel}
            />
          ) : (
            <span className="insp-name" title={selNode?.label}>
              {selNode?.label}
            </span>
          )}
          <span
            className="row-act"
            title="rename — gesture labels become intentions"
            onClick={() => setEditingLabel(selNode?.label ?? "")}
          >
            ✎
          </span>
          <span
            className={`insp-pin ${selNode?.pinned ? "on" : ""}`}
            title={selNode?.pinned ? "unpin" : "pin this node"}
            onClick={() => selNode && onEdit(selNode.id, { pinned: !selNode.pinned })}
          >
            ★
          </span>
        </div>
        <div className="mono dim-sm">
          {selNode ? `${fmtWhenShort(selNode.ts)} · ${(selNode.snap ?? []).length} filters` : ""}
        </div>
        <input
          key={`note-${selNodeId}`}
          className="insp-note"
          placeholder="add a note…"
          defaultValue={selNode?.note ?? ""}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          }}
          onBlur={(e) => {
            const v = e.target.value.trim();
            if (selNode && v !== (selNode.note ?? "")) onEdit(selNode.id, { note: v });
          }}
        />

        <div className="insp-vs">
          <span className="dim-sm">compared to</span>
          <span
            className={`hist-chip mono ${pickMode ? "picking" : ""}`}
            title="everything below — the graph, the changes, hearing, the blind test — is this node against the one named here. Click, then click any node on the tree to change it."
            onClick={() => setPickMode((p) => !p)}
          >
            {pickMode ? "click a node on the tree…" : baseLabel}
          </span>
          {baseSel != null && !pickMode && (
            <span className="row-act" title="back to comparing against the parent" onClick={() => setBaseSel(null)}>
              ↺
            </span>
          )}
        </div>

        {!multi && (
          <>
            {!baseNode && (
              <p className="dim-sm">
                the root has nothing before it — use the chip above to compare it to any node.
              </p>
            )}
            {baseNode && (
              <>
                {pairDiff && curves && (
                  <>
                    <DiffPlot
                      freqs={curves.freqs}
                      diffs={[{ dbs: pairDiff, color: "var(--ink)" }]}
                      faint={faint}
                      range={range}
                    />
                    <p className="dim-sm insp-cap">
                      {`how #${selNodeId} differs from #${baseId} — flat at 0 = they sound identical`}
                    </p>
                  </>
                )}
                {scaleChips}
                {par && (
                  <div className="diff-rows">
                    {par.rows.length === 0 && <span className="dim-sm">no parametric changes</span>}
                    {par.rows.map((row, k) => (
                      <DiffRowView key={k} row={row} />
                    ))}
                    {par.unchanged > 0 && (
                      <span className="dim-sm">{`${par.unchanged} filter${par.unchanged === 1 ? "" : "s"} unchanged`}</span>
                    )}
                  </div>
                )}
              </>
            )}
          </>
        )}

        {multi && (
          <>
            {!baseNode && (
              <p className="dim-sm">nothing to compare against — use the chip above to pick a node.</p>
            )}
            {baseNode && curves && (
              <>
                <DiffPlot
                  freqs={curves.freqs}
                  diffs={multiDiffs.filter((d) => d.dbs).map((d) => ({ dbs: d.dbs!, color: d.color }))}
                  range={range}
                />
                <p className="dim-sm insp-cap">{`each curve: how that node differs from #${baseId}`}</p>
                {scaleChips}
                <div className="diff-rows">
                  {multiDiffs.map((d) => {
                    const n = map.get(d.id)!;
                    return (
                      <div
                        key={d.id}
                        className={`legend-row ${d.id === selNodeId ? "sel" : ""}`}
                        onClick={() => setSelId(d.id)}
                      >
                        <span className="sw" style={{ background: d.color }} />
                        <span className="mono">{`#${d.id}`}</span>
                        <span className="legend-name">{n.label}</span>
                        <span className="spacer" />
                        {d.id !== selNodeId && (
                          <span
                            className="row-act"
                            onClick={(e) => {
                              e.stopPropagation();
                              setCmp((p) => {
                                const s = new Set(p);
                                s.delete(d.id);
                                return s;
                              });
                            }}
                          >
                            ×
                          </span>
                        )}
                      </div>
                    );
                  })}
                </div>
                <p className="dim-sm">Ctrl+click nodes on the tree to add or remove them.</p>
              </>
            )}
          </>
        )}

        <div className="insp-actions">
          <button
            className="primary"
            disabled={!baseNode}
            onClick={() => baseNode && selNode && onAbx(selNode.id, baseNode.id)}
            title="blind ABX these two — Fletcher claims nothing; the test is the only arbiter"
          >
            {baseNode ? `⚖ blind test #${selNodeId} vs #${baseId}` : "⚖ blind test"}
          </button>
        </div>
        <div className="insp-vs">
          <span className="dim-sm">hear</span>
          <div className="insp-ab mono">
            <span
              className={`insp-ab-side ${previewing === "node" ? "active" : ""} ${
                selNodeId === data.current && previewing !== "node" ? "disabled" : ""
              }`}
              title={
                selNodeId === data.current
                  ? `#${selNodeId} is what you're hearing already`
                  : `hear #${selNodeId} without moving to it — click again to go back`
              }
              onClick={() => {
                if (selNodeId !== data.current || previewing === "node") doPreview("node");
              }}
            >
              {`#${selNodeId}`}
            </span>
            <span
              className={`insp-ab-side ${previewing === "base" ? "active" : ""} ${!baseNode ? "disabled" : ""}`}
              title={
                baseNode
                  ? `hear #${baseId} without moving to it — click again to go back`
                  : "nothing to compare against yet"
              }
              onClick={() => baseNode && doPreview("base")}
            >
              {baseNode ? `#${baseId}` : "—"}
            </span>
          </div>
          {previewing && <span className="dim-sm">click again to go back</span>}
        </div>
        <div className="insp-actions">
          <button
            onClick={() => selNode && onPromote(selNode.id)}
            disabled={!(selNode?.snap ?? []).length}
            title="save this node's chain as a new preset in the preset menu"
          >
            save as preset
          </button>
          <button
            onClick={copyNode}
            disabled={!(selNode?.snap ?? []).length}
            title="copy this node's filters to the clipboard as APO text"
          >
            copy APO
          </button>
        </div>
      </div>
    </div>
  );
}

/** The FFT pane (artboard): instantaneous spectrum at the playhead with the
 *  EQ curve overlaid and boost/cut shading — shows where the EQ acts on what
 *  is playing right now. Never claims audibility. */
function FftView({
  trackId,
  posS,
  eq,
}: {
  trackId: number;
  posS: number;
  eq: { freqs: number[]; sumDb: number[]; preampDb: number } | null;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [spectrum, setSpectrum] = useState<number[] | null>(null);
  const [cur, setCur] = useState<{ x: number; y: number } | null>(null);
  const [resizeTick, setResizeTick] = useState(0);
  const busy = useRef(false);
  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setResizeTick((t) => t + 1));
    ro.observe(cv);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (busy.current) return;
    busy.current = true;
    invoke<number[]>("track_fft", { id: trackId, tS: posS, points: 240 })
      .then(setSpectrum)
      .catch(() => {})
      .finally(() => {
        busy.current = false;
      });
  }, [trackId, posS]);

  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv) return;
    const dpr = window.devicePixelRatio || 1;
    const w = cv.clientWidth;
    const h = cv.clientHeight;
    if (cv.width !== Math.round(w * dpr)) cv.width = Math.round(w * dpr);
    if (cv.height !== Math.round(h * dpr)) cv.height = Math.round(h * dpr);
    const ctx = cv.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const xOfF = (f: number) =>
      ((Math.log10(f) - Math.log10(20)) / (Math.log10(20000) - Math.log10(20))) * w;
    // frequency grid
    ctx.strokeStyle = "#eee9dd";
    ctx.font = "9px 'IBM Plex Mono', monospace";
    for (const f of [100, 1000, 10000]) {
      const x = xOfF(f);
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
      ctx.stroke();
      ctx.fillStyle = "#8b8578";
      ctx.fillText(f >= 1000 ? `${f / 1000}k` : `${f}`, x + 3, h - 4);
    }
    // spectrum: filled grey, −90..−10 dB mapped to the pane
    if (spectrum) {
      const yOfDb = (db: number) => h - ((db + 90) / 80) * (h - 14);
      ctx.beginPath();
      spectrum.forEach((db, i) => {
        const x = (i / (spectrum.length - 1)) * w;
        const y = yOfDb(db);
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      });
      ctx.strokeStyle = "#55503f";
      ctx.lineWidth = 1.2;
      ctx.stroke();
      ctx.lineTo(w, h);
      ctx.lineTo(0, h);
      ctx.closePath();
      ctx.fillStyle = "rgba(85, 80, 63, 0.25)";
      ctx.fill();
      ctx.lineWidth = 1;
    }
    // the EQ overlay: dashed 0-line, curve, boost/cut shading (artboard)
    if (eq && eq.freqs.length) {
      const zeroY = h * 0.3;
      const pxPerDb = 3.0;
      const yOfGain = (g: number) => zeroY - g * pxPerDb;
      ctx.strokeStyle = "#c9c2b0";
      ctx.setLineDash([4, 4]);
      ctx.beginPath();
      ctx.moveTo(0, zeroY);
      ctx.lineTo(w, zeroY);
      ctx.stroke();
      ctx.setLineDash([]);
      const gains = eq.freqs.map((f, i) => ({ x: xOfF(f), g: eq.sumDb[i] - eq.preampDb }));
      // shading between the curve and 0: orange above, blue below
      for (const on of [true, false]) {
        ctx.beginPath();
        ctx.moveTo(gains[0].x, zeroY);
        for (const p of gains) {
          const y = on ? Math.min(yOfGain(p.g), zeroY) : Math.max(yOfGain(p.g), zeroY);
          ctx.lineTo(p.x, y);
        }
        ctx.lineTo(gains[gains.length - 1].x, zeroY);
        ctx.closePath();
        ctx.fillStyle = on ? "rgba(200, 90, 19, 0.14)" : "rgba(63, 109, 158, 0.14)";
        ctx.fill();
      }
      ctx.beginPath();
      gains.forEach((p, i) => {
        if (i === 0) ctx.moveTo(p.x, yOfGain(p.g));
        else ctx.lineTo(p.x, yOfGain(p.g));
      });
      ctx.strokeStyle = "rgba(200, 90, 19, 0.75)";
      ctx.lineWidth = 1.6;
      ctx.stroke();
      ctx.lineWidth = 1;
    }
    // legend
    ctx.fillStyle = "#8b8578";
    ctx.font = "9.5px 'IBM Plex Mono', monospace";
    const legend = "FFT at playhead · — your EQ · ▮▮ where it boosts/cuts";
    ctx.fillText(legend, w - ctx.measureText(legend).width - 8, 12);

    // cursor readout: frequency + level under the pointer, and the signal's
    // actual dB at that frequency.
    if (cur && cur.x >= 0 && cur.x <= w) {
      const f = 10 ** (Math.log10(20) + (cur.x / w) * (Math.log10(20000) - Math.log10(20)));
      const curDb = ((h - cur.y) / (h - 14)) * 80 - 90;
      let sig = "";
      if (spectrum) {
        const i = Math.min(
          Math.max(Math.round((cur.x / w) * (spectrum.length - 1)), 0),
          spectrum.length - 1,
        );
        sig = ` · signal ${spectrum[i].toFixed(1)} dB`;
      }
      const label = `${fmtHz(f)}Hz · ${curDb.toFixed(1)} dB${sig}`;
      ctx.font = "10px 'IBM Plex Mono', monospace";
      const tw = ctx.measureText(label).width;
      ctx.fillStyle = "rgba(242, 239, 233, 0.92)";
      ctx.fillRect(6, 4, tw + 10, 15);
      ctx.strokeStyle = "#c9c2b0";
      ctx.strokeRect(5.5, 3.5, tw + 11, 16);
      ctx.fillStyle = "#55503f";
      ctx.fillText(label, 11, 15);
    }
  }, [spectrum, eq, cur, resizeTick]);

  return (
    <canvas
      ref={canvasRef}
      className="fft-pane"
      onPointerMove={(e) => {
        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        setCur({ x: e.clientX - rect.left, y: e.clientY - rect.top });
      }}
      onPointerLeave={() => setCur(null)}
    />
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

  // Trampoline (dev law, Q-20) for the document-level key listener.
  const keyRef = useRef(handleKey);
  keyRef.current = handleKey;

  useEffect(() => {
    // Hello only after the listener is live — a race leaves the reply unheard.
    // `alive` = post-unmount setState hygiene (the unlisten promise is async).
    let alive = true;
    const un = listen<HistTreeData>("hist-sync", (e) => {
      if (alive) setData(e.payload);
    });
    un.then(() => emitTo("main", "hist-hello", {}));
    const onKey = (e: KeyboardEvent) => keyRef.current(e);
    // Capture phase + document: maximum chance of delivery in a secondary webview.
    document.addEventListener("keydown", onKey, true);
    const grabFocus = () => rootRef.current?.focus();
    window.addEventListener("focus", grabFocus);
    grabFocus();
    return () => {
      alive = false;
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
      onPointerDown={(e) => {
        // Keyboard reclaim — but never mid-node-press: stealing focus while a
        // drag-graft is starting can disturb its pointer capture.
        if (!(e.target as Element).closest(".tree-g")) rootRef.current?.focus();
      }}
      style={{ outline: "none" }}
    >
      <div className="hist-head">
        <span className="mono hist-title">{`HISTORY${data?.name ? ` — ${data.name}` : ""}`}</span>
        <span className="spacer" />
        <button
          onClick={() => emitTo("main", "hist-cmd", { type: "import", id: 0 })}
          title="load a history file — its tree replaces this one and you land on its current node"
        >
          import
        </button>
        <button
          onClick={() => emitTo("main", "hist-cmd", { type: "export", id: 0 })}
          title="save this tree (with every branch and snapshot) as a shareable file"
        >
          export
        </button>
        <span className="mono dim-sm">live-synced with the main window</span>
      </div>
      {data ? (
        <HistoryTree
          data={data}
          onJump={(id) => emitTo("main", "hist-cmd", { type: "jump", id })}
          onDelete={(id) => emitTo("main", "hist-cmd", { type: "del", id })}
          onEdit={(id, patch) => emitTo("main", "hist-cmd", { type: "edit", id, patch })}
          onPreview={(snap) => emitTo("main", "hist-cmd", { type: snap ? "preview" : "restore", id: 0, snap })}
          onAbx={(a, b) => emitTo("main", "hist-cmd", { type: "abx", id: a, base: b })}
          onPromote={(id) => emitTo("main", "hist-cmd", { type: "promote", id })}
          onCompare={(spec) => emitTo("main", "hist-cmd", { type: "compare", id: 0, spec })}
          onPopoutDiff={() => emitTo("main", "hist-cmd", { type: "popdiff", id: 0 })}
          onGraft={(id, onto) => emitTo("main", "hist-cmd", { type: "graft", id, base: onto })}
        />
      ) : (
        <p className="dim-sm" style={{ padding: 20 }}>
          waiting for the main window…
        </p>
      )}
    </div>
  );
}

/** Satellite scopes feed themselves: transport orientation via track_status,
 *  then the broadcast track-state/track-pos events. Commands (seek, fft,
 *  spectrogram) invoke Rust directly — no main-window routing needed. */
function useTrackFeed() {
  const [sess, setSess] = useState<{ id: number; durationS: number } | null>(null);
  const [pos, setPos] = useState({ posS: 0, paused: false });
  useEffect(() => {
    invoke<{
      active: boolean;
      trackId: number | null;
      paused: boolean;
      posS: number;
      durationS: number;
    }>("track_status")
      .then((s) => {
        if (s.active && s.trackId != null) {
          setSess({ id: s.trackId, durationS: s.durationS });
          setPos({ posS: s.posS, paused: s.paused });
        }
      })
      .catch(() => {});
    const u1 = listen<Record<string, unknown>>("track-state", (e) => {
      const p = e.payload;
      if (p.event === "started") {
        setSess({ id: p.trackId as number, durationS: p.durationS as number });
      } else if (p.event === "ended") {
        setSess(null);
      }
    });
    const u2 = listen<{ posS: number; paused: boolean }>("track-pos", (e) =>
      setPos({ posS: e.payload.posS, paused: e.payload.paused }),
    );
    return () => {
      u1.then((f) => f());
      u2.then((f) => f());
    };
  }, []);
  return { sess, pos };
}

type RoomState = {
  tcDec: number;
  specOn: boolean;
  waveOn: boolean;
  fftOn: boolean;
  specWin: number;
  specFloor: number;
  specLinear: boolean;
  mode: "bypass" | "eq";
  scrubTau: number;
  scrubMax: number;
};

/**
 * The Clip Studio ⚙ menu, satellite edition: the SAME menu as the main
 * window, but the state has one owner — main feeds it over "room-state" and
 * every change goes back over "room-cmd". Typed input works here (Q-20
 * refinement: focused inputs get keys; only document-level handlers don't).
 */
function ScopeRoomMenu() {
  const [open, setOpen] = useState(false);
  const [st, setSt] = useState<RoomState | null>(null);
  useEffect(() => {
    const un = listen<RoomState>("room-state", (e) => setSt(e.payload));
    emit("scope-hello", "room-menu");
    return () => {
      un.then((f) => f());
    };
  }, []);
  const cmd = (key: string, value: number | string) => emit("room-cmd", { key, value });
  return (
    <span style={{ position: "relative" }}>
      <span
        className="row-act"
        title="Clip Studio view settings — shared with the main window"
        onClick={() => setOpen((o) => !o)}
      >
        ⚙
      </span>
      {open && st && (
        <div className="preset-menu device-menu room-menu">
          <span className="mono lab-label">CLIP STUDIO VIEW</span>
          <div className="room-row">
            <span
              className="room-key"
              title="One-click view setups. Melody: 8k window, spectrogram solo, log axis, ~1 ms view at the playhead — the harmonics become live pitch lines that ride the melody as it plays. Standard: all scopes, 2k window, whole track."
            >
              View preset
            </span>
            <div className="seg seg-sm">
              <span className="seg-opt" onClick={() => cmd("viewpreset", "melody")}>
                Melody
              </span>
              <span className="seg-opt" onClick={() => cmd("viewpreset", "standard")}>
                Standard
              </span>
            </div>
          </div>
          <div className="room-row">
            <span className="room-key">Timecode decimals</span>
            <div className="seg seg-sm">
              {[1, 2, 3].map((v) => (
                <span
                  key={v}
                  className={`seg-opt ${st.tcDec === v ? "on" : ""}`}
                  onClick={() => cmd("tcdec", v)}
                >
                  {`.${"0".repeat(v)}`}
                </span>
              ))}
            </div>
          </div>
          <div className="room-row">
            <span className="room-key">Scopes</span>
            <div className="is-tags">
              {(
                [
                  ["spec", "Spec", st.specOn],
                  ["wave", "Wave", st.waveOn],
                  ["fft", "FFT", st.fftOn],
                ] as const
              ).map(([k, label, on]) => (
                <span
                  key={k}
                  className={`scale-opt ${on ? "on" : ""}`}
                  onClick={() => cmd("scope", k)}
                >
                  {label}
                </span>
              ))}
            </div>
          </div>
          <div className="room-row">
            <span
              className="room-key"
              title="FFT window: smaller = sharper in time (transients, rhythm), bigger = sharper in frequency (tones, harmonics). 256 ≈ 5 ms slices; 8k ≈ 170 ms."
            >
              Spectrogram window
            </span>
            <div className="seg seg-sm">
              {(
                [
                  [256, "256"],
                  [512, "512"],
                  [1024, "1k"],
                  [2048, "2k"],
                  [4096, "4k"],
                  [8192, "8k"],
                ] as const
              ).map(([v, label]) => (
                <span
                  key={v}
                  className={`seg-opt ${st.specWin === v ? "on" : ""}`}
                  onClick={() => cmd("specwin", v)}
                >
                  {label}
                </span>
              ))}
            </div>
          </div>
          <div className="room-row">
            <span
              className="room-key"
              title="Log matches hearing (octaves get equal space). Linear gives every Hz equal space — harmonic stacks read as evenly spaced lines, and the treble half isn't squeezed."
            >
              Spectrogram axis
            </span>
            <div className="seg seg-sm">
              <span
                className={`seg-opt ${!st.specLinear ? "on" : ""}`}
                onClick={() => cmd("speclinear", 0)}
              >
                Log
              </span>
              <span
                className={`seg-opt ${st.specLinear ? "on" : ""}`}
                onClick={() => cmd("speclinear", 1)}
              >
                Linear
              </span>
            </div>
          </div>
          <div className="room-row">
            <span className="room-key">Spectrogram floor</span>
            <div className="seg seg-sm">
              {(
                [
                  [-70, "hot"],
                  [-90, "normal"],
                  [-110, "deep"],
                ] as const
              ).map(([v, label]) => (
                <span
                  key={v}
                  className={`seg-opt ${st.specFloor === v ? "on" : ""}`}
                  onClick={() => cmd("specfloor", v)}
                >
                  {label}
                </span>
              ))}
            </div>
          </div>
          <div className="room-row">
            <span
              className="room-key"
              title="How held-C chases your cursor. Chase is its reaction time — every chase interval it closes ~63% of the remaining gap to the mouse (smaller = tighter to your hand, larger = smoother tape-like glide). Max caps how fast it may play while catching up, in multiples of real time. Defaults: 60 ms · 8×."
            >
              Scrub feel
            </span>
            <div className="trials-ctl">
              <GenNum
                value={st.scrubTau}
                min={5}
                max={1000}
                unit="ms chase"
                onCommit={(v) => cmd("scrubtau", v)}
              />
              <GenNum
                value={st.scrubMax}
                min={0.5}
                max={64}
                unit="× max"
                onCommit={(v) => cmd("scrubmax", v)}
              />
            </div>
          </div>
          <div className="room-row">
            <span className="room-key">Play method</span>
            <div className="seg seg-sm">
              <span
                className={`seg-opt ${st.mode === "bypass" ? "on" : ""}`}
                onClick={() => cmd("mode", "bypass")}
                title="Curation: the track itself — exclusive device, no EQ, level-matched toward the reference. Takes effect on the next play."
              >
                Bypass
              </span>
              <span
                className={`seg-opt ${st.mode === "eq" ? "on" : ""}`}
                onClick={() => cmd("mode", "eq")}
                title="A regular player: the normal shared path — your EQ and the level-matched A/B apply like for any stream. Takes effect on the next play."
              >
                Through EQ
              </span>
            </div>
          </div>
        </div>
      )}
    </span>
  );
}

/** The spectrogram in its own window — zoom/pan/seek, fed by broadcasts. */
function PopoutScopeSpec() {
  const { sess, pos } = useTrackFeed();
  const [spec, setSpec] = useState<SpecData | null>(null);
  const posRef = useRef(pos);
  posRef.current = pos;
  // The I/O region lives in the main window's state — it broadcasts changes
  // and answers our hello with the current value (satellite law: same view
  // everywhere).
  const [region, setRegion] = useState<{ a: number; b: number } | null>(null);
  useEffect(() => {
    const un = listen<{ a: number; b: number } | null>("io-region", (e) =>
      setRegion(e.payload ?? null),
    );
    emit("scope-hello", "scope-spec");
    return () => {
      un.then((f) => f());
    };
  }, []);
  // C arrives as a forwarded OS shortcut with press/release (Q-20 law):
  // held C = scrub mode, same semantics as the main window.
  const hover = useRef<number | null>(null);
  const scrub = useRef(false);
  useEffect(() => {
    const un = listen<{ key: string; state: string }>("scope-key", (e) => {
      const p = e.payload;
      if (p.key !== "c") return;
      if (p.state === "down") {
        if (scrub.current || hover.current == null) return;
        scrub.current = true;
        invoke("track_scrub", { on: true }).catch(() => {});
        invoke("track_seek", { seconds: hover.current }).catch(() => {});
      } else {
        if (!scrub.current) return;
        scrub.current = false;
        invoke("track_scrub", { on: false }).catch(() => {});
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);
  // Spectrogram parameters: seeded from localStorage (same origin), kept
  // live by the main window's "spec-params" broadcasts (localStorage changes
  // don't notify other webviews).
  const [specParams, setSpecParams] = useState(() => {
    const lsN = (key: string, dflt: number) => {
      try {
        const v = Number(localStorage.getItem(key));
        return Number.isFinite(v) && v !== 0 ? v : dflt;
      } catch {
        return dflt;
      }
    };
    let linear = false;
    try {
      linear = localStorage.getItem("fletcher.speclinear") === "1";
    } catch {
      /* default */
    }
    return { win: lsN("fletcher.specwin", 2048), floor: lsN("fletcher.specfloor", -90), linear };
  });
  useEffect(() => {
    const un = listen<{ win: number; floor: number; linear: boolean }>("spec-params", (e) =>
      setSpecParams({ win: e.payload.win, floor: e.payload.floor, linear: !!e.payload.linear }),
    );
    return () => {
      un.then((f) => f());
    };
  }, []);
  useEffect(() => {
    setSpec(null);
    if (!sess) return;
    let alive = true;
    invoke<SpecData>("track_spectrogram", {
      id: sess.id,
      win: specParams.win,
      floorDb: specParams.floor,
      linear: specParams.linear,
    })
      .then((s) => {
        if (alive) setSpec(s);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sess?.id, specParams.win, specParams.floor, specParams.linear]);
  return (
    <div className="hist-panel full scope-window">
      <div className="hist-head">
        <span className="mono hist-title">SPECTROGRAM</span>
        <span className="spacer" />
        <span className="mono dim-sm">live · click = seek · scroll = zoom · drag = pan</span>
        <ScopeRoomMenu />
      </div>
      {sess ? (
        <div className="scope-body">
          <TimelineView
            trackId={sess.id}
            durationS={sess.durationS}
            posS={pos.posS}
            onSeek={(s) => invoke("track_seek", { seconds: s }).catch(() => {})}
            onHover={(t) => {
              hover.current = t;
              if (t != null && scrub.current) {
                invoke("track_seek", { seconds: t }).catch(() => {});
              }
            }}
            region={region}
            spec={spec}
            showSpec
            showWave={false}
            syncKey="scope-spec"
          />
        </div>
      ) : (
        <p className="dim-sm" style={{ padding: 20 }}>
          no track playing — start one in Clip Studio
        </p>
      )}
    </div>
  );
}

/** The FFT pane in its own window — spectrum at the playhead + the live EQ. */
function PopoutScopeFft() {
  const { sess, pos } = useTrackFeed();
  const [eq, setEq] = useState<{ freqs: number[]; sumDb: number[]; preampDb: number } | null>(null);
  useEffect(() => {
    const load = () =>
      invoke<EqState>("eq_state")
        .then((s) => setEq({ freqs: s.freqs, sumDb: s.sumDb, preampDb: s.preampDb }))
        .catch(() => {});
    load();
    const u = listen("apo-config-changed", load);
    return () => {
      u.then((f) => f());
    };
  }, []);
  return (
    <div className="hist-panel full scope-window">
      <div className="hist-head">
        <span className="mono hist-title">FFT</span>
        <span className="spacer" />
        <span className="mono dim-sm">spectrum at the playhead · your EQ overlaid</span>
        <ScopeRoomMenu />
      </div>
      {sess ? (
        <div className="scope-body">
          <FftView trackId={sess.id} posS={pos.posS} eq={eq} />
        </div>
      ) : (
        <p className="dim-sm" style={{ padding: 20 }}>
          no track playing — start one in Clip Studio
        </p>
      )}
    </div>
  );
}

// What the difference pop-out mirrors: the compared nodes, resolved by the
// main window against the live tree (so it keeps tracking as the tree grows).
type DiffSync = {
  base: { id: number; label: string; snap: ChainSnap } | null;
  series: { id: number; label: string; color: string; snap: ChainSnap }[];
};

const DIFF_SCALES = [
  { label: "±3", v: 3 },
  { label: "±6", v: 6 },
  { label: "±12", v: 12 },
  { label: "±18", v: 18 },
  { label: "±30", v: 33 }, // main-graph margin trick: a ±30 curve isn't pinned to the edge
];

const loadDiffScale = (): number | "auto" => {
  try {
    const s = localStorage.getItem("fletcher.diffscale");
    return s === null || s === "auto" ? "auto" : Number(s);
  } catch {
    return "auto";
  }
};
const storeDiffScale = (v: number | "auto") => {
  try {
    localStorage.setItem("fletcher.diffscale", String(v));
  } catch {
    /* per-viewer nicety only */
  }
};

/** The difference-graph pop-out: mirrors whichever history inspector the user
 *  last touched and keeps tracking the live tree. With the default compare
 *  (current node vs its parent) it continuously shows "what did my last
 *  gesture change" while sculpting. */
function PopoutDiff() {
  const [sync, setSync] = useState<DiffSync | null>(null);
  const [curves, setCurves] = useState<{ freqs: number[]; audible: number[][] } | null>(null);
  const [scale, setScale] = useState<number | "auto">(loadDiffScale);

  useEffect(() => {
    // Say hello only AFTER the listener is registered — the reply is a one-shot
    // (re-emits only happen on tree changes), so a race here means a window
    // that sits on "waiting" forever.
    let un: (() => void) | undefined;
    let gone = false;
    listen<DiffSync>("diff-sync", (e) => {
      if (!gone) setSync(e.payload);
    }).then((f) => {
      if (gone) {
        f();
        return;
      }
      un = f;
      emitTo("main", "diff-hello", {});
    });
    return () => {
      gone = true;
      un?.();
    };
  }, []);

  useEffect(() => {
    if (!sync?.base) {
      setCurves(null);
      return;
    }
    const chains = [sync.base.snap, ...sync.series.map((s) => s.snap)];
    const t = window.setTimeout(() => {
      invoke<ChainCurvesResp>("chain_curves", { chains })
        .then((r) =>
          setCurves({
            freqs: r.freqs,
            audible: r.curves.map((c) => c.responseDb.map((db) => db + c.matchedPreampDb)),
          }),
        )
        .catch(() => {});
    }, 100);
    return () => window.clearTimeout(t);
  }, [sync]);

  const diffs =
    sync && curves
      ? sync.series.map((s, k) => ({
          dbs: diffOf(curves.audible[k + 1], curves.audible[0]),
          color: sync.series.length === 1 ? "var(--ink)" : s.color,
        }))
      : [];
  const faint =
    sync && curves && sync.series.length === 1
      ? (() => {
          const a = curves.audible[1];
          const b = curves.audible[0];
          const center =
            (a.reduce((s, x) => s + x, 0) + b.reduce((s, x) => s + x, 0)) / (a.length + b.length);
          return [a.map((x) => x - center), b.map((x) => x - center)];
        })()
      : undefined;
  const autoR = (() => {
    const peaks = diffs.map((d) => Math.max(...d.dbs.map(Math.abs)));
    const m = peaks.length ? Math.max(...peaks) : 0;
    return [3, 6, 12, 18, 33].find((r) => r >= m + 0.4) ?? 33;
  })();
  const range = scale === "auto" ? autoR : scale;
  const pick = (v: number | "auto") => {
    setScale(v);
    storeDiffScale(v);
  };

  return (
    <div className="hist-panel full">
      <div className="hist-head">
        <span className="mono hist-title">DIFFERENCE</span>
        {sync?.base && sync.series.length === 1 && (
          <span className="mono dim-sm">
            {`#${sync.series[0].id} ${sync.series[0].label} vs #${sync.base.id} ${sync.base.label}`}
          </span>
        )}
        <span className="spacer" />
        <span className="mono dim-sm">live-synced · flat at 0 = they sound identical</span>
      </div>
      {sync?.base && curves ? (
        <div className="diff-win-body">
          <DiffPlot freqs={curves.freqs} diffs={diffs} faint={faint} range={range} w={760} h={420} />
          <div className="diff-scale">
            <span className={`scale-opt ${scale === "auto" ? "on" : ""}`} onClick={() => pick("auto")}>
              auto
            </span>
            {DIFF_SCALES.map((s) => (
              <span key={s.v} className={`scale-opt ${scale === s.v ? "on" : ""}`} onClick={() => pick(s.v)}>
                {s.label}
              </span>
            ))}
          </div>
          {sync.series.length > 1 && (
            <div className="diff-rows">
              {sync.series.map((s) => (
                <div key={s.id} className="legend-row">
                  <span className="sw" style={{ background: s.color }} />
                  <span className="mono">{`#${s.id}`}</span>
                  <span className="legend-name">{s.label}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      ) : (
        <p className="dim-sm" style={{ padding: 20 }}>
          {sync && !sync.base
            ? "nothing to compare — the current node is the root"
            : "waiting for the main window…"}
        </p>
      )}
    </div>
  );
}

type PresetsState = { presets: string[]; active: string | null };
type AbInfo = { side: string; matchDb: number; shortfallDb: number };
type Device = { id: string; name: string; isDefault: boolean };
type AutoeqEntry = { name: string; path: string; note: string };

type AbxState = {
  active: boolean;
  aName: string;
  bName?: string;
  planned: number;
  answered: number;
  audition: string;
  levelMatched?: boolean;
  runningCorrect: number | null;
};

type SettingsState = {
  referenceDb: number;
  levelMatching: boolean;
  apoInstallPath: string | null;
  apoConfigPath: string | null;
};
type AbxTrial = { xWasA: boolean; answeredA: boolean; correct: boolean };
type AbxResult = {
  id: string;
  aName: string;
  // Sessions recorded before node-vs-node ABX lack these; B was always Flat.
  bName?: string;
  aChain?: ChainSnap;
  bChain?: ChainSnap;
  referenceDb?: number;
  levelMatched?: boolean;
  trials: number;
  correct: number;
  pValue: number;
  statsViewed: number[];
  log: AbxTrial[];
  startedMs: number;
};

type TrackRow = {
  id: number;
  kind: string;
  title: string;
  artist: string | null;
  genre: string | null;
  path: string | null;
  sourceUrl: string | null;
  signalParams: string | null;
  durationS: number | null;
  addedMs: number;
};

/** A generated signal's recipe (mirrors the Rust SigSpec; the future API/MCP surface). */
type SigSpec = {
  kind: "white" | "pink" | "sine" | "sweepLog" | "sweepLinear" | "band" | "mix";
  seconds: number;
  levelDb: number;
  hz?: number;
  fromHz?: number;
  toHz?: number;
  sweepS?: number;
  loHz?: number;
  hiHz?: number;
  /** Tremolo: rate Hz + depth 0..1 (both or neither). */
  amHz?: number;
  amDepth?: number;
  /** Vibrato (tonal kinds): rate Hz + ±deviation Hz. */
  fmHz?: number;
  fmDevHz?: number;
  /** kind === "mix": primitives summed (one level deep, cap binds the sum). */
  layers?: SigSpec[];
};

/** A typed number field for the generator: commit on blur/Enter, clamped. */
function GenNum({
  value,
  min,
  max,
  unit,
  onCommit,
}: {
  value: number | undefined;
  min: number;
  max: number;
  unit: string;
  onCommit: (v: number) => void;
}) {
  return (
    <span className="gen-num">
      <input
        key={value}
        className="trials-input mono"
        type="number"
        min={min}
        max={max}
        defaultValue={value}
        onBlur={(e) => {
          const v = +e.target.value;
          if (Number.isFinite(v) && v !== value) onCommit(Math.min(max, Math.max(min, v)));
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
      />
      <span className="dim-sm">{unit}</span>
    </span>
  );
}

/** Short name for one layer of a mix (or any primitive spec). */
const layerName = (l: SigSpec): string => {
  const hz = (v?: number) => (v == null ? "?" : v >= 1000 ? `${+(v / 1000).toFixed(2)}k` : `${v}`);
  switch (l.kind) {
    case "white":
      return "white";
    case "pink":
      return "pink";
    case "sine":
      return `sine ${hz(l.hz)}`;
    case "band":
      return `band ${hz(l.loHz)}–${hz(l.hiHz)}`;
    case "sweepLog":
      return `sweep ${hz(l.fromHz)}–${hz(l.toHz)} log`;
    case "sweepLinear":
      return `sweep ${hz(l.fromHz)}–${hz(l.toHz)} lin`;
    default:
      return l.kind;
  }
};

/** The rail's metadata line for a signal row — what the title doesn't say. */
const sigSummary = (t: TrackRow): string => {
  try {
    const s = JSON.parse(t.signalParams ?? "") as SigSpec;
    if (s.kind === "mix") {
      const ls = s.layers ?? [];
      return ls.map((l) => `${layerName(l)} ${l.levelDb} dB`).join(" + ") || "empty mix";
    }
    const sweep =
      (s.kind === "sweepLog" || s.kind === "sweepLinear") && s.sweepS != null
        ? ` · repeats every ${s.sweepS} s`
        : "";
    const am = s.amHz != null ? ` · trem ${s.amHz} Hz` : "";
    const fm = s.fmHz != null ? ` · vib ${s.fmHz} Hz` : "";
    return `${s.levelDb} dB peak${sweep}${am}${fm}`;
  } catch {
    return "generated signal";
  }
};
type ClipRow = {
  id: number;
  trackId: number;
  kind: string;
  name: string;
  tIn: number;
  tOut: number;
  note: string | null;
  tags: string[];
  createdMs: number;
};

/** Band tags color the library (artboard: lows = cut-blue, highs = boost-orange). */
const clipDotColor = (tags: string[]) =>
  tags.includes("lows")
    ? "#3f6d9e"
    : tags.includes("highs")
      ? "#c85a13"
      : tags.includes("mids")
        ? "#3d7d43"
        : "#b3ac9c";
type LibraryState = { tracks: TrackRow[]; clips: ClipRow[] };
type ToolsState = { ffmpeg: string | null; ytdlp: string | null };
type TrackSess = {
  id: number;
  sess: number;
  title: string;
  durationS: number;
  mode: string; // "bypass" | "eq"
  exclusive?: boolean;
  gainDb?: number;
  device?: string;
  rate?: number;
  bits?: number;
  phase?: "decoding";
};

const fmtTime = (s: number) => {
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
};

/** Timecode with configurable decimals (Clip Studio room setting). */
const fmtTcN = (s: number, decimals: number) => {
  const m = Math.floor(s / 60);
  const sec = s - m * 60;
  return `${m}:${sec.toFixed(decimals).padStart(decimals > 0 ? 3 + decimals : 2, "0")}`;
};

type WaveData = { durationS: number; mins: number[]; maxs: number[] };

const RULER_H = 24;
const TICK_STEPS = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600];
const fmtTick = (s: number, step: number) => {
  const m = Math.floor(s / 60);
  const sec = s - m * 60;
  return step < 1
    ? `${m}:${sec.toFixed(1).padStart(4, "0")}`
    : `${m}:${Math.round(sec).toString().padStart(2, "0")}`;
};

/** The Clip Studio timeline (M4): a mini NLE lane for audio — adaptive
 *  timecode ruler, scroll = zoom around the cursor, drag = pan, click =
 *  seek, playhead follows playback (never during a gesture — the
 *  interaction-inert law). Peaks are Rust-computed at ~2.7 ms resolution so
 *  deep zooms stay sharp without refetching. */
type SampleWindow = { rate: number; startS: number; mono: number[] };
type SpecData = {
  cols: number;
  rows: number;
  durationS: number;
  minDb: number;
  maxDb: number;
  /** true = linear 0–20 kHz axis; false = log 20 Hz–20 kHz. */
  linear: boolean;
  data: string; // base64 u8 grid, row 0 = lowest frequency
};

/// The scope palette: ink ground into the boost-orange family (artboard).
const SPEC_PALETTE: [number, number, number][] = (() => {
  const stops: [number, [number, number, number]][] = [
    [0.0, [20, 23, 26]],
    [0.45, [90, 45, 20]],
    [0.7, [200, 90, 19]],
    [0.88, [232, 154, 95]],
    [1.0, [242, 239, 233]],
  ];
  const out: [number, number, number][] = [];
  for (let i = 0; i < 256; i++) {
    const t = i / 255;
    let k = 0;
    while (k < stops.length - 2 && t > stops[k + 1][0]) k++;
    const [t0, c0] = stops[k];
    const [t1, c1] = stops[k + 1];
    const f = Math.min(Math.max((t - t0) / (t1 - t0), 0), 1);
    out.push([
      Math.round(c0[0] + (c1[0] - c0[0]) * f),
      Math.round(c0[1] + (c1[1] - c0[1]) * f),
      Math.round(c0[2] + (c1[2] - c0[2]) * f),
    ]);
  }
  return out;
})();

/** Decode the spectrogram grid into an offscreen canvas (cols × rows px). */
function specToCanvas(spec: SpecData): HTMLCanvasElement {
  const bytes = Uint8Array.from(atob(spec.data), (c) => c.charCodeAt(0));
  const cv = document.createElement("canvas");
  cv.width = spec.cols;
  cv.height = spec.rows;
  const ctx = cv.getContext("2d")!;
  const img = ctx.createImageData(spec.cols, spec.rows);
  for (let r = 0; r < spec.rows; r++) {
    const srcRow = r * spec.cols;
    // row 0 = lowest frequency → bottom of the image
    const dstRow = (spec.rows - 1 - r) * spec.cols;
    for (let c = 0; c < spec.cols; c++) {
      const [rr, gg, bb] = SPEC_PALETTE[bytes[srcRow + c]];
      const o = (dstRow + c) * 4;
      img.data[o] = rr;
      img.data[o + 1] = gg;
      img.data[o + 2] = bb;
      img.data[o + 3] = 255;
    }
  }
  ctx.putImageData(img, 0, 0);
  return cv;
}
/// Below this span the timeline wants raw samples, not peak buckets.
const SAMPLE_SPAN = 2.0;
const MIN_SPAN = 0.0008; // ~38 samples @ 48 kHz — single-sample territory

function TimelineView({
  trackId,
  durationS,
  posS,
  onSeek,
  onHover,
  region,
  onRegionChange,
  spec,
  showSpec = false,
  showWave = true,
  heightPx,
  decimals = 1,
  syncKey,
  focus,
  onFocusDone,
}: {
  trackId: number;
  durationS: number;
  posS: number;
  onSeek: (s: number) => void;
  onHover?: (t: number | null) => void;
  region?: { a: number; b: number } | null;
  onRegionChange?: (r: { a: number; b: number }) => void;
  spec?: SpecData | null;
  showSpec?: boolean;
  showWave?: boolean;
  heightPx?: number;
  /// IN/OUT label precision (room setting).
  decimals?: number;
  /// Participate in cross-window view sync under this window label.
  syncKey?: string;
  /// A double-clicked clip: jump + zoom the view to this range once the
  /// waveform is ready (synced windows follow via the normal broadcast).
  focus?: { a: number; b: number; trackId: number } | null;
  onFocusDone?: () => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [wave, setWave] = useState<WaveData | null>(null);
  const [view, setView] = useState<{ start: number; span: number } | null>(null);
  const viewRef = useRef(view);
  viewRef.current = view;
  const [win, setWin] = useState<SampleWindow | null>(null);
  const winRef = useRef(win);
  winRef.current = win;
  const winReq = useRef(false);
  const drag = useRef<{ x0: number; start0: number; moved: boolean; edge?: "a" | "b" } | null>(
    null,
  );
  const follow = useRef(true);
  const regionRef = useRef(region);
  regionRef.current = region;
  const specCanvas = useRef<HTMLCanvasElement | null>(null);
  const specBytes = useRef<Uint8Array | null>(null);
  useEffect(() => {
    specCanvas.current = spec ? specToCanvas(spec) : null;
    specBytes.current = spec ? Uint8Array.from(atob(spec.data), (c) => c.charCodeAt(0)) : null;
  }, [spec]);
  // Cursor readout: where the pointer is in data terms (time / amplitude /
  // frequency+level), drawn on the canvas so satellites get it for free.
  const [cur, setCur] = useState<{ x: number; y: number } | null>(null);
  // The pane flexes now — redraw whenever the canvas box actually changes.
  const [resizeTick, setResizeTick] = useState(0);
  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setResizeTick((t) => t + 1));
    ro.observe(cv);
    return () => ro.disconnect();
  }, []);

  // Clamp the SPAN first, then anchor — clamping after anchoring made every
  // wheel notch at the zoom limit drift the view sideways.
  const clampView = (start: number, span: number) => {
    const s = Math.min(Math.max(span, MIN_SPAN), durationS || span);
    return { start: Math.min(Math.max(start, 0), Math.max(0, (durationS || s) - s)), span: s };
  };

  // Cross-window view sync: local changes broadcast; remote changes apply
  // without re-broadcasting (src guards the loop).
  const applyingExternal = useRef(false);
  const setViewSynced = (v: { start: number; span: number }) => {
    setView(v);
    if (syncKey && !applyingExternal.current) {
      emit("scope-view", { start: v.start, span: v.span, src: syncKey });
    }
  };
  useEffect(() => {
    if (!syncKey) return;
    const un = listen<{ start: number; span: number; src: string }>("scope-view", (e) => {
      if (e.payload.src === syncKey) return;
      applyingExternal.current = true;
      setView(clampView(e.payload.start, e.payload.span));
      applyingExternal.current = false;
    });
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [syncKey, durationS]);

  useEffect(() => {
    setWave(null);
    setView(null);
    setWin(null);
    follow.current = true;
    let alive = true;
    invoke<WaveData>("track_waveform", { id: trackId, buckets: 0 })
      .then((w) => {
        if (alive) {
          setWave(w);
          setView({ start: 0, span: w.durationS });
        }
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [trackId]);

  // A double-clicked clip: zoom to its range with breathing room. Waits for
  // the waveform (a fresh session resets the view when it lands), then fires
  // once — MainApp clears the request via onFocusDone.
  useEffect(() => {
    if (!focus || !wave || focus.trackId !== trackId) return;
    const len = Math.max(focus.b - focus.a, MIN_SPAN);
    const span = Math.min(len * 1.3, durationS || len * 1.3);
    setViewSynced(clampView(focus.a - len * 0.15, span));
    onFocusDone?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focus, wave]);

  // Deep zoom: fetch a raw-sample window (with margin) when the view drops
  // below the peak-bucket resolution; refetch only when the view escapes it.
  useEffect(() => {
    if (!view || view.span > SAMPLE_SPAN || winReq.current) return;
    const cur = winRef.current;
    const covered =
      cur &&
      cur.startS <= view.start &&
      view.start + view.span <= cur.startS + cur.mono.length / cur.rate;
    if (covered) return;
    winReq.current = true;
    const fetchStart = Math.max(0, view.start + view.span / 2 - 3);
    invoke<SampleWindow>("track_samples", { id: trackId, startS: fetchStart, spanS: 6.0 })
      .then((w) => setWin(w))
      .catch(() => {})
      .finally(() => {
        winReq.current = false;
      });
  }, [view, trackId]);

  // Zoom around the cursor — non-passive so the page never scrolls.
  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const v = viewRef.current;
      if (!v) return;
      const rect = cv.getBoundingClientRect();
      const frac = Math.min(Math.max((e.clientX - rect.left) / rect.width, 0), 1);
      const t = v.start + frac * v.span;
      const factor = e.deltaY < 0 ? 1 / 1.25 : 1.25;
      // Clamp span BEFORE anchoring so a wheel notch at the limit is a no-op.
      const span = Math.min(Math.max(v.span * factor, MIN_SPAN), durationS || v.span);
      setViewSynced(clampView(t - frac * span, span));
    };
    cv.addEventListener("wheel", onWheel, { passive: false });
    return () => cv.removeEventListener("wheel", onWheel);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [durationS]);

  // Keep the playhead in view while it advances past the right edge — but
  // never fight a pan: once the user moves the playhead out of view, follow
  // re-arms only when it's visible again.
  useEffect(() => {
    const v = viewRef.current;
    if (!v || drag.current || durationS <= 0) return;
    const end = v.start + v.span;
    if (follow.current && posS > end && posS <= durationS) {
      setViewSynced(clampView(posS - v.span * 0.1, v.span));
      return;
    }
    follow.current = posS >= v.start && posS <= end;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [posS]);

  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv) return;
    const dpr = window.devicePixelRatio || 1;
    const w = cv.clientWidth;
    const h = cv.clientHeight;
    if (cv.width !== Math.round(w * dpr)) cv.width = Math.round(w * dpr);
    if (cv.height !== Math.round(h * dpr)) cv.height = Math.round(h * dpr);
    const ctx = cv.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);

    // ruler ground
    ctx.fillStyle = "#ece8df";
    ctx.fillRect(0, 0, w, RULER_H);
    ctx.strokeStyle = "#b3ac9c";
    ctx.beginPath();
    ctx.moveTo(0, RULER_H - 0.5);
    ctx.lineTo(w, RULER_H - 0.5);
    ctx.stroke();

    if (!wave || !view) {
      ctx.fillStyle = "#8b8578";
      ctx.font = "11px 'IBM Plex Mono', monospace";
      ctx.fillText("building waveform…", 12, RULER_H + 24);
      return;
    }
    const { start, span } = view;
    const xOfT = (t: number) => ((t - start) / span) * w;

    // adaptive ticks: majors ≥ ~70 px apart, minors at a fifth
    const step = TICK_STEPS.find((s) => (s / span) * w >= 70) ?? 600;
    const minor = step / 5;
    ctx.font = "9px 'IBM Plex Mono', monospace";
    for (let t = Math.floor(start / minor) * minor; t <= start + span; t += minor) {
      if (t < 0) continue;
      const x = xOfT(t);
      const isMajor = Math.abs(t / step - Math.round(t / step)) < 1e-6;
      ctx.strokeStyle = "#c9c2b0";
      ctx.beginPath();
      ctx.moveTo(x, isMajor ? 8 : 15);
      ctx.lineTo(x, RULER_H - 1);
      ctx.stroke();
      if (isMajor) {
        ctx.fillStyle = "#8b8578";
        ctx.fillText(fmtTick(t, step), x + 3, 12);
      }
      // grid line through the lane on majors
      if (isMajor) {
        ctx.strokeStyle = "#eee9dd";
        ctx.beginPath();
        ctx.moveTo(x, RULER_H);
        ctx.lineTo(x, h);
        ctx.stroke();
      }
    }

    // lane layout: spectrogram on top (artboard order), waveform below
    const lanesTop = RULER_H;
    const lanesH = h - lanesTop;
    const specH = showSpec ? (showWave ? Math.round(lanesH * 0.55) : lanesH) : 0;
    if (showSpec) {
      ctx.fillStyle = "#14171a";
      ctx.fillRect(0, lanesTop, w, specH);
      const off = specCanvas.current;
      if (off && spec && spec.durationS > 0) {
        const sx = (start / spec.durationS) * spec.cols;
        const sw = (span / spec.durationS) * spec.cols;
        ctx.drawImage(off, sx, 0, sw, spec.rows, 0, lanesTop, w, specH);
        ctx.font = "9px 'IBM Plex Mono', monospace";
        ctx.fillStyle = "#8b8578";
        for (const f of spec.linear ? [5000, 10000, 15000] : [100, 1000, 10000]) {
          const frac = spec.linear
            ? f / 20000
            : (Math.log10(f) - Math.log10(20)) / (Math.log10(20000) - Math.log10(20));
          const yf = lanesTop + specH * (1 - frac);
          ctx.fillText(f >= 1000 ? `${f / 1000}k` : `${f}`, 4, yf + 3);
        }
      } else {
        ctx.fillStyle = "#8b8578";
        ctx.font = "11px 'IBM Plex Mono', monospace";
        ctx.fillText("building spectrogram…", 12, lanesTop + 24);
      }
    }

    // waveform lane: per-pixel min/max aggregated from the peak buckets
    const laneTop = lanesTop + specH + 4;
    const laneH = Math.max(h - laneTop - 4, 10);
    const mid = laneTop + laneH / 2;
    if (showWave) {
      ctx.strokeStyle = "#ddd6c4";
      ctx.beginPath();
      ctx.moveTo(0, mid);
      ctx.lineTo(w, mid);
      ctx.stroke();
    }
    // One renderer for every zoom (the Resolve look): per-pixel lo/hi values —
    // peak buckets zoomed out, raw-sample min/max zoomed in, linear
    // interpolation past one sample per pixel — then a single bridged draw
    // pass so adjacent columns always connect. No gaps, no mode pop.
    if (showWave) {
    const sampleWin =
      win &&
      span <= SAMPLE_SPAN &&
      win.startS <= start &&
      start + span <= win.startS + win.mono.length / win.rate
        ? win
        : null;
    const colLo = new Float32Array(w);
    const colHi = new Float32Array(w);
    let cols = 0;
    if (sampleWin) {
      const spp = (span * sampleWin.rate) / w;
      for (let x = 0; x < w; x++) {
        const t0 = start + (x / w) * span;
        if (t0 >= wave.durationS) break;
        if (spp >= 1) {
          const a = Math.floor((t0 - sampleWin.startS) * sampleWin.rate);
          const b = Math.max(a + 1, Math.floor((t0 + span / w - sampleWin.startS) * sampleWin.rate));
          let mn = 1;
          let mx = -1;
          for (let i = a; i < b; i++) {
            const v = sampleWin.mono[i] ?? 0;
            if (v < mn) mn = v;
            if (v > mx) mx = v;
          }
          colLo[x] = mn;
          colHi[x] = mx;
        } else {
          // Sub-sample zoom: interpolate the curve between neighboring samples.
          const p = (start + ((x + 0.5) / w) * span - sampleWin.startS) * sampleWin.rate;
          const i = Math.floor(p);
          const frac = p - i;
          const v0 = sampleWin.mono[i] ?? 0;
          const v1 = sampleWin.mono[i + 1] ?? v0;
          const v = v0 + (v1 - v0) * frac;
          colLo[x] = v;
          colHi[x] = v;
        }
        cols = x + 1;
      }
    } else {
      const n = wave.mins.length;
      const bucketDur = wave.durationS / n;
      for (let x = 0; x < w; x++) {
        const t0 = start + (x / w) * span;
        if (t0 >= wave.durationS) break;
        let i0 = Math.floor(t0 / bucketDur);
        let i1 = Math.min(Math.ceil((t0 + span / w) / bucketDur), n);
        if (i0 >= n) break;
        if (i1 <= i0) i1 = i0 + 1;
        let mn = 1;
        let mx = -1;
        for (let i = i0; i < i1; i++) {
          if (wave.mins[i] < mn) mn = wave.mins[i];
          if (wave.maxs[i] > mx) mx = wave.maxs[i];
        }
        colLo[x] = mn;
        colHi[x] = mx;
        cols = x + 1;
      }
    }
    ctx.fillStyle = "#55503f";
    for (let x = 0; x < cols; x++) {
      let lo = colLo[x];
      let hi = colHi[x];
      if (x > 0) {
        // Bridge to the neighbor's range so the contour never breaks.
        if (lo > colHi[x - 1]) lo = colHi[x - 1];
        if (hi < colLo[x - 1]) hi = colLo[x - 1];
      }
      const y1 = mid - hi * (laneH / 2);
      const y2 = mid - lo * (laneH / 2);
      ctx.fillRect(x, y1, 1, Math.max(1.2, y2 - y1));
    }
    }

    // the I/O region: shaded span, edge grips, IN/OUT/Δ readouts (artboard)
    if (region && region.b > region.a) {
      const xa = xOfT(region.a);
      const xb = xOfT(region.b);
      if (xb > 0 && xa < w) {
        ctx.fillStyle = "rgba(200, 90, 19, 0.10)";
        ctx.fillRect(Math.max(xa, 0), RULER_H, Math.min(xb, w) - Math.max(xa, 0), h - RULER_H);
        ctx.strokeStyle = "#c85a13";
        ctx.lineWidth = 1.4;
        for (const x of [xa, xb]) {
          if (x < 0 || x > w) continue;
          ctx.beginPath();
          ctx.moveTo(x, RULER_H);
          ctx.lineTo(x, h);
          ctx.stroke();
        }
        ctx.lineWidth = 1;
        // edge grips (in points right, out points left — Resolve idiom)
        ctx.fillStyle = "#c85a13";
        if (xa >= 0 && xa <= w) {
          ctx.beginPath();
          ctx.moveTo(xa, RULER_H + 2);
          ctx.lineTo(xa + 7, RULER_H + 8);
          ctx.lineTo(xa, RULER_H + 14);
          ctx.closePath();
          ctx.fill();
        }
        if (xb >= 0 && xb <= w) {
          ctx.beginPath();
          ctx.moveTo(xb, RULER_H + 2);
          ctx.lineTo(xb - 7, RULER_H + 8);
          ctx.lineTo(xb, RULER_H + 14);
          ctx.closePath();
          ctx.fill();
        }
        ctx.font = "10px 'IBM Plex Mono', monospace";
        ctx.fillStyle = "#c85a13";
        ctx.fillText(`IN ${fmtTcN(region.a, decimals)}`, Math.max(xa, 0) + 4, h - 8);
        const outLabel = `OUT ${fmtTcN(region.b, decimals)}`;
        ctx.fillText(outLabel, Math.min(xb, w) - ctx.measureText(outLabel).width - 4, h - 8);
        ctx.fillStyle = "#8b8578";
        const dLabel = `Δ ${(region.b - region.a).toFixed(1)}s`;
        ctx.fillText(dLabel, (Math.max(xa, 0) + Math.min(xb, w)) / 2 - ctx.measureText(dLabel).width / 2, h - 8);
      }
    }

    // playhead: line through the lane + a grip on the ruler
    if (posS >= start && posS <= start + span) {
      const x = xOfT(posS);
      ctx.strokeStyle = "#14171a";
      ctx.lineWidth = 1.6;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
      ctx.stroke();
      ctx.lineWidth = 1;
      ctx.fillStyle = "#14171a";
      ctx.beginPath();
      ctx.moveTo(x - 5, 0);
      ctx.lineTo(x + 5, 0);
      ctx.lineTo(x, 8);
      ctx.closePath();
      ctx.fill();
    }

    // cursor readout — the coordinates under the pointer, in data terms:
    // time everywhere; + frequency/level in the spectrogram lane (z from the
    // tile grid); + amplitude/dBFS in the waveform lane.
    if (cur && cur.y > RULER_H && cur.x >= 0 && cur.x <= w) {
      const tC = Math.min(Math.max(start + (cur.x / w) * span, 0), durationS || 0);
      let label: string;
      if (showSpec && cur.y >= lanesTop && cur.y < lanesTop + specH) {
        const frac = 1 - (cur.y - lanesTop) / specH; // bottom → top of the freq axis
        const f = spec?.linear
          ? frac * 20000
          : 10 ** (Math.log10(20) + frac * (Math.log10(20000) - Math.log10(20)));
        let z = "";
        const bytes = specBytes.current;
        if (spec && bytes && spec.durationS > 0) {
          const c = Math.min(Math.max(Math.floor((tC / spec.durationS) * spec.cols), 0), spec.cols - 1);
          const r = Math.min(Math.max(Math.floor(frac * spec.rows), 0), spec.rows - 1);
          const v = bytes[r * spec.cols + c];
          z =
            v === 0
              ? ` · ≤${spec.minDb.toFixed(0)} dB`
              : ` · ${(spec.minDb + (v / 255) * (spec.maxDb - spec.minDb)).toFixed(1)} dB`;
        }
        label = `${fmtTcN(tC, decimals)} · ${fmtHz(f)}Hz${z}`;
      } else if (showWave && cur.y >= laneTop) {
        const amp = (mid - cur.y) / (laneH / 2);
        const dbfs =
          Math.abs(amp) > 1e-4 ? `${(20 * Math.log10(Math.abs(amp))).toFixed(1)} dBFS` : "−∞";
        label = `${fmtTcN(tC, decimals)} · ${amp >= 0 ? "+" : ""}${amp.toFixed(2)} (${dbfs})`;
      } else {
        label = fmtTcN(tC, decimals);
      }
      ctx.font = "10px 'IBM Plex Mono', monospace";
      const tw = ctx.measureText(label).width;
      const bx = w - tw - 14;
      const by = h - 24;
      ctx.fillStyle = "rgba(242, 239, 233, 0.92)";
      ctx.fillRect(bx - 5, by - 11, tw + 10, 15);
      ctx.strokeStyle = "#c9c2b0";
      ctx.strokeRect(bx - 5.5, by - 11.5, tw + 11, 16);
      ctx.fillStyle = "#55503f";
      ctx.fillText(label, bx, by);
    }
  }, [wave, view, win, posS, durationS, region, spec, showSpec, showWave, decimals, cur, resizeTick]);

  return (
    <canvas
      ref={canvasRef}
      className="timeline"
      style={heightPx ? { height: heightPx } : undefined}
      onPointerDown={(e) => {
        const v = viewRef.current;
        if (!v) return;
        // Grabbing an I/O edge (±6 px) trims the region instead of panning.
        let edge: "a" | "b" | undefined;
        const r = regionRef.current;
        if (r && onRegionChange) {
          const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
          const xOf = (t: number) => ((t - v.start) / v.span) * rect.width + rect.left;
          if (Math.abs(e.clientX - xOf(r.a)) <= 6) edge = "a";
          else if (Math.abs(e.clientX - xOf(r.b)) <= 6) edge = "b";
        }
        drag.current = { x0: e.clientX, start0: v.start, moved: false, edge };
        (e.currentTarget as Element).setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        const v = viewRef.current;
        if (!v) return;
        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const tAt = Math.min(
          Math.max(v.start + ((e.clientX - rect.left) / rect.width) * v.span, 0),
          durationS || 0,
        );
        // C = seek to the cursor: report where the mouse hovers, always.
        onHover?.(tAt);
        setCur({ x: e.clientX - rect.left, y: e.clientY - rect.top });
        const d = drag.current;
        if (!d) {
          // Near an I/O edge → the resize cursor announces the trim grab.
          let cursor = "crosshair";
          const r = regionRef.current;
          if (r && onRegionChange) {
            const xOf = (t: number) => ((t - v.start) / v.span) * rect.width + rect.left;
            if (Math.abs(e.clientX - xOf(r.a)) <= 6 || Math.abs(e.clientX - xOf(r.b)) <= 6) {
              cursor = "ew-resize";
            }
          }
          (e.currentTarget as HTMLElement).style.cursor = cursor;
          return;
        }
        if (d.edge) {
          const r = regionRef.current;
          if (r && onRegionChange) {
            if (d.edge === "a") onRegionChange({ a: Math.min(tAt, r.b - 0.05), b: r.b });
            else onRegionChange({ a: r.a, b: Math.max(tAt, r.a + 0.05) });
          }
          d.moved = true;
          return;
        }
        const dx = e.clientX - d.x0;
        if (!d.moved && Math.abs(dx) < 5) return;
        d.moved = true;
        setViewSynced(clampView(d.start0 - (dx / rect.width) * v.span, v.span));
      }}
      onPointerLeave={() => {
        onHover?.(null);
        setCur(null);
      }}
      onPointerUp={(e) => {
        const d = drag.current;
        drag.current = null;
        const v = viewRef.current;
        if (!d || !v) return;
        if (!d.moved && !d.edge && durationS > 0) {
          const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
          const t = v.start + ((e.clientX - rect.left) / rect.width) * v.span;
          follow.current = true;
          onSeek(Math.min(Math.max(t, 0), durationS));
        }
      }}
    />
  );
}

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
  // Pop-out windows render only their view (fed over events). Constant per
  // window lifetime, so the early returns are hook-safe.
  if (IS_HISTORY_WINDOW) return <PopoutHistory />;
  if (IS_DIFF_WINDOW) return <PopoutDiff />;
  if (IS_SCOPE_SPEC_WINDOW) return <PopoutScopeSpec />;
  if (IS_SCOPE_FFT_WINDOW) return <PopoutScopeFft />;
  // eslint-disable-next-line react-hooks/rules-of-hooks
  return <MainApp />;
}

function MainApp() {
  const [state, setState] = useState<EqState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [, setSelected] = useState<number | null>(null);
  const [multiSel, setMultiSel] = useState<ReadonlySet<number>>(new Set());
  const multiSelRef = useRef<ReadonlySet<number>>(new Set());
  multiSelRef.current = multiSel;
  const anchorSelRef = useRef<number | null>(null);

  const selectOnly = (i: number | null) => {
    setSelected(i);
    setMultiSel(i == null ? new Set() : new Set([i]));
    anchorSelRef.current = i;
  };
  const [presets, setPresets] = useState<PresetsState>({ presets: [], active: null });
  const [menuOpen, setMenuOpen] = useState(false);
  const [eqMenu, setEqMenu] = useState(false);
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

  const [ab, setAb] = useState<AbInfo>({ side: "a", matchDb: 0, shortfallDb: 0 });
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

  // ---- Clip Studio (M2: library + track engine transport) ----
  const [view, setView] = useState<"eq" | "lab" | "settings" | "clips">("eq");
  const viewModeRef = useRef(view);
  viewModeRef.current = view;
  const [library, setLibrary] = useState<LibraryState | null>(null);
  const libraryRef = useRef<LibraryState | null>(null);
  libraryRef.current = library;
  const [tools, setTools] = useState<ToolsState | null>(null);
  const [toolsProg, setToolsProg] = useState<{ which: string; pct: number | null } | null>(null);
  const [trackSess, setTrackSess] = useState<TrackSess | null>(null);
  const trackSessRef = useRef<TrackSess | null>(null);
  trackSessRef.current = trackSess;
  const [trackPos, setTrackPos] = useState<{ posS: number; paused: boolean }>({ posS: 0, paused: false });
  const [studioMode, setStudioMode] = useState<"bypass" | "eq">("bypass");
  const loadStudioMode = () =>
    invoke<{ mode: string }>("studio_state")
      .then((s) => setStudioMode(s.mode === "eq" ? "eq" : "bypass"))
      .catch(() => {});
  const pickStudioMode = (m: "bypass" | "eq") => {
    setStudioMode(m);
    invoke("set_studio_mode", { mode: m }).catch(() => {});
  };
  const playPending = useRef(false);
  /// C = seek to cursor (Resolve): where the mouse hovers on the timeline.
  const timelineHover = useRef<number | null>(null);
  const trackPosRef = useRef(trackPos);
  trackPosRef.current = trackPos;
  // The I/O region (Resolve: I = in at playhead, O = out) — clip raw material.
  const [ioRegion, setIoRegion] = useState<{ a: number; b: number } | null>(null);
  const ioRegionRef = useRef(ioRegion);
  ioRegionRef.current = ioRegion;
  const [loopOn, setLoopOn] = useState(false);
  // Which saved clip the region currently IS (by identity, not by time —
  // duplicated ranges must not co-highlight).
  const [activeClipId, setActiveClipId] = useState<number | null>(null);
  // A clip clicked before its track was playing: applied on session start.
  const pendingClip = useRef<ClipRow | null>(null);
  const [clipRename, setClipRename] = useState<string | null>(null);
  // The scopes: combinable view toggles (artboard: Spec ✓ Wave ✓ FFT ✓).
  const scopeInit = (key: string) => {
    try {
      return localStorage.getItem(key) !== "0";
    } catch {
      return true;
    }
  };
  const [specOn, setSpecOn] = useState(() => scopeInit("fletcher.scope.spec"));
  const [waveOn, setWaveOn] = useState(() => scopeInit("fletcher.scope.wave"));
  const [fftOn, setFftOn] = useState(() => scopeInit("fletcher.scope.fft"));
  const toggleScope = (which: "spec" | "wave" | "fft") => {
    const [get, set] =
      which === "spec"
        ? [specOn, setSpecOn]
        : which === "wave"
          ? [waveOn, setWaveOn]
          : [fftOn, setFftOn];
    set(!get);
    try {
      localStorage.setItem(`fletcher.scope.${which}`, get ? "0" : "1");
    } catch {
      /* per-viewer nicety only */
    }
  };
  const [specData, setSpecData] = useState<SpecData | null>(null);
  // Panes leave the main page while they live in their own window.
  const [popped, setPopped] = useState({ spec: false, fft: false });
  const [clipsMenu, setClipsMenu] = useState(false);
  // Timecode precision — Clip Studio's first room setting.
  const [tcDec, setTcDec] = useState<number>(() => {
    try {
      const v = Number(localStorage.getItem("fletcher.tcdec"));
      return [1, 2, 3].includes(v) ? v : 2;
    } catch {
      return 2;
    }
  });
  const pickTcDec = (v: number) => {
    setTcDec(v);
    try {
      localStorage.setItem("fletcher.tcdec", String(v));
    } catch {
      /* per-viewer nicety only */
    }
  };
  // Spectrogram parameters (room settings; satellites read the same keys).
  const lsNum = (key: string, dflt: number, allowed: number[]) => {
    try {
      const v = Number(localStorage.getItem(key));
      return allowed.includes(v) ? v : dflt;
    } catch {
      return dflt;
    }
  };
  const [specWin, setSpecWin] = useState(() =>
    lsNum("fletcher.specwin", 2048, [256, 512, 1024, 2048, 4096, 8192]),
  );
  const [specFloor, setSpecFloor] = useState(() =>
    lsNum("fletcher.specfloor", -90, [-70, -90, -110]),
  );
  const [specLinear, setSpecLinear] = useState(() => {
    try {
      return localStorage.getItem("fletcher.speclinear") === "1";
    } catch {
      return false;
    }
  });
  const pickSpecParam = (key: "specwin" | "specfloor" | "speclinear", v: number) => {
    (key === "specwin" ? setSpecWin : key === "specfloor" ? setSpecFloor : (x: number) => setSpecLinear(x === 1))(v);
    try {
      localStorage.setItem(`fletcher.${key}`, String(v));
    } catch {
      /* per-viewer nicety only */
    }
    // localStorage doesn't notify other webviews — broadcast the change so a
    // popped-out spectrogram re-renders with the new parameters.
    emit("spec-params", {
      win: key === "specwin" ? v : specWin,
      floor: key === "specfloor" ? v : specFloor,
      linear: key === "speclinear" ? v === 1 : specLinear,
    });
  };
  // Scrub feel — typed-in, not preset. Chase = the scrub's reaction time in ms
  // (it closes ~63% of the gap to your cursor per chase interval); max = the
  // catch-up speed ceiling in multiples of real time.
  const lsRange = (key: string, dflt: number, lo: number, hi: number) => {
    try {
      const v = Number(localStorage.getItem(key));
      return Number.isFinite(v) && v >= lo && v <= hi ? v : dflt;
    } catch {
      return dflt;
    }
  };
  const [scrubTau, setScrubTau] = useState(() => lsRange("fletcher.scrubtau", 60, 5, 1000));
  const [scrubMax, setScrubMax] = useState(() => lsRange("fletcher.scrubmax", 8, 0.5, 64));
  const commitScrub = (which: "tau" | "max", raw: number) => {
    if (!Number.isFinite(raw)) return;
    if (which === "tau") setScrubTau(Math.min(1000, Math.max(5, raw)));
    else setScrubMax(Math.min(64, Math.max(0.5, raw)));
  };
  useEffect(() => {
    // Live in the engine (mid-scrub included) and remembered across sessions.
    invoke("track_scrub_params", { tauMs: scrubTau, maxSpeed: scrubMax }).catch(() => {});
    try {
      localStorage.setItem("fletcher.scrubtau", String(scrubTau));
      localStorage.setItem("fletcher.scrubmax", String(scrubMax));
    } catch {
      /* per-viewer nicety only */
    }
  }, [scrubTau, scrubMax]);

  // View presets — one-click setups of the whole scope stack. "Melody" is
  // the user's discovery (2026-08-23): 8k window + spectrogram solo + a tight
  // time zoom turns vocal lines into readable pitch traces — "the lines
  // literally look like the song".
  const applyViewPreset = (which: "melody" | "standard") => {
    if (which === "melody") {
      if (!specOn) toggleScope("spec");
      if (waveOn) toggleScope("wave");
      if (fftOn) toggleScope("fft");
      pickSpecParam("specwin", 8192);
      pickSpecParam("speclinear", 0); // pitch is log-natural
      const dur = trackSessRef.current?.durationS ?? 0;
      if (dur > 0) {
        // ~1 ms wide (user-calibrated): the view is essentially one FFT
        // column stretched across the screen — harmonics become horizontal
        // lines, and playhead-follow makes them ride the melody live.
        const span = Math.min(0.001, dur);
        const start = Math.max(0, Math.min(trackPosRef.current.posS - span / 2, dur - span));
        emit("scope-view", { start, span, src: "preset" });
      }
    } else {
      if (!specOn) toggleScope("spec");
      if (!waveOn) toggleScope("wave");
      if (!fftOn) toggleScope("fft");
      pickSpecParam("specwin", 2048);
      const dur = trackSessRef.current?.durationS ?? 0;
      if (dur > 0) emit("scope-view", { start: 0, span: dur, src: "preset" });
    }
  };

  // The satellites' ⚙ menus mirror this state — rebroadcast on any change.
  useEffect(() => {
    emit("room-state", {
      tcDec,
      specOn,
      waveOn,
      fftOn,
      specWin,
      specFloor,
      specLinear,
      mode: studioMode,
      scrubTau,
      scrubMax,
    });
  }, [tcDec, specOn, waveOn, fftOn, specWin, specFloor, specLinear, studioMode, scrubTau, scrubMax]);

  // Audible scrub (Resolve): while C is held the cursor IS the playhead —
  // each move sounds a short burst at the new spot, stillness holds silent,
  // and releasing C hands the transport back exactly as it was.
  const scrub = useRef(false);
  const scrubStart = () => {
    const sess = trackSessRef.current;
    if (scrub.current || !sess || sess.phase || timelineHover.current == null) return;
    scrub.current = true;
    invoke("track_scrub", { on: true }).catch(() => {});
    invoke("track_seek", { seconds: timelineHover.current }).catch(() => {});
  };
  const scrubMove = (t: number) => {
    if (scrub.current) invoke("track_seek", { seconds: t }).catch(() => {});
  };
  const scrubEnd = () => {
    if (!scrub.current) return;
    scrub.current = false;
    invoke("track_scrub", { on: false }).catch(() => {});
  };
  useEffect(() => {
    setSpecData(null);
    if (!trackSess || trackSess.phase) return;
    let alive = true;
    invoke<SpecData>("track_spectrogram", {
      id: trackSess.id,
      win: specWin,
      floorDb: specFloor,
      linear: specLinear,
    })
      .then((s) => {
        if (alive) setSpecData(s);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [trackSess?.id, trackSess?.phase, specWin, specFloor, specLinear]);

  const popOutScope = async (kind: "spec" | "fft") => {
    try {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const label = `scope-${kind}`;
      const existing = await WebviewWindow.getByLabel(label);
      if (existing) {
        await existing.show().catch(() => {});
        await existing.setFocus();
      } else {
        new WebviewWindow(label, {
          url: `index.html?view=${label}`,
          title: kind === "spec" ? "Fletcher — Spectrogram" : "Fletcher — FFT",
          width: 960,
          height: kind === "spec" ? 440 : 320,
        });
      }
      setPopped((p) => ({ ...p, [kind]: true }));
    } catch (e) {
      showNotice(String(e));
    }
  };
  const activeClip = library?.clips.find((c) => c.id === activeClipId) ?? null;
  const updateClip = (id: number, patch: { name?: string; note?: string }) =>
    invoke<LibraryState>("clip_update", { id, ...patch })
      .then(setLibrary)
      .catch((e) => showNotice(String(e)));
  const toggleTag = (c: ClipRow, tag: string) =>
    invoke<LibraryState>("clip_tag", { id: c.id, tag, on: !c.tags.includes(tag) })
      .then(setLibrary)
      .catch((e) => showNotice(String(e)));

  const setIn = (t: number) => {
    setActiveClipId(null);
    setIoRegion((cur) => {
      const b = cur && cur.b > t ? cur.b : (trackSessRef.current?.durationS ?? t + 1);
      return { a: t, b };
    });
  };
  const setOut = (t: number) => {
    setActiveClipId(null);
    setIoRegion((cur) => ({ a: cur && cur.a < t ? cur.a : 0, b: t }));
  };
  const clearRegion = () => {
    setIoRegion(null);
    setLoopOn(false);
    setActiveClipId(null);
  };
  // Satellites render the region too — broadcast every change.
  useEffect(() => {
    emit("io-region", ioRegion ?? null);
  }, [ioRegion]);

  // The engine's loop follows the region + toggle (session-scoped).
  useEffect(() => {
    if (!trackSess || trackSess.phase) return;
    const r = ioRegion;
    invoke("track_loop", {
      aS: r?.a ?? 0,
      bS: r?.b ?? 0,
      on: loopOn && !!r,
    }).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ioRegion, loopOn, trackSess?.sess, trackSess?.phase]);

  const saveClip = () => {
    const r = ioRegionRef.current;
    const sess = trackSessRef.current;
    if (!r || !sess) return;
    invoke<LibraryState>("clip_create", { trackId: sess.id, tIn: r.a, tOut: r.b })
      .then((l) => {
        setLibrary(l);
        // The newborn clip becomes active — name/tags/note are one strip away.
        const newest = l.clips
          .filter((c) => c.trackId === sess.id)
          .reduce<ClipRow | null>((best, c) => (best && best.id > c.id ? best : c), null);
        if (newest) setActiveClipId(newest.id);
      })
      .catch((e) => showNotice(String(e)));
  };

  /** Click a clip: solo it on loop (play its track first if needed). */
  const openClip = (c: ClipRow) => {
    const sess = trackSessRef.current;
    if (sess && sess.id === c.trackId && !sess.phase) {
      setIoRegion({ a: c.tIn, b: c.tOut });
      setLoopOn(true);
      setActiveClipId(c.id);
      invoke("track_seek", { seconds: c.tIn }).catch(() => {});
    } else {
      pendingClip.current = c;
      const t = libraryRef.current?.tracks.find((x) => x.id === c.trackId);
      if (t) playTrack(t);
    }
  };

  // Double-click: open the clip AND zoom the timeline to it (satellites
  // follow through the normal view-sync broadcast).
  const [clipFocus, setClipFocus] = useState<{ a: number; b: number; trackId: number } | null>(
    null,
  );
  const focusClip = (c: ClipRow) => {
    openClip(c);
    setClipFocus({ a: c.tIn, b: c.tOut, trackId: c.trackId });
  };

  // The signal generator: primitives → library items (kind='signal'). The
  // recipe lives on the row; audio synthesizes deterministically on first use.
  const [genOpen, setGenOpen] = useState(false);
  const [genSpec, setGenSpec] = useState<SigSpec>({ kind: "pink", seconds: 30, levelDb: -18 });
  const [genPreview, setGenPreview] = useState(false);
  const genPreviewRef = useRef(genPreview);
  genPreviewRef.current = genPreview;
  // The serial of the preview we most recently started: a replaced stream's
  // "ended" event arrives late and must not flip the button off.
  const genSerial = useRef(0);
  const genPreviewSet = (spec: SigSpec | null) => {
    invoke<number>("signal_preview", { spec })
      .then((n) => {
        genSerial.current = n ?? 0;
        setGenPreview(!!spec);
      })
      .catch((e) => {
        setGenPreview(false);
        showNotice(String(e));
      });
  };
  // Param edits retune a running preview live.
  const mutateGen = (fn: (cur: SigSpec) => SigSpec) => {
    setGenSpec((cur) => {
      const next = fn(cur);
      if (genPreviewRef.current) {
        invoke<number>("signal_preview", { spec: next })
          .then((n) => {
            genSerial.current = n ?? 0;
          })
          .catch(() => {});
      }
      return next;
    });
  };
  const updateGen = (patch: Partial<SigSpec>) => mutateGen((cur) => ({ ...cur, ...patch }));
  // Mix mode: the primitive controls below the layer list edit the SELECTED layer.
  const [selLayer, setSelLayer] = useState(0);
  const isMix = genSpec.kind === "mix";
  const curSpec: SigSpec = isMix ? (genSpec.layers?.[selLayer] ?? genSpec) : genSpec;
  const updateCur = (patch: Partial<SigSpec>) =>
    isMix
      ? mutateGen((cur) => ({
          ...cur,
          layers: (cur.layers ?? []).map((l, i) => (i === selLayer ? { ...l, ...patch } : l)),
        }))
      : updateGen(patch);
  const kindDefaults = (kind: SigSpec["kind"], base: SigSpec): Partial<SigSpec> =>
    kind === "sine"
      ? { hz: base.hz ?? 1000 }
      : kind === "sweepLog" || kind === "sweepLinear"
        ? { fromHz: base.fromHz ?? 20, toHz: base.toHz ?? 20000 }
        : kind === "band"
          ? { loHz: base.loHz ?? 500, hiHz: base.hiHz ?? 2000 }
          : {};
  const pickGenKind = (kind: SigSpec["kind"]) => {
    if (kind === "mix") {
      if (isMix) return;
      // The current primitive becomes layer 1.
      mutateGen((cur) => ({
        kind: "mix",
        seconds: cur.seconds,
        levelDb: cur.levelDb,
        layers: [{ ...cur }],
      }));
      setSelLayer(0);
    } else if (isMix) {
      // Leaving mix: the selected layer survives, retyped.
      mutateGen((cur) => {
        const base = cur.layers?.[selLayer] ?? cur;
        return { ...base, kind, ...kindDefaults(kind, base), seconds: cur.seconds, layers: undefined };
      });
    } else {
      updateGen({ kind, ...kindDefaults(kind, genSpec) });
    }
  };
  const addLayer = () => {
    setSelLayer(genSpec.layers?.length ?? 0);
    mutateGen((cur) => ({
      ...cur,
      layers: [...(cur.layers ?? []), { kind: "pink", seconds: cur.seconds, levelDb: -24 }],
    }));
  };
  const removeLayer = (idx: number) => {
    setSelLayer((s) => Math.max(0, s >= idx ? s - 1 : s));
    mutateGen((cur) => ({ ...cur, layers: (cur.layers ?? []).filter((_, i) => i !== idx) }));
  };
  // The text tier: the recipe JSON itself — validate + apply. This format is
  // the future API/MCP surface, so the editor IS the power path.
  const [genText, setGenText] = useState<string | null>(null);
  const [genErr, setGenErr] = useState<string | null>(null);
  const applyGenText = () => {
    let parsed: SigSpec;
    try {
      parsed = JSON.parse(genText ?? "");
    } catch (e) {
      setGenErr(`not valid JSON — ${e}`);
      return;
    }
    invoke<string>("signal_validate", { spec: parsed })
      .then(() => {
        mutateGen(() => parsed);
        setSelLayer(0);
        setGenText(null);
        setGenErr(null);
      })
      .catch((e) => setGenErr(String(e)));
  };

  // URL import (M7): paste a link, yt-dlp extracts the audio into the
  // managed media dir, provenance kept on the track row.
  const [urlOpen, setUrlOpen] = useState(false);
  const [urlText, setUrlText] = useState("");
  const [urlBusy, setUrlBusy] = useState<number | null>(null);
  useEffect(() => {
    if (!urlOpen) return;
    const un = listen<number>("ytdlp-progress", (e) => setUrlBusy(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, [urlOpen]);
  const openUrl = () => {
    setUrlText("");
    setUrlBusy(null);
    setUrlOpen(true);
    // Prefill when the clipboard already holds a link — the paste flow.
    navigator.clipboard
      ?.readText?.()
      .then((t) => {
        if (/^https?:\/\//.test(t.trim())) setUrlText(t.trim());
      })
      .catch(() => {});
  };
  const importUrl = () => {
    if (urlBusy != null || !urlText.trim()) return;
    setUrlBusy(0);
    invoke<LibraryState>("track_import_url", { url: urlText.trim() })
      .then((l) => {
        setLibrary(l);
        setUrlOpen(false);
        setUrlBusy(null);
        showNotice("added to the library — audio extracted, source link kept");
      })
      .catch((e) => {
        setUrlBusy(null);
        showNotice(String(e));
      });
  };
  // Editing an existing created signal reuses the whole modal; saving
  // replaces the recipe and invalidates every derived artifact.
  const [genEditId, setGenEditId] = useState<number | null>(null);
  const openGenEdit = (t: TrackRow) => {
    try {
      const spec = JSON.parse(t.signalParams ?? "") as SigSpec;
      setGenSpec(spec);
      setSelLayer(0);
      setGenText(null);
      setGenErr(null);
      setGenEditId(t.id);
      setGenOpen(true);
    } catch {
      showNotice("this signal's recipe is unreadable — delete and recreate it");
    }
  };
  const closeGen = () => {
    if (genPreviewRef.current) genPreviewSet(null);
    setGenOpen(false);
    setGenEditId(null);
  };
  const addSignal = () => {
    const editing = genEditId != null;
    (editing
      ? invoke<LibraryState>("signal_update", { id: genEditId, spec: genSpec })
      : invoke<LibraryState>("signal_create", { spec: genSpec })
    )
      .then((l) => {
        setLibrary(l);
        closeGen();
        showNotice(
          editing
            ? "signal updated — it re-renders the next time it plays"
            : "added to the library — it synthesizes the first time you play it",
        );
      })
      .catch((e) => showNotice(String(e)));
  };
  // If anything else claims the aux stream (track play, calibration), the
  // preview thread ends and tells us — un-stick the button.
  useEffect(() => {
    if (!genOpen) return;
    const un = listen<number>("sig-preview-ended", (e) => {
      // Only the CURRENT preview's ending un-sticks the button — a replaced
      // stream's late "ended" is stale.
      if ((e.payload ?? 0) >= genSerial.current) setGenPreview(false);
    });
    return () => {
      un.then((f) => f());
    };
  }, [genOpen]);

  const loadLibrary = () =>
    invoke<LibraryState>("library_state").then(setLibrary).catch((e) => showNotice(String(e)));
  const loadTools = () => invoke<ToolsState>("tools_state").then(setTools).catch(() => {});
  const titleOf = (id: number) =>
    libraryRef.current?.tracks.find((t) => t.id === id)?.title ?? `track #${id}`;

  const importTrack = async () => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({
        multiple: false,
        filters: [
          {
            name: "Audio",
            extensions: ["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "wma", "aiff", "webm", "mp4", "mkv"],
          },
        ],
      });
      if (typeof path !== "string") return;
      setLibrary(await invoke<LibraryState>("track_import", { path }));
    } catch (e) {
      showNotice(String(e));
    }
  };

  const playTrack = async (t: TrackRow) => {
    if (playPending.current) return; // one handoff at a time — no stream overlap
    playPending.current = true;
    try {
      await invoke("track_play", { id: t.id });
    } catch (e) {
      showNotice(String(e));
    } finally {
      playPending.current = false;
    }
  };

  const installTool = (which: "ffmpeg" | "yt-dlp") => {
    setToolsProg({ which, pct: 0 });
    invoke<ToolsState>("tools_install", { which })
      .then((t) => {
        setTools(t);
        setToolsProg(null);
        showNotice(`${which} installed — it lives in Fletcher's tools folder and never touches your system`);
      })
      .catch((e) => {
        setToolsProg(null);
        showNotice(String(e));
      });
  };

  // ---- Settings (v1: the approved artboard) ----
  const [settings, setSettings] = useState<SettingsState | null>(null);
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [calNoise, setCalNoise] = useState(false);
  const toggleCalNoise = () => {
    const next = !calNoise;
    setCalNoise(next);
    invoke("calibration_noise", { on: next }).catch((e) => {
      showNotice(String(e));
      setCalNoise(false);
    });
  };
  const [testingTone, setTestingTone] = useState(false);
  const testTone = (mode: "exclusive" | "shared") => {
    setTestingTone(true);
    invoke<string>("engine_test_tone", { mode })
      .then((info) => showNotice(`track engine OK — ${info}`))
      .catch((e) => showNotice(String(e)))
      .finally(() => setTestingTone(false));
  };
  const loadSettings = () => invoke<SettingsState>("settings_state").then(setSettings).catch(() => {});
  const loadAutostart = () =>
    import("@tauri-apps/plugin-autostart")
      .then((m) => m.isEnabled())
      .then(setAutostart)
      .catch(() => setAutostart(null));
  const toggleAutostart = async () => {
    try {
      const m = await import("@tauri-apps/plugin-autostart");
      if (await m.isEnabled()) await m.disable();
      else await m.enable();
      setAutostart(await m.isEnabled());
    } catch (e) {
      showNotice(String(e));
    }
  };
  const setReference = (db: number) =>
    invoke<EqState>("set_reference_db", { db })
      .then((s) => {
        setState(s);
        loadSettings();
        refreshPresets();
      })
      .catch((e) => showNotice(String(e)));
  const toggleLevelMatching = () => {
    if (!settings) return;
    invoke<EqState>("set_level_matching", { on: !settings.levelMatching })
      .then((s) => {
        setState(s);
        loadSettings();
        refreshPresets();
      })
      .catch((e) => showNotice(String(e)));
  };
  // Display-only preferences live client-side (like the y-scale).
  const [ordering, setOrdering] = useState<"freq" | "importance" | "file">(() => {
    try {
      const v = localStorage.getItem("fletcher.order");
      return v === "importance" || v === "file" ? v : "freq";
    } catch {
      return "freq";
    }
  });
  const pickOrdering = (v: "freq" | "importance" | "file") => {
    setOrdering(v);
    try {
      localStorage.setItem("fletcher.order", v);
    } catch {
      /* per-viewer nicety only */
    }
  };
  const [uiMode, setUiMode] = useState<"standard" | "advanced">(() => {
    try {
      return localStorage.getItem("fletcher.mode") === "advanced" ? "advanced" : "standard";
    } catch {
      return "standard";
    }
  });
  const pickUiMode = (v: "standard" | "advanced") => {
    setUiMode(v);
    try {
      localStorage.setItem("fletcher.mode", v);
    } catch {
      /* per-viewer nicety only */
    }
  };

  // ---- Listening Lab state ----
  const [abx, setAbx] = useState<AbxState | null>(null);
  const abxRef = useRef<AbxState | null>(null);
  abxRef.current = abx;
  const [abxResult, setAbxResult] = useState<AbxResult | null>(null);
  const [sessions, setSessions] = useState<AbxResult[]>([]);
  const [trials, setTrials] = useState(16);
  const [expanded, setExpanded] = useState<string | null>(null);

  const loadSessions = () =>
    invoke<AbxResult[]>("abx_sessions").then(setSessions).catch(() => {});

  /** Classic mode (no chains): active preset vs flat. With chains: any two —
   *  node-vs-node from the history inspector, or a recorded session's re-run. */
  const startAbx = (n?: number, chains?: { a: ChainSnap; b: ChainSnap; aName: string; bName: string }) => {
    invoke<AbxState>("abx_start", {
      trials: n ?? trials,
      ...(chains ?? {}),
    })
      .then((s) => {
        setAbx(s);
        setAbxResult(null);
      })
      .catch((e) => showNotice(String(e)));
  };
  /** Re-run a recorded session: with its exact chains when recorded, else classic. */
  const rerunAbx = (r: AbxResult) =>
    startAbx(
      r.trials,
      r.aChain && r.bChain
        ? { a: r.aChain, b: r.bChain, aName: r.aName, bName: r.bName ?? "Flat" }
        : undefined,
    );

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
  // Trampoline (dev law, Q-20): registered only while a session runs, but the
  // body must stay current across hot reloads mid-session too.
  const abxKeyRef = useRef<(e: KeyboardEvent) => void>(() => {});
  abxKeyRef.current = (e: KeyboardEvent) => {
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
  useEffect(() => {
    if (!abx) return;
    const onKey = (e: KeyboardEvent) => abxKeyRef.current(e);
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

  // Trampoline (dev law, Q-20): fast-refresh never re-runs []-effects, so
  // registered callbacks are permanent — they dispatch through this ref,
  // whose body is refreshed every render.
  const pushRef = useRef({
    config: () => {},
    ab: (_s: string) => {},
    abx: (_t: string) => {},
    calEnded: (_e: string | null) => {},
    toolsProgress: (_p: { which: string; pct: number | null }) => {},
    trackState: (_p: Record<string, unknown>) => {},
    trackPos: (_p: { posS: number; paused: boolean }) => {},
    scopeClosed: (_label: string) => {},
    scopeKey: (_p: { key: string; state: string }) => {},
    scopeHello: () => {},
    roomCmd: (_p: { key: string; value: number | string }) => {},
  });
  pushRef.current = {
    // Push-based updates from the Rust config watcher — no polling. Ignored
    // mid-drag (our own writes fire it; the drag state is fresher).
    config: () => {
      if (!dragging.current) {
        refresh();
        invoke<AbInfo>("ab_info").then(setAb).catch(() => {});
      }
    },
    // Hotkey / tray flips land here.
    ab: (side: string) => setAb((cur) => ({ ...cur, side })),
    // During a session the hotkey cycles the audition target.
    abx: (target: string) => setAbx((cur) => (cur ? { ...cur, audition: target } : cur)),
    // The leveling noise stopped (finished fading, was preempted, or errored).
    calEnded: (err: string | null) => {
      setCalNoise(false);
      if (err) showNotice(err);
    },
    toolsProgress: (p) => setToolsProg(p),
    // A scope window closed → its pane comes back inline.
    scopeClosed: (label) => {
      if (label === "scope-spec") setPopped((p) => ({ ...p, spec: false }));
      if (label === "scope-fft") setPopped((p) => ({ ...p, fft: false }));
    },
    // I/O keys forwarded from focused scope windows (Q-20 OS shortcuts).
    scopeKey: (p) => {
      if (!trackSessRef.current || trackSessRef.current.phase || p.state !== "down") return;
      if (p.key === "i") setIn(trackPosRef.current.posS);
      else if (p.key === "o") setOut(trackPosRef.current.posS);
      else if (p.key === "escape") clearRegion();
    },
    // A satellite just opened and wants the shared state it can't derive.
    scopeHello: () => {
      emit("io-region", ioRegionRef.current ?? null);
      emit("spec-params", { win: specWin, floor: specFloor, linear: specLinear });
      emit("room-state", {
        tcDec,
        specOn,
        waveOn,
        fftOn,
        specWin,
        specFloor,
        specLinear,
        mode: studioMode,
        scrubTau,
        scrubMax,
      });
    },
    // A satellite's ⚙ menu made a change — main owns the state, applies it
    // through the same pick functions, and rebroadcasts.
    roomCmd: (p) => {
      const num = typeof p.value === "number" ? p.value : Number(p.value);
      if (p.key === "tcdec") pickTcDec(num);
      else if (p.key === "scope") toggleScope(p.value as "spec" | "wave" | "fft");
      else if (p.key === "specwin" || p.key === "specfloor" || p.key === "speclinear")
        pickSpecParam(p.key, num);
      else if (p.key === "mode") pickStudioMode(p.value === "eq" ? "eq" : "bypass");
      else if (p.key === "scrubtau") commitScrub("tau", num);
      else if (p.key === "scrubmax") commitScrub("max", num);
      else if (p.key === "viewpreset") applyViewPreset(p.value === "melody" ? "melody" : "standard");
    },
    trackState: (p) => {
      const id = p.trackId as number;
      const sess = (p.sess as number) ?? 0;
      if (p.event === "decoding") {
        setTrackSess({ id, sess, title: titleOf(id), durationS: 0, mode: "", phase: "decoding" });
      } else if (p.event === "started") {
        setTrackSess({
          id,
          sess,
          title: titleOf(id),
          durationS: p.durationS as number,
          mode: p.mode as string,
          exclusive: p.exclusive as boolean,
          gainDb: p.gainDb as number,
          device: p.device as string,
          rate: p.rate as number,
          bits: p.bits as number,
        });
        // A clicked clip carries its region into the fresh session.
        const pc = pendingClip.current;
        pendingClip.current = null;
        if (pc && pc.trackId === id) {
          setIoRegion({ a: pc.tIn, b: pc.tOut });
          setLoopOn(true);
          setActiveClipId(pc.id);
          invoke("track_seek", { seconds: pc.tIn }).catch(() => {});
        } else {
          clearRegion();
        }
        loadLibrary(); // duration got written
      } else if (p.event === "ended") {
        // A superseded session's death must not clobber its successor.
        const cur = trackSessRef.current;
        if (cur && cur.sess !== sess) {
          if (p.error) showNotice(String(p.error));
          return;
        }
        setTrackSess(null);
        setTrackPos({ posS: 0, paused: false });
        if (p.error) showNotice(String(p.error));
      }
    },
    trackPos: (p) => setTrackPos(p),
  };
  useEffect(() => {
    refresh();
    refreshPresets();
    loadSessions();
    loadSettings(); // the A/B bar's matched/unmatched state needs it
    loadAutostart();
    const unlisten = listen("apo-config-changed", () => pushRef.current.config());
    const unlistenAb = listen<string>("ab-changed", (e) => pushRef.current.ab(e.payload));
    const unlistenAbx = listen<string>("abx-audition", (e) => pushRef.current.abx(e.payload));
    const unlistenCal = listen<string | null>("cal-noise-ended", (e) => pushRef.current.calEnded(e.payload));
    const unlistenTools = listen<{ which: string; pct: number | null }>("tools-progress", (e) =>
      pushRef.current.toolsProgress(e.payload),
    );
    const unlistenTrackState = listen<Record<string, unknown>>("track-state", (e) =>
      pushRef.current.trackState(e.payload),
    );
    const unlistenTrackPos = listen<{ posS: number; paused: boolean }>("track-pos", (e) =>
      pushRef.current.trackPos(e.payload),
    );
    const unlistenScopeClosed = listen<string>("scope-closed", (e) =>
      pushRef.current.scopeClosed(e.payload),
    );
    const unlistenScopeKey = listen<{ key: string; state: string }>("scope-key", (e) =>
      pushRef.current.scopeKey(e.payload),
    );
    const unlistenScopeHello = listen("scope-hello", () => pushRef.current.scopeHello());
    const unlistenRoomCmd = listen<{ key: string; value: number | string }>("room-cmd", (e) =>
      pushRef.current.roomCmd(e.payload),
    );
    return () => {
      unlistenRoomCmd.then((f) => f());
      unlistenScopeHello.then((f) => f());
      unlistenScopeClosed.then((f) => f());
      unlistenScopeKey.then((f) => f());
      unlisten.then((f) => f());
      unlistenAb.then((f) => f());
      unlistenAbx.then((f) => f());
      unlistenCal.then((f) => f());
      unlistenTools.then((f) => f());
      unlistenTrackState.then((f) => f());
      unlistenTrackPos.then((f) => f());
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
  type HistNode = {
    id: number;
    parent: number | null;
    children: number[];
    snap: ChainSnap;
    label: string;
    ts: number;
    note?: string;
    pinned?: boolean;
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
    // A real move supersedes any inspector preview — never "restore" over it.
    previewRestore.current = null;
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
    selectOnly(null);
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

  // ---- the history inspector's actions (Q-24) ----

  /** Drag-graft: copy a node's exact sound in as ONE clean step under any
   *  other node — a chain of 100 messy edits becomes a single child of the
   *  ancestor you dropped it on. The original branch stays until pruned
   *  (history never lies); nodes are full snapshots, so the copy is exact
   *  wherever it lands. The graft becomes current, audibly. */
  const graftNode = (id: number, ontoId: number) => {
    const h = hist.current;
    const src = h?.nodes.get(id);
    const onto = h?.nodes.get(ontoId);
    // Dropping on its own parent is allowed: that's a plain node duplicate.
    if (!h || !src || !onto || id === ontoId) return;
    // steps from src up to onto, when onto is an ancestor — names the label
    let d = 0;
    let c: number | null = src.parent;
    for (let steps = 1; c != null; steps++) {
      if (c === ontoId) {
        d = steps;
        break;
      }
      c = h.nodes.get(c)?.parent ?? null;
    }
    const node: HistNode = {
      id: h.next++,
      parent: onto.id,
      children: [],
      snap: src.snap.map((f) => ({ ...f })),
      label: d ? `${d} edit${d === 1 ? "" : "s"} in one` : `from #${src.id}`,
      ts: Date.now(),
      note: src.note,
    };
    onto.children.push(node.id);
    h.nodes.set(node.id, node);
    h.current = node.id;
    applySnap(node.snap);
    buildRail(node.id);
    setHistVersion((v) => v + 1);
  };

  /** Rename / annotate / pin — labels become intentions, notes become memory. */
  const editNode = (id: number, patch: NodePatch) => {
    const h = hist.current;
    const n = h?.nodes.get(id);
    if (!n) return;
    if (patch.label !== undefined && patch.label) n.label = patch.label;
    if (patch.note !== undefined) n.note = patch.note || undefined;
    if (patch.pinned !== undefined) n.pinned = patch.pinned || undefined;
    setHistVersion((v) => v + 1);
  };

  /** Level-matched preview of an arbitrary snap; null restores what played
   *  before the first preview. Uses preview_chain — fletcher.txt only, no
   *  preset mutation, no A/B side reset. */
  const previewRestore = useRef<ChainSnap | null>(null);
  const previewSnap = (snap: ChainSnap | null) => {
    if (snap) {
      if (previewRestore.current == null) previewRestore.current = ownSnap();
      invoke("preview_chain", { filters: snap }).catch((e) => showNotice(String(e)));
    } else {
      const back = previewRestore.current;
      previewRestore.current = null;
      if (back) invoke("preview_chain", { filters: back }).catch((e) => showNotice(String(e)));
    }
  };

  // The hist-cmd listener runs in a [] effect; these refs keep it (and the
  // inspector callbacks) reading fresh state instead of stale closures.
  const trialsRef = useRef(trials);
  trialsRef.current = trials;
  const presetsRef = useRef(presets);
  presetsRef.current = presets;

  /** Blind-test two tree nodes — closes Q-17's last residual. The trial room
   *  lives in the main window, so surface it (the pop-out may have started this). */
  const abxNodes = (aId: number, bId: number) => {
    const h = hist.current;
    const a = h?.nodes.get(aId);
    const b = h?.nodes.get(bId);
    if (!a || !b) return;
    previewRestore.current = null; // the session owns fletcher.txt now
    setHistOpen(false);
    const name = (n: HistNode) => `node #${n.id} · ${n.label}`;
    startAbx(trialsRef.current, { a: a.snap, b: b.snap, aName: name(a), bName: name(b) });
    import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => {
        const w = getCurrentWindow();
        return w.show().then(() => w.setFocus());
      })
      .catch(() => {});
  };

  /** Promote a node's chain into a preset (auto-suffixed name, not activated). */
  const promoteNode = (id: number) => {
    const n = hist.current?.nodes.get(id);
    if (!n) return;
    // sanitize_name caps at 60 bytes — leave headroom for the auto-suffix.
    const base = `${presetsRef.current.active ?? "chain"} · ${n.label === "start" ? "root" : n.label}`.slice(0, 48);
    invoke<string>("preset_create_from_chain", { name: base, filters: n.snap })
      .then((saved) => {
        refreshPresets();
        showNotice(`saved as preset “${saved}” — select it in the preset menu to activate`);
      })
      .catch((e) => showNotice(String(e)));
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
        note: n.note,
        pinned: n.pinned,
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
          note: typeof n.note === "string" && n.note ? n.note : undefined,
          pinned: n.pinned === true ? true : undefined,
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

  // Serializable tree for the shared canvas + pop-out sync. Snapshots ride
  // along since Q-24: the inspector needs them in both windows (they're small,
  // and curves are recomputed per window through chain_curves).
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
        snap: n.snap,
        note: n.note,
        pinned: n.pinned,
      })),
      current: h.current,
      name: presets.active ?? "chain",
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [histVersion, presets.active]);
  const treeDataRef = useRef<HistTreeData | null>(null);
  treeDataRef.current = treeData;

  // Live-sync the pop-out window; answer its hello; act on its commands.
  useEffect(() => {
    if (treeData) emit("hist-sync", treeData);
  }, [treeData]);
  // The command bus dispatches through a ref refreshed every render. Vite
  // fast-refresh does NOT re-run []-effects, so a listener that captures app
  // closures directly keeps executing STALE code after hot reloads — the
  // pop-out path acting on old behavior while the main path uses new. The
  // registered callbacks below are permanent trampolines; only this ref's
  // body ever swaps, so both paths are always the same code.
  type HistCmd = {
    type: string;
    id: number;
    base?: number;
    patch?: NodePatch;
    snap?: ChainSnap | null;
    spec?: { sel: number | null; base: number | null; cmp: number[] };
  };
  const busRef = useRef<{ hello: () => void; cmd: (p: HistCmd) => void; diffHello: () => void }>({
    hello: () => {},
    cmd: () => {},
    diffHello: () => {},
  });
  busRef.current = {
    hello: () => {
      if (treeDataRef.current) emit("hist-sync", treeDataRef.current);
    },
    cmd: (p) => {
      if (p.type === "jump") jumpTo(p.id);
      else if (p.type === "del") deleteNode(p.id);
      else if (p.type === "undo") undo();
      else if (p.type === "redo") redo();
      else if (p.type === "edit" && p.patch) editNode(p.id, p.patch);
      else if (p.type === "preview" && p.snap) previewSnap(p.snap);
      else if (p.type === "restore") previewSnap(null);
      else if (p.type === "abx" && p.base != null) abxNodes(p.id, p.base);
      else if (p.type === "promote") promoteNode(p.id);
      else if (p.type === "compare" && p.spec) onCompareSpec(p.spec);
      else if (p.type === "popdiff") popOutDiff();
      else if (p.type === "graft" && p.base != null) graftNode(p.id, p.base);
      else if (p.type === "import") importHistory();
      else if (p.type === "export") exportHistory();
    },
    diffHello: () => emitDiffSync(),
  };
  useEffect(() => {
    const u1 = listen("hist-hello", () => busRef.current.hello());
    const u2 = listen<HistCmd>("hist-cmd", (e) => busRef.current.cmd(e.payload));
    const u3 = listen("diff-hello", () => busRef.current.diffHello());
    return () => {
      u1.then((f) => f());
      u2.then((f) => f());
      u3.then((f) => f());
    };
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

  // ---- the difference pop-out: mirrors the last-touched inspector's compare,
  // re-resolved against the live tree so it keeps tracking while you edit ----
  const diffSpecRef = useRef<{ sel: number | null; base: number | null; cmp: number[] }>({
    sel: null,
    base: null,
    cmp: [],
  });
  const emitDiffSync = () => {
    const h = hist.current;
    if (!h) return;
    const spec = diffSpecRef.current;
    const selId = spec.sel != null && h.nodes.has(spec.sel) ? spec.sel : h.current;
    const selN = h.nodes.get(selId)!;
    const baseId =
      spec.base != null && h.nodes.has(spec.base) && spec.base !== selId ? spec.base : selN.parent;
    const base = baseId != null ? h.nodes.get(baseId) ?? null : null;
    const ids = [selId, ...spec.cmp.filter((id) => id !== selId && h.nodes.has(id))];
    const payload: DiffSync = {
      base: base ? { id: base.id, label: base.label, snap: base.snap } : null,
      series: ids.map((id, k) => {
        const n = h.nodes.get(id)!;
        return { id: n.id, label: n.label, color: CMP_COLORS[k % CMP_COLORS.length], snap: n.snap };
      }),
    };
    emit("diff-sync", payload);
  };
  const onCompareSpec = (spec: { sel: number | null; base: number | null; cmp: number[] }) => {
    diffSpecRef.current = spec;
    emitDiffSync();
  };
  // Any tree change (gesture, undo, prune, preset switch) re-resolves the compare.
  useEffect(() => {
    emitDiffSync();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [histVersion]);

  const popOutDiff = async () => {
    try {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const existing = await WebviewWindow.getByLabel("diff");
      if (existing) {
        await existing.show().catch(() => {});
        await existing.setFocus();
      } else {
        new WebviewWindow("diff", {
          url: "index.html?view=diff",
          title: "Fletcher — Difference",
          width: 880,
          height: 560,
        });
      }
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
  const filterClipboard = useRef<{ enabled: boolean; kind: string; fcHz: number; gainDb: number; q: number }[]>([]);

  const copySelectedFilter = () => {
    const cur = stateRef.current;
    if (!cur) return;
    const ord = orderedRef.current;
    const members = ord.filter((o) => multiSelRef.current.has(o.i)).map((o) => cur.filters[o.i]);
    if (members.length === 0) return;
    filterClipboard.current = members.map((f) => ({
      enabled: f.enabled,
      kind: f.kind,
      fcHz: f.fcHz,
      gainDb: f.gainDb,
      q: f.q,
    }));
    const text = members.map(apoLine).join("\r\n");
    navigator.clipboard?.writeText(text).catch(() => {});
    showNotice(
      members.length === 1
        ? `copied ${members[0].kind} ${fmtHz(members[0].fcHz)} Hz — Ctrl+V pastes it into any preset`
        : `copied ${members.length} filters — Ctrl+V pastes them into any preset`,
    );
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
    if (pasted.length === 0 && filterClipboard.current.length) pasted = filterClipboard.current;
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
    setMultiSel(new Set(fresh.map((_, k) => cur.filters.length + k)));
    anchorSelRef.current = filters.length - 1;
    pushChain(filters, true);
    window.setTimeout(() => commitGesture(pasted.length === 1 ? "paste filter" : `paste ${pasted.length} filters`), 0);
    showNotice(`pasted ${pasted.length} filter${pasted.length > 1 ? "s" : ""}`);
  };

  // Trampoline (same reason as the command bus): the registered handler is
  // permanent; the ref body carries the always-current behavior.
  const keyRef = useRef<(e: KeyboardEvent) => void>(() => {});
  keyRef.current = (e: KeyboardEvent) => {
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
    } else if (e.key === "Escape") {
      selectOnly(null);
      setTypeMenu(null);
      if (viewModeRef.current === "clips") clearRegion();
    } else if (k === "x" && e.altKey && ioRegionRef.current) {
      // Resolve: Alt+X clears the in/out region.
      e.preventDefault();
      clearRegion();
    } else if (e.key === " " && trackSessRef.current) {
      // Resolve: space = play/pause, wherever you are while a track session runs.
      e.preventDefault();
      invoke("track_toggle").catch(() => {});
    } else if (k === "c" && timelineHover.current != null) {
      // Resolve: C = playhead to cursor; HELD C + drag = audible scrub.
      e.preventDefault();
      if (!e.repeat) scrubStart();
    } else if (k === "i" && trackSessRef.current && !trackSessRef.current.phase) {
      e.preventDefault();
      setIn(trackPosRef.current.posS);
    } else if (k === "o" && trackSessRef.current && !trackSessRef.current.phase) {
      e.preventDefault();
      setOut(trackPosRef.current.posS);
    }
  };
  const keyUpRef = useRef<(e: KeyboardEvent) => void>(() => {});
  keyUpRef.current = (e: KeyboardEvent) => {
    if (e.key.toLowerCase() === "c") scrubEnd();
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => keyRef.current(e);
    const onKeyUp = (e: KeyboardEvent) => keyUpRef.current(e);
    window.addEventListener("keydown", onKey);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, []);

  const mutateFilter = (i: number, patch: Partial<EqFilter>, immediate = false) => {
    const cur = stateRef.current;
    if (!cur) return;
    const filters = cur.filters.map((f, j) => (j === i ? { ...f, ...patch } : f));
    setState({ ...cur, filters });
    pushChain(filters, immediate);
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
    selectOnly(filters.length - 1);
    pushChain(filters, true);
    window.setTimeout(() => commitGesture("add filter"), 0);
  };

  const ordered = useMemo(() => {
    if (!state) return [];
    const rows = state.filters.map((f, i) => ({ f, i }));
    if (ordering === "file") return rows; // as parsed — never reordered on disk
    if (ordering === "importance") return rows.sort((a, b) => Math.abs(b.f.gainDb) - Math.abs(a.f.gainDb));
    return rows.sort((a, b) => a.f.fcHz - b.f.fcHz);
  }, [state, ordering]);

  const orderedRef = useRef(ordered);
  orderedRef.current = ordered;

  /** Click selection: plain = only this; Ctrl = toggle; Shift = range in strip order. */
  const handleSelect = (e: { ctrlKey: boolean; shiftKey: boolean }, i: number) => {
    if (e.ctrlKey) {
      setMultiSel((prev) => {
        const n = new Set(prev);
        if (n.has(i)) n.delete(i);
        else n.add(i);
        setSelected(n.has(i) ? i : n.size ? [...n][0] : null);
        return n;
      });
      anchorSelRef.current = i;
    } else if (e.shiftKey && anchorSelRef.current != null) {
      const ord = orderedRef.current;
      const di = ord.findIndex((o) => o.i === i);
      const da = ord.findIndex((o) => o.i === anchorSelRef.current);
      if (di >= 0 && da >= 0) {
        const [lo, hi] = di < da ? [di, da] : [da, di];
        setMultiSel(new Set(ord.slice(lo, hi + 1).map((o) => o.i)));
        setSelected(i);
      }
    } else {
      selectOnly(i);
    }
  };

  const sel = multiSel.size === 1 && state ? state.filters[[...multiSel][0]] : null;


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
  const dragGrab = useRef<number | null>(null);
  const dragOrig = useRef<Map<number, { fc: number; gain: number }>>(new Map());

  const mutateMany = (patches: Map<number, Partial<EqFilter>>, immediate = false) => {
    const cur = stateRef.current;
    if (!cur) return;
    const filters = cur.filters.map((f, j) => (patches.has(j) ? { ...f, ...patches.get(j)! } : f));
    setState({ ...cur, filters });
    pushChain(filters, immediate);
  };

  const startDrag = (i: number) => (e: React.PointerEvent) => {
    const f = state?.filters[i];
    if (f?.sourceFile !== OWN_FILE || e.ctrlKey || e.shiftKey) {
      handleSelect(e, i); // selection gesture, not a drag
      return;
    }
    let selNow: ReadonlySet<number> = multiSelRef.current;
    if (!selNow.has(i)) {
      selectOnly(i);
      selNow = new Set([i]);
    }
    const cur = stateRef.current!;
    dragOrig.current = new Map(
      [...selNow]
        .filter((j) => cur.filters[j]?.sourceFile === OWN_FILE)
        .map((j) => [j, { fc: cur.filters[j].fcHz, gain: cur.filters[j].gainDb }]),
    );
    dragging.current = true;
    dragGrab.current = i;
    const p = svgPoint(e);
    grabOffset.current = { dx: xOf(f.fcHz) - p.x, dy: yOf(f.gainDb) - p.y };
    (e.target as Element).setPointerCapture(e.pointerId);
  };

  const onDragMove = (i: number) => (e: React.PointerEvent) => {
    if (!dragging.current || dragGrab.current !== i) return;
    const o = dragOrig.current.get(i);
    if (!o) return;
    const p = svgPoint(e);
    const x = Math.max(0, Math.min(GW, p.x + grabOffset.current.dx));
    const f = fOf(x);
    const db = dbOfR(p.y + grabOffset.current.dy, dbRange);
    const clamped = Math.max(-dbRange, Math.min(dbRange, db));
    // The grabbed filter follows the cursor; the rest of the selection moves
    // with it — same dB delta, same log-frequency ratio.
    const dGain = clamped - o.gain;
    const rFc = f / o.fc;
    const patches = new Map<number, Partial<EqFilter>>();
    dragOrig.current.forEach((oj, j) => {
      const nf = Math.max(10, Math.min(24000, oj.fc * rFc));
      patches.set(j, {
        fcHz: +nf.toFixed(nf < 100 ? 1 : 0),
        gainDb: +Math.max(-30, Math.min(30, oj.gain + dGain)).toFixed(1),
      });
    });
    mutateMany(patches);
  };

  const endDrag = (_i: number) => () => {
    if (!dragging.current) return;
    dragging.current = false;
    const n = dragOrig.current.size;
    const cur = stateRef.current;
    if (cur) pushChain(cur.filters, true);
    window.setTimeout(() => commitGesture(n > 1 ? `move x${n}` : "move"), 0);
  };

  const onWheelQ = (i: number) => (e: React.WheelEvent) => {
    const cur = stateRef.current;
    const f = cur?.filters[i];
    if (!cur || !f || f.sourceFile !== OWN_FILE) return;
    const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
    const targets = multiSelRef.current.has(i) ? [...multiSelRef.current] : [i];
    const patches = new Map<number, Partial<EqFilter>>();
    targets.forEach((j) => {
      const fj = cur.filters[j];
      if (fj?.sourceFile === OWN_FILE) {
        patches.set(j, { q: +Math.max(0.05, Math.min(50, fj.q * factor)).toFixed(2) });
      }
    });
    mutateMany(patches, true);
    settleWheelGesture(); // the burst becomes one history node
  };

  /** A selected cell's x deletes the whole (editable) selection; else just one. */
  const deleteSelected = (i: number) => {
    const cur = stateRef.current;
    if (!cur) return;
    const targets = multiSelRef.current.has(i) ? multiSelRef.current : new Set([i]);
    const filters = cur.filters.filter((f, j) => !(targets.has(j) && f.sourceFile === OWN_FILE));
    const n = cur.filters.length - filters.length;
    if (n === 0) return;
    setState({ ...cur, filters });
    selectOnly(null);
    pushChain(filters, true);
    window.setTimeout(() => commitGesture(n > 1 ? `delete x${n}` : "delete"), 0);
  };

  return (
    <div className="frame">
      <header>
        <span className="wordmark">FLETCHER</span>
        <nav>
          <span className={`tab ${view === "eq" ? "active" : ""}`} onClick={() => setView("eq")}>EQ</span>
          <span className={`tab ${view === "lab" ? "active" : ""}`} onClick={() => setView("lab")}>LISTENING LAB</span>
          <span
            className={`tab ${view === "clips" ? "active" : ""}`}
            onClick={() => {
              setView("clips");
              loadLibrary();
              loadTools();
              loadStudioMode();
            }}
          >
            CLIP STUDIO
          </span>
          <span
            className={`tab ${uiMode === "advanced" ? "disabled" : "advanced"}`}
            title={
              uiMode === "advanced"
                ? "Fingerprint Lab — measure and match headphones. Coming in Phase 4."
                : "Measure and match headphones. Advanced: needs a measurement microphone — switch to Advanced mode in Settings."
            }
          >
            FINGERPRINTS
          </span>
          <span
            className={`tab ${view === "settings" ? "active" : ""}`}
            onClick={() => {
              setView("settings");
              loadSettings();
              loadAutostart();
              loadTools();
            }}
          >
            SETTINGS
          </span>
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
              <span>{`${abx.aName} vs ${abx.bName ?? "Flat"}`}</span>
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
              {abx.levelMatched === false ? (
                <span className="warn-text">▲ levels NOT matched — this result measures loudness too</span>
              ) : (
                <span className="ok-text">● levels matched</span>
              )}
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
                <button onClick={() => { const r = abxResult; setAbxResult(null); rerunAbx(r); }}>
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
                      <div className="session-title">{`${r.aName} vs ${r.bName ?? "Flat"}`}</div>
                      <div className={`session-verdict ${v.good ? "good" : "meh"}`}>{v.text}</div>
                      {r.levelMatched === false && (
                        <div className="dim-sm warn-text">levels were NOT matched — loudness was audible</div>
                      )}
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
                        <button
                          onClick={() =>
                            r.aChain
                              ? invoke("preview_chain", { filters: r.aChain }).catch((e) => showNotice(String(e)))
                              : setSide("a")
                          }
                        >
                          {`A · ${r.aName}`}
                        </button>
                        <button
                          onClick={() =>
                            r.bChain
                              ? invoke("preview_chain", { filters: r.bChain }).catch((e) => showNotice(String(e)))
                              : setSide("b")
                          }
                        >
                          {`B · ${r.bName ?? "Flat"}`}
                        </button>
                        <span className="spacer" />
                        <button onClick={() => rerunAbx(r)}>{`Run again · ${r.trials} trials`}</button>
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {view === "clips" && (
        <div className="clips">
          <div className="clips-rail">
            <div className="rail-head">
              <span className="mono lab-label rail-pad">LIBRARY</span>
              <span className="spacer" />
              <span
                className="row-act"
                title="open Fletcher's data folder — downloaded media, caches, the clip library"
                onClick={() => invoke("open_data_dir").catch(() => {})}
              >
                📁
              </span>
            </div>
            {tools && !tools.ffmpeg && (
              <div className="tool-banner">
                <p>
                  The track engine decodes with <b>ffmpeg</b> — fetched on demand into Fletcher's own
                  tools folder (~180 MB, BtbN's official build), never installed system-wide.
                </p>
                {toolsProg?.which === "ffmpeg" ? (
                  <span className="mono dim-sm">{`downloading… ${toolsProg.pct ?? 0}%`}</span>
                ) : (
                  <button onClick={() => installTool("ffmpeg")}>Install ffmpeg</button>
                )}
              </div>
            )}
            <div className="clips-tracks">
              {library?.tracks.length === 0 && (
                <p className="dim-sm rail-pad">No tracks yet — import one below.</p>
              )}
              {[
                { label: "IMPORTED", rows: (library?.tracks ?? []).filter((t) => t.kind !== "signal") },
                { label: "CREATED", rows: (library?.tracks ?? []).filter((t) => t.kind === "signal") },
              ]
                .filter((g) => g.rows.length > 0)
                .map((g, _gi, groups) => (
                  <div key={g.label}>
                    {groups.length > 1 && <div className="mono lab-label rail-sec">{g.label}</div>}
                    {g.rows.map((t) => {
                const playing = trackSess?.id === t.id && !trackSess?.phase;
                const clipCount = (library?.clips ?? []).filter((c) => c.trackId === t.id).length;
                return (
                  <div key={t.id}>
                    <div
                      className={`clips-track ${playing ? "playing" : ""}`}
                      onClick={() =>
                        playing ? invoke("track_toggle").catch(() => {}) : playTrack(t)
                      }
                      title={playing ? "click = play/pause" : "play through the track engine"}
                    >
                      <div className="ct-main">
                        <div className="ct-line">
                          {t.kind === "signal" && <span className="mono sig-glyph">∿</span>}
                          <span className="ct-title">{t.title}</span>
                          {t.artist && <span className="dim-sm">{t.artist}</span>}
                          {playing && (
                            <span className="mono play-ind">{trackPos.paused ? "❚❚" : "▶"}</span>
                          )}
                        </div>
                        <span className="dim-sm">
                          {`${t.kind === "signal" ? `${sigSummary(t)} · ` : ""}${clipCount ? `${clipCount} clip${clipCount === 1 ? "" : "s"}` : "no clips yet"}${
                            t.kind !== "signal" && t.durationS ? ` · ${fmtTime(t.durationS)}` : ""
                          }`}
                        </span>
                      </div>
                      {t.kind === "signal" && (
                        <span
                          className="row-act"
                          title="edit this signal's recipe"
                          onClick={(e) => {
                            e.stopPropagation();
                            openGenEdit(t);
                          }}
                        >
                          ✎
                        </span>
                      )}
                      <span
                        className="row-act"
                        title="remove from library, clips included (the file itself is untouched)"
                        onClick={(e) => {
                          e.stopPropagation();
                          invoke<LibraryState>("track_delete", { id: t.id })
                            .then(setLibrary)
                            .catch((e2) => showNotice(String(e2)));
                        }}
                      >
                        ×
                      </span>
                    </div>
                    {(library?.clips ?? [])
                      .filter((c) => c.trackId === t.id)
                      .map((c) => (
                        <div
                          key={c.id}
                          className={`clips-clip ${c.id === activeClipId ? "active" : ""}`}
                          onClick={() => openClip(c)}
                          onDoubleClick={() => focusClip(c)}
                          title="solo this clip on loop · double-click zooms to it"
                        >
                          <span className="clip-dot" style={{ background: clipDotColor(c.tags) }} />
                          <span className="clip-name">{c.name}</span>
                          <span className="mono dim-sm">{`${fmtTcN(c.tIn, tcDec)}–${fmtTcN(c.tOut, tcDec)}`}</span>
                          <span className="spacer" />
                          {c.tags.length > 0 && (
                            <span className="mono clip-tag">{c.tags[0].toUpperCase()}</span>
                          )}
                          <span
                            className="row-act"
                            title="delete clip"
                            onClick={(e) => {
                              e.stopPropagation();
                              invoke<LibraryState>("clip_delete", { id: c.id })
                                .then(setLibrary)
                                .catch((e2) => showNotice(String(e2)));
                            }}
                          >
                            ×
                          </span>
                        </div>
                      ))}
                  </div>
                );
                    })}
                  </div>
                ))}
            </div>
            <div className="clips-rail-foot">
              <button onClick={importTrack}>+ Import track</button>
              <button onClick={openUrl} title="paste any link — yt-dlp extracts the audio into the library">
                + From link
              </button>
              <button onClick={() => setGenOpen(true)} title="build a signal from primitives — it lands in the library like any track">
                ∿ Generate
              </button>
            </div>
            {genOpen && (
              <>
                <div className="hist-backdrop" onClick={closeGen} />
                <div className="hist-panel gen-panel">
                  <div className="hist-head">
                    <span className="mono hist-title">
                      {genEditId != null ? "EDIT SIGNAL" : "SIGNAL GENERATOR"}
                    </span>
                    <span className="spacer" />
                    <span
                      className={`scale-opt mono ${genText != null ? "on" : ""}`}
                      title="the recipe itself, as JSON — everything the engine can do, validated on apply (this format becomes the API later)"
                      onClick={() => {
                        setGenErr(null);
                        setGenText(genText != null ? null : JSON.stringify(genSpec, null, 2));
                      }}
                    >
                      {"{ }"}
                    </span>
                    <span className="row-act" onClick={closeGen}>
                      ×
                    </span>
                  </div>
                  {genText != null ? (
                    <div className="gen-body">
                      <textarea
                        className="gen-json mono"
                        value={genText}
                        spellCheck={false}
                        onChange={(e) => setGenText(e.target.value)}
                      />
                      {genErr && <p className="gen-err mono">{genErr}</p>}
                      <p className="dim-sm gen-note">
                        The recipe, raw. kinds: white · pink · sine · sweepLog · sweepLinear · band ·
                        mix (with "layers"). Optional per layer: amHz+amDepth (tremolo), fmHz+fmDevHz
                        (vibrato). Apply validates before anything plays.
                      </p>
                      <div className="gen-actions">
                        <button onClick={() => setGenText(JSON.stringify(genSpec, null, 2))}>Revert</button>
                        <span className="spacer" />
                        <button className="primary" onClick={applyGenText}>
                          Apply recipe
                        </button>
                      </div>
                    </div>
                  ) : (
                  <div className="gen-body">
                    <div className="room-row">
                      <span className="room-key">Type</span>
                      <div className="seg seg-sm">
                        {(
                          [
                            ["pink", "Pink"],
                            ["white", "White"],
                            ["sine", "Sine"],
                            ["sweepLog", "Sweep log"],
                            ["sweepLinear", "Sweep lin"],
                            ["band", "Band"],
                            ["mix", "Mix"],
                          ] as const
                        ).map(([k, label]) => (
                          <span
                            key={k}
                            className={`seg-opt ${genSpec.kind === k ? "on" : ""}`}
                            onClick={() => pickGenKind(k)}
                          >
                            {label}
                          </span>
                        ))}
                      </div>
                    </div>
                    {isMix && (
                      <div className="room-row gen-layers-row">
                        <span className="room-key" title="the layers are summed; the −12 dBFS cap binds the sum, so hot layers can't stack past it">
                          Layers
                        </span>
                        <div className="gen-layers">
                          {(genSpec.layers ?? []).map((l, i) => (
                            <div
                              key={i}
                              className={`gen-layer ${i === selLayer ? "on" : ""}`}
                              onClick={() => setSelLayer(i)}
                            >
                              <span className="mono sig-glyph">∿</span>
                              <span>{`${layerName(l)} · ${l.levelDb} dB`}</span>
                              <span className="spacer" />
                              {(genSpec.layers?.length ?? 0) > 1 && (
                                <span
                                  className="row-act"
                                  title="remove layer"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    removeLayer(i);
                                  }}
                                >
                                  ×
                                </span>
                              )}
                            </div>
                          ))}
                          <button className="gen-add-layer" onClick={addLayer}>
                            + Add layer
                          </button>
                          {(() => {
                            const sum = (genSpec.layers ?? []).reduce(
                              (a, l) => a + 10 ** (Math.min(l.levelDb, -12) / 20),
                              0,
                            );
                            return sum > 0.25 ? (
                              <span className="dim-sm">
                                {`auto-trim ${(20 * Math.log10(0.25 / sum)).toFixed(1)} dB — every layer scaled together so the sum stays under the −12 dBFS cap`}
                              </span>
                            ) : null;
                          })()}
                        </div>
                      </div>
                    )}
                    {isMix && (
                      <div className="room-row">
                        <span className="room-key">{`Layer ${selLayer + 1} type`}</span>
                        <div className="seg seg-sm">
                          {(
                            [
                              ["pink", "Pink"],
                              ["white", "White"],
                              ["sine", "Sine"],
                              ["sweepLog", "Swp log"],
                              ["sweepLinear", "Swp lin"],
                              ["band", "Band"],
                            ] as const
                          ).map(([k, label]) => (
                            <span
                              key={k}
                              className={`seg-opt ${curSpec.kind === k ? "on" : ""}`}
                              onClick={() => updateCur({ kind: k, ...kindDefaults(k, curSpec) })}
                            >
                              {label}
                            </span>
                          ))}
                        </div>
                      </div>
                    )}
                    {curSpec.kind === "sine" && (
                      <div className="room-row">
                        <span className="room-key">Frequency</span>
                        <GenNum value={curSpec.hz} min={10} max={24000} unit="Hz" onCommit={(v) => updateCur({ hz: v })} />
                      </div>
                    )}
                    {(curSpec.kind === "sweepLog" || curSpec.kind === "sweepLinear") && (
                      <>
                        <div className="room-row">
                          <span className="room-key">Range</span>
                          <div className="trials-ctl">
                            <GenNum value={curSpec.fromHz} min={10} max={24000} unit="→" onCommit={(v) => updateCur({ fromHz: v })} />
                            <GenNum value={curSpec.toHz} min={10} max={24000} unit="Hz" onCommit={(v) => updateCur({ toHz: v })} />
                          </div>
                        </div>
                        <div className="room-row">
                          <span
                            className="room-key"
                            title="One pass: the sweep takes the whole duration to cross the range once. Repeat: it crosses the range every N seconds, starting over until the duration runs out."
                          >
                            Pacing
                          </span>
                          <div className="trials-ctl">
                            <div className="seg seg-sm">
                              <span
                                className={`seg-opt ${curSpec.sweepS == null ? "on" : ""}`}
                                onClick={() => updateCur({ sweepS: undefined })}
                              >
                                One pass
                              </span>
                              <span
                                className={`seg-opt ${curSpec.sweepS != null ? "on" : ""}`}
                                onClick={() => updateCur({ sweepS: Math.min(genSpec.seconds, 4) })}
                              >
                                Repeat
                              </span>
                            </div>
                            {curSpec.sweepS != null && (
                              <GenNum
                                value={curSpec.sweepS}
                                min={0.1}
                                max={600}
                                unit="s each"
                                onCommit={(v) => updateCur({ sweepS: v })}
                              />
                            )}
                          </div>
                        </div>
                      </>
                    )}
                    {curSpec.kind === "band" && (
                      <div className="room-row">
                        <span className="room-key">Band</span>
                        <div className="trials-ctl">
                          <GenNum value={curSpec.loHz} min={10} max={24000} unit="→" onCommit={(v) => updateCur({ loHz: v })} />
                          <GenNum value={curSpec.hiHz} min={10} max={24000} unit="Hz" onCommit={(v) => updateCur({ hiHz: v })} />
                        </div>
                      </div>
                    )}
                    <div className="room-row">
                      <span className="room-key">{isMix ? `Layer ${selLayer + 1} level` : "Level"}</span>
                      <div className="seg seg-sm">
                        {[-30, -24, -18, -12].map((db) => (
                          <span
                            key={db}
                            className={`seg-opt ${curSpec.levelDb === db ? "on" : ""}`}
                            onClick={() => updateCur({ levelDb: db })}
                          >
                            {`${db} dB`}
                          </span>
                        ))}
                      </div>
                    </div>
                    <div className="room-row">
                      <span className="room-key" title="amplitude modulation: the level pulses between full and (1 − depth) at the given rate — depth 100% gates to silence each cycle">
                        Tremolo
                      </span>
                      <div className="trials-ctl">
                        <div className="seg seg-sm">
                          <span
                            className={`seg-opt ${curSpec.amHz == null ? "on" : ""}`}
                            onClick={() => updateCur({ amHz: undefined, amDepth: undefined })}
                          >
                            Off
                          </span>
                          <span
                            className={`seg-opt ${curSpec.amHz != null ? "on" : ""}`}
                            onClick={() => updateCur({ amHz: 4, amDepth: curSpec.amDepth ?? 0.8 })}
                          >
                            On
                          </span>
                        </div>
                        {curSpec.amHz != null && (
                          <>
                            <GenNum value={curSpec.amHz} min={0.1} max={100} unit="Hz" onCommit={(v) => updateCur({ amHz: v })} />
                            <GenNum
                              value={Math.round((curSpec.amDepth ?? 0.8) * 100)}
                              min={1}
                              max={100}
                              unit="% depth"
                              onCommit={(v) => updateCur({ amDepth: v / 100 })}
                            />
                          </>
                        )}
                      </div>
                    </div>
                    {(curSpec.kind === "sine" || curSpec.kind === "sweepLog" || curSpec.kind === "sweepLinear") && (
                      <div className="room-row">
                        <span className="room-key" title="frequency modulation: the pitch swings ± the deviation at the given rate">
                          Vibrato
                        </span>
                        <div className="trials-ctl">
                          <div className="seg seg-sm">
                            <span
                              className={`seg-opt ${curSpec.fmHz == null ? "on" : ""}`}
                              onClick={() => updateCur({ fmHz: undefined, fmDevHz: undefined })}
                            >
                              Off
                            </span>
                            <span
                              className={`seg-opt ${curSpec.fmHz != null ? "on" : ""}`}
                              onClick={() => updateCur({ fmHz: 5, fmDevHz: curSpec.fmDevHz ?? 25 })}
                            >
                              On
                            </span>
                          </div>
                          {curSpec.fmHz != null && (
                            <>
                              <GenNum value={curSpec.fmHz} min={0.1} max={100} unit="Hz" onCommit={(v) => updateCur({ fmHz: v })} />
                              <GenNum value={curSpec.fmDevHz} min={1} max={5000} unit="±Hz" onCommit={(v) => updateCur({ fmDevHz: v })} />
                            </>
                          )}
                        </div>
                      </div>
                    )}
                    <div className="room-row">
                      <span className="room-key">Duration</span>
                      <GenNum value={genSpec.seconds} min={1} max={600} unit="s" onCommit={(v) => updateGen({ seconds: v })} />
                    </div>
                    <p className="dim-sm gen-note">
                      The same recipe always renders the exact same audio — that's its provenance.
                      {isMix
                        ? " A hot mix auto-trims all layers together so the sum stays under −12 dBFS — no clipping, ratios kept."
                        : " Peak is hard-capped at −12 dBFS."}{" "}
                      Preview loops through the shared path, so your volume knob stays live; edits
                      retune it as it plays.
                    </p>
                    <div className="gen-actions">
                      <button onClick={() => (genPreview ? genPreviewSet(null) : genPreviewSet(genSpec))}>
                        {genPreview ? "■ Stop preview" : "▶ Preview"}
                      </button>
                      <span className="spacer" />
                      <button className="primary" onClick={addSignal}>
                        {genEditId != null ? "Save changes" : "Add to library"}
                      </button>
                    </div>
                  </div>
                  )}
                </div>
              </>
            )}
            {urlOpen && (
              <>
                <div className="hist-backdrop" onClick={() => urlBusy == null && setUrlOpen(false)} />
                <div className="hist-panel gen-panel">
                  <div className="hist-head">
                    <span className="mono hist-title">IMPORT FROM LINK</span>
                    <span className="spacer" />
                    <span className="row-act" onClick={() => setUrlOpen(false)}>
                      ×
                    </span>
                  </div>
                  <div className="gen-body">
                    {tools?.ytdlp && tools?.ffmpeg ? (
                      <>
                        <input
                          className="url-input mono"
                          placeholder="paste a link — YouTube, SoundCloud, most anywhere"
                          value={urlText}
                          autoFocus
                          onChange={(e) => setUrlText(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") importUrl();
                          }}
                        />
                        <p className="dim-sm gen-note">
                          yt-dlp extracts the audio (m4a) into Fletcher's media folder and the
                          source link stays on the track. One item only — never a whole playlist.
                        </p>
                        <div className="gen-actions">
                          {urlBusy != null && (
                            <span className="mono dim-sm">
                              {urlBusy < 0 ? "extracting audio…" : `downloading… ${Math.round(urlBusy)}%`}
                            </span>
                          )}
                          <span className="spacer" />
                          <button
                            className="primary"
                            disabled={urlBusy != null || !urlText.trim()}
                            onClick={importUrl}
                          >
                            {urlBusy != null ? "working…" : "Add to library"}
                          </button>
                        </div>
                      </>
                    ) : (
                      <>
                        <p className="dim-sm gen-note">
                          {`Link import needs ${!tools?.ytdlp ? "yt-dlp" : ""}${
                            !tools?.ytdlp && !tools?.ffmpeg ? " and " : ""
                          }${!tools?.ffmpeg ? "ffmpeg" : ""} — fetched on demand into Fletcher's tools folder, never touching your system.`}
                        </p>
                        <div className="gen-actions">
                          {!tools?.ytdlp && (
                            <button onClick={() => installTool("yt-dlp")} disabled={toolsProg != null}>
                              {toolsProg?.which === "yt-dlp"
                                ? `yt-dlp… ${toolsProg.pct ?? 0}%`
                                : "Install yt-dlp"}
                            </button>
                          )}
                          {!tools?.ffmpeg && (
                            <button onClick={() => installTool("ffmpeg")} disabled={toolsProg != null}>
                              {toolsProg?.which === "ffmpeg"
                                ? `ffmpeg… ${toolsProg.pct ?? 0}%`
                                : "Install ffmpeg"}
                            </button>
                          )}
                        </div>
                      </>
                    )}
                  </div>
                </div>
              </>
            )}
          </div>
          <div className="clips-main">
            <div className="clips-toolbar">
              {trackSess ? (
                <>
                  <span className="ct-title">{trackSess.title}</span>
                  {trackSess.phase ? (
                    <span className="mono dim-sm">decoding…</span>
                  ) : (
                    <span className="mono dim-sm">
                      {`${trackSess.mode === "bypass" ? (trackSess.exclusive ? "bypass · exclusive" : "bypass unavailable · shared") : "through your EQ · shared"}${
                        trackSess.device ? ` · ${trackSess.device}` : ""
                      }${trackSess.rate ? ` · ${trackSess.bits} bit @ ${trackSess.rate} Hz` : ""}${
                        trackSess.mode === "bypass" && trackSess.exclusive
                          ? ` · level ${(trackSess.gainDb ?? 0).toFixed(1)} dB`
                          : ""
                      }`}
                    </span>
                  )}
                </>
              ) : (
                <span className="dim-sm">no track playing</span>
              )}
              <span className="spacer" />
              <span className="mono dim-sm">space · C = cursor · I·O = in/out · scroll zoom · drag pan</span>
              {ioRegion && (
                <>
                  <span
                    className={`scale-opt ${loopOn ? "on" : ""}`}
                    title="loop the I/O region"
                    onClick={() => setLoopOn((o) => !o)}
                  >
                    loop
                  </span>
                  <span
                    className="scale-opt"
                    title="clear the I/O region (Alt+X or Esc)"
                    onClick={clearRegion}
                  >
                    ×
                  </span>
                </>
              )}
              <span className="preset-wrap">
                <span
                  className="hist-chip mono"
                  title="Clip Studio view settings — this room's own options"
                  onClick={() => setClipsMenu((o) => !o)}
                >
                  ⚙
                </span>
                {clipsMenu && (
                  <div className="preset-menu device-menu room-menu">
                    <span className="mono lab-label">CLIP STUDIO VIEW</span>
                    <div className="room-row">
                      <span
                        className="room-key"
                        title="One-click view setups. Melody: 8k window, spectrogram solo, log axis, ~1 ms view at the playhead — the harmonics become live pitch lines that ride the melody as it plays. Standard: all scopes, 2k window, whole track."
                      >
                        View preset
                      </span>
                      <div className="seg seg-sm">
                        <span className="seg-opt" onClick={() => applyViewPreset("melody")}>
                          Melody
                        </span>
                        <span className="seg-opt" onClick={() => applyViewPreset("standard")}>
                          Standard
                        </span>
                      </div>
                    </div>
                    <div className="room-row">
                      <span className="room-key">Timecode decimals</span>
                      <div className="seg seg-sm">
                        {[1, 2, 3].map((v) => (
                          <span
                            key={v}
                            className={`seg-opt ${tcDec === v ? "on" : ""}`}
                            onClick={() => pickTcDec(v)}
                          >
                            {`.${"0".repeat(v)}`}
                          </span>
                        ))}
                      </div>
                    </div>
                    <div className="room-row">
                      <span className="room-key">Scopes</span>
                      <div className="is-tags">
                        <span
                          className={`scale-opt ${specOn ? "on" : ""}`}
                          onClick={() => toggleScope("spec")}
                        >
                          Spec
                        </span>
                        <span
                          className={`scale-opt ${waveOn ? "on" : ""}`}
                          onClick={() => toggleScope("wave")}
                        >
                          Wave
                        </span>
                        <span
                          className={`scale-opt ${fftOn ? "on" : ""}`}
                          onClick={() => toggleScope("fft")}
                        >
                          FFT
                        </span>
                      </div>
                    </div>
                    <div className="room-row">
                      <span
                        className="room-key"
                        title="FFT window: smaller = sharper in time (transients, rhythm), bigger = sharper in frequency (tones, harmonics). 256 ≈ 5 ms slices; 8k ≈ 170 ms."
                      >
                        Spectrogram window
                      </span>
                      <div className="seg seg-sm">
                        {(
                          [
                            [256, "256"],
                            [512, "512"],
                            [1024, "1k"],
                            [2048, "2k"],
                            [4096, "4k"],
                            [8192, "8k"],
                          ] as const
                        ).map(([v, label]) => (
                          <span
                            key={v}
                            className={`seg-opt ${specWin === v ? "on" : ""}`}
                            onClick={() => pickSpecParam("specwin", v)}
                          >
                            {label}
                          </span>
                        ))}
                      </div>
                    </div>
                    <div className="room-row">
                      <span
                        className="room-key"
                        title="Log matches hearing (octaves get equal space). Linear gives every Hz equal space — harmonic stacks read as evenly spaced lines, and the treble half isn't squeezed."
                      >
                        Spectrogram axis
                      </span>
                      <div className="seg seg-sm">
                        <span
                          className={`seg-opt ${!specLinear ? "on" : ""}`}
                          onClick={() => pickSpecParam("speclinear", 0)}
                        >
                          Log
                        </span>
                        <span
                          className={`seg-opt ${specLinear ? "on" : ""}`}
                          onClick={() => pickSpecParam("speclinear", 1)}
                        >
                          Linear
                        </span>
                      </div>
                    </div>
                    <div className="room-row">
                      <span className="room-key">Spectrogram floor</span>
                      <div className="seg seg-sm">
                        {(
                          [
                            [-70, "hot"],
                            [-90, "normal"],
                            [-110, "deep"],
                          ] as const
                        ).map(([v, label]) => (
                          <span
                            key={v}
                            className={`seg-opt ${specFloor === v ? "on" : ""}`}
                            onClick={() => pickSpecParam("specfloor", v)}
                          >
                            {label}
                          </span>
                        ))}
                      </div>
                    </div>
                    <div className="room-row">
                      <span
                        className="room-key"
                        title="How held-C chases your cursor. Chase is its reaction time — every chase interval it closes ~63% of the remaining gap to the mouse (smaller = tighter to your hand, larger = smoother tape-like glide). Max caps how fast it may play while catching up, in multiples of real time. Defaults: 60 ms · 8×."
                      >
                        Scrub feel
                      </span>
                      <div className="trials-ctl">
                        <input
                          key={`scrubtau-${scrubTau}`}
                          className="trials-input mono"
                          type="number"
                          min={5}
                          max={1000}
                          step={5}
                          defaultValue={scrubTau}
                          onBlur={(e) => commitScrub("tau", +e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                          }}
                        />
                        <span className="dim-sm">ms chase</span>
                        <input
                          key={`scrubmax-${scrubMax}`}
                          className="trials-input mono"
                          type="number"
                          min={0.5}
                          max={64}
                          step={0.5}
                          defaultValue={scrubMax}
                          onBlur={(e) => commitScrub("max", +e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                          }}
                        />
                        <span className="dim-sm">× max</span>
                      </div>
                    </div>
                    <div className="room-row">
                      <span className="room-key">Play method</span>
                      <div className="seg seg-sm">
                        <span
                          className={`seg-opt ${studioMode === "bypass" ? "on" : ""}`}
                          onClick={() => pickStudioMode("bypass")}
                          title="Curation: the track itself — exclusive device, no EQ, level-matched toward the reference. Takes effect on the next play."
                        >
                          Bypass
                        </span>
                        <span
                          className={`seg-opt ${studioMode === "eq" ? "on" : ""}`}
                          onClick={() => pickStudioMode("eq")}
                          title="A regular player: the normal shared path — your EQ and the level-matched A/B apply like for any stream. Takes effect on the next play."
                        >
                          Through EQ
                        </span>
                      </div>
                    </div>
                  </div>
                )}
              </span>
            </div>
            <div className="clips-viewer">
              {trackSess && !trackSess.phase ? (
                <>
                  <div
                    className="pane-wrap"
                    style={{
                      // Weight ∝ visible lanes; the whole column when FFT is away.
                      flex: `${(specOn && !popped.spec ? 3 : 0) + (waveOn ? 2 : 0) || 1} 1 0`,
                    }}
                  >
                    <TimelineView
                      trackId={trackSess.id}
                      durationS={trackSess.durationS}
                      posS={trackPos.posS}
                      onSeek={(s) => invoke("track_seek", { seconds: s }).catch(() => {})}
                      onHover={(t) => {
                        timelineHover.current = t;
                        if (t != null) scrubMove(t);
                      }}
                      region={ioRegion}
                      onRegionChange={(r) => {
                        setActiveClipId(null); // a trimmed region is no longer that clip
                        setIoRegion(r);
                      }}
                      spec={specData}
                      showSpec={specOn && !popped.spec}
                      showWave={waveOn}
                      decimals={tcDec}
                      syncKey="main"
                      focus={clipFocus}
                      onFocusDone={() => setClipFocus(null)}
                    />
                    {specOn && !popped.spec && (
                      <span
                        className="pane-pop row-act"
                        title="pop the spectrogram into its own window (it leaves this page while open)"
                        onClick={() => popOutScope("spec")}
                      >
                        ⇱
                      </span>
                    )}
                  </div>
                  {fftOn && !popped.fft && (
                    <div className="pane-wrap" style={{ flex: "2 1 0" }}>
                      <FftView
                        trackId={trackSess.id}
                        posS={trackPos.posS}
                        eq={
                          state
                            ? { freqs: state.freqs, sumDb: state.sumDb, preampDb: state.preampDb }
                            : null
                        }
                      />
                      <span
                        className="pane-pop row-act"
                        title="pop the FFT into its own window (it leaves this page while open)"
                        onClick={() => popOutScope("fft")}
                      >
                        ⇱
                      </span>
                    </div>
                  )}
                </>
              ) : (
                <p className="dim-sm clips-empty">
                  Play a track to see its waveform. Timeline, in/out clips and markers come next —
                  then the spectrogram, FFT and moments.
                </p>
              )}
            </div>
            {trackSess && !trackSess.phase && (activeClip || ioRegion) && (
              <div className="insp-strip">
                {activeClip ? (
                  <>
                    <div className="is-cell">
                      <span className="mono is-label">CLIP NAME</span>
                      {clipRename != null ? (
                        <input
                          className="rename-input"
                          autoFocus
                          value={clipRename}
                          onChange={(e) => setClipRename(e.target.value)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") {
                              updateClip(activeClip.id, { name: clipRename });
                              setClipRename(null);
                            }
                            if (e.key === "Escape") setClipRename(null);
                          }}
                          onBlur={() => {
                            updateClip(activeClip.id, { name: clipRename });
                            setClipRename(null);
                          }}
                        />
                      ) : (
                        <span className="is-value">
                          {activeClip.name}
                          <span
                            className="row-act"
                            title="rename"
                            onClick={() => setClipRename(activeClip.name)}
                          >
                            ✎
                          </span>
                        </span>
                      )}
                    </div>
                    <div className="is-cell">
                      <span className="mono is-label">RANGE</span>
                      <span className="mono is-value">{`${fmtTcN(activeClip.tIn, tcDec)} → ${fmtTcN(activeClip.tOut, tcDec)}`}</span>
                    </div>
                    <div className="is-cell">
                      <span className="mono is-label">TAGS</span>
                      <div className="is-tags">
                        {["lows", "mids", "highs"].map((tag) => (
                          <span
                            key={tag}
                            className={`tag-chip ${activeClip.tags.includes(tag) ? `on ${tag}` : ""}`}
                            onClick={() => toggleTag(activeClip, tag)}
                          >
                            {tag.toUpperCase()}
                          </span>
                        ))}
                        {activeClip.tags
                          .filter((t) => !["lows", "mids", "highs"].includes(t))
                          .map((t) => (
                            <span
                              key={t}
                              className="tag-chip on custom"
                              title="click to remove"
                              onClick={() => toggleTag(activeClip, t)}
                            >
                              {t}
                            </span>
                          ))}
                        <input
                          className="tag-input mono"
                          placeholder="+tag"
                          onKeyDown={(e) => {
                            if (e.key === "Enter") {
                              const v = (e.target as HTMLInputElement).value.trim();
                              if (v) {
                                invoke<LibraryState>("clip_tag", {
                                  id: activeClip.id,
                                  tag: v,
                                  on: true,
                                })
                                  .then(setLibrary)
                                  .catch((e2) => showNotice(String(e2)));
                                (e.target as HTMLInputElement).value = "";
                              }
                            }
                          }}
                        />
                      </div>
                    </div>
                    <div className="is-cell is-grow">
                      <span className="mono is-label">NOTE</span>
                      <input
                        key={`note-${activeClip.id}`}
                        className="insp-note"
                        placeholder="what should the listener notice here?"
                        defaultValue={activeClip.note ?? ""}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                        }}
                        onBlur={(e) => {
                          const v = e.target.value;
                          if (v.trim() !== (activeClip.note ?? "")) {
                            updateClip(activeClip.id, { note: v });
                          }
                        }}
                      />
                    </div>
                  </>
                ) : (
                  ioRegion && (
                    <>
                      <div className="is-cell">
                        <span className="mono is-label">RANGE</span>
                        <span className="mono is-value">{`${fmtTcN(ioRegion.a, tcDec)} → ${fmtTcN(ioRegion.b, tcDec)}`}</span>
                      </div>
                      <span className="dim-sm">unsaved region — I·O and edge drags adjust it</span>
                      <span className="spacer" />
                      <button className="primary" onClick={saveClip}>
                        Save clip
                      </button>
                    </>
                  )
                )}
              </div>
            )}
          </div>
        </div>
      )}

      {view === "settings" && settings && (
        <div className="settings">
          <div className="set-col">
            <div className="set-sect">
              <span className="mono set-head">HONESTY</span>
              <div className="set-row">
                <span className="set-key">Level matching</span>
                <div className={`switch ${settings.levelMatching ? "on ok" : ""}`} onClick={toggleLevelMatching}>
                  <div className="knob" />
                </div>
                {settings.levelMatching ? (
                  <span className="set-live ok-text">on — every comparison loudness-matched to the reference</span>
                ) : (
                  <span className="set-live warn-text">off — every comparison is marked unmatched</span>
                )}
              </div>
              <span className="set-note">
                Louder reliably sounds better; matched is honest. Off keeps only clip protection — the A/B bar,
                trial room, and session records all mark comparisons as unmatched.
              </span>
            </div>

            <div className="set-sect">
              <span className="mono set-head">REFERENCE</span>
              <div className="set-row">
                <span className="set-key">Reference loudness</span>
                <ValueEdit
                  className="set-val mono"
                  display={`${fmtGain(settings.referenceDb)} dB`}
                  value={settings.referenceDb}
                  onCommit={(v) => setReference(Math.max(-30, Math.min(0, v)))}
                />
              </div>
              <div className="set-row">
                <span className="set-key">Set your volume</span>
                <button onClick={toggleCalNoise}>{calNoise ? "■ stop noise" : "▶ pink noise"}</button>
                <span className="set-live dim-sm">
                  {calNoise
                    ? "playing — adjust your volume until this sits comfortably"
                    : "plays through the normal path, level-matched like everything else"}
                </span>
              </div>
              <span className="set-note">
                Everything — flat, every preset, chains mid-edit — is normalized to this loudness. Play the
                pink noise and set your physical volume so it sits where you like listening: that knob
                position is your reference, and it absorbs the digital number entirely. −8 dB is a headroom
                budget, not a taste choice: a boosted chain can only be level-matched if its peaks fit under
                0 dBFS, so raising this toward 0 breaks matching. Volume anchoring comes later (Q-16).
              </span>
            </div>

            <div className="set-sect">
              <span className="mono set-head">SYSTEM</span>
              <div className="set-row">
                <span className="set-key">Start with Windows</span>
                <div className={`switch ${autostart ? "on" : ""}`} onClick={toggleAutostart}>
                  <div className="knob" />
                </div>
                <span className="set-live dim-sm">runs in the tray for hotkeys and profiles</span>
              </div>
              <div className="set-row">
                <span className="set-key">A/B hotkey</span>
                <span className="mono set-kbd">Ctrl + Shift + A</span>
              </div>
              <div className="set-row">
                <span className="set-key">Track engine</span>
                <button onClick={() => testTone("exclusive")} disabled={testingTone}>
                  {testingTone ? "playing…" : "play test tone"}
                </button>
                <span className="set-live dim-sm">quiet 2 s tone, exclusive path — other audio pauses</span>
              </div>
              <span className="set-note">
                Proves the direct device path Clip Studio's track engine uses: Equalizer APO and Windows
                volume are bypassed, so the tone starts silent and ramps in. Other apps' audio stops and
                may need a manual restart afterwards.
              </span>
            </div>
          </div>

          <div className="set-col">
            <div className="set-sect">
              <span className="mono set-head">INTERFACE</span>
              <div className="set-row">
                <span className="set-key">Mode</span>
                <div className="seg">
                  <span className={`seg-opt ${uiMode === "standard" ? "on" : ""}`} onClick={() => pickUiMode("standard")}>
                    Standard
                  </span>
                  <span className={`seg-opt ${uiMode === "advanced" ? "on" : ""}`} onClick={() => pickUiMode("advanced")}>
                    Advanced
                  </span>
                </div>
              </div>
              <span className="set-note">
                Same layout in both. Standard greys out advanced tools instead of hiding them — hover any greyed
                control to learn what it does.
              </span>
            </div>

            <div className="set-demo">
              <span className="mono set-head no-line">HOW GREYED CONTROLS BEHAVE</span>
              <div className="set-demo-row">
                <span className="mono">FINGERPRINTS</span>
                <span>Fingerprint Lab — measure and match headphones</span>
              </div>
              <div className="set-demo-tip">
                <div className="t-title">Fingerprint Lab</div>
                <p>
                  Measures how a headphone actually sounds at your ears using sweeps, so two headphones can be
                  matched or auditioned virtually. Advanced because it needs a measurement microphone and a quiet
                  room. Switch to Advanced mode to enable it when it ships (Phase 4).
                </p>
              </div>
            </div>

            <div className="set-sect">
              <span className="mono set-head">TOOL PATHS</span>
              <div className="set-row">
                <span className={`apo-dot ${settings.apoInstallPath ? "ok" : "bad"}`} />
                <span className="set-tool mono">Equalizer APO</span>
                {settings.apoInstallPath ? (
                  <span className="set-path mono">{settings.apoInstallPath}</span>
                ) : (
                  <span className="set-text">not detected — sourceforge.net/projects/equalizerapo</span>
                )}
              </div>
              <div className="set-row">
                <span className={`apo-dot ${tools?.ffmpeg ? "ok" : "bad"}`} />
                <span className="set-tool mono">ffmpeg</span>
                {tools?.ffmpeg ? (
                  <span className="set-path mono">{tools.ffmpeg}</span>
                ) : toolsProg?.which === "ffmpeg" ? (
                  <span className="mono dim-sm">{`downloading… ${toolsProg.pct ?? 0}%`}</span>
                ) : (
                  <button onClick={() => installTool("ffmpeg")}>Install</button>
                )}
              </div>
              <div className="set-row">
                <span className={`apo-dot ${tools?.ytdlp ? "ok" : "bad"}`} />
                <span className="set-tool mono">yt-dlp</span>
                {tools?.ytdlp ? (
                  <span className="set-path mono">{tools.ytdlp}</span>
                ) : toolsProg?.which === "yt-dlp" ? (
                  <span className="mono dim-sm">{`downloading… ${toolsProg.pct ?? 0}%`}</span>
                ) : (
                  <button onClick={() => installTool("yt-dlp")}>Install</button>
                )}
              </div>
              <span className="set-note">
                Equalizer APO: Fletcher writes only fletcher.txt and one Include line — Peace and hand-written
                configs are never touched. ffmpeg decodes everything the track engine plays; yt-dlp imports
                audio from URLs. Managed tools are fetched on demand into Fletcher's own tools folder, never
                installed system-wide.
              </span>
            </div>
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
                      <HistoryTree
                        data={treeData}
                        onJump={jumpTo}
                        onDelete={deleteNode}
                        onEdit={editNode}
                        onPreview={previewSnap}
                        onAbx={abxNodes}
                        onPromote={promoteNode}
                        onCompare={onCompareSpec}
                        onPopoutDiff={popOutDiff}
                        onGraft={graftNode}
                        notify={showNotice}
                      />
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
            <span className="preset-wrap">
              <span
                className="hist-chip mono"
                title="EQ view settings — this room's own options (the Settings tab is system-level)"
                onClick={() => setEqMenu((o) => !o)}
              >
                ⚙
              </span>
              {eqMenu && (
                <div className="preset-menu device-menu room-menu">
                  <span className="mono lab-label">EQ VIEW</span>
                  <div className="room-row">
                    <span className="room-key">Filter ordering</span>
                    <div className="seg seg-sm">
                      {(
                        [
                          ["freq", "By frequency"],
                          ["importance", "By importance"],
                          ["file", "File order"],
                        ] as const
                      ).map(([v, label]) => (
                        <span
                          key={v}
                          className={`seg-opt ${ordering === v ? "on" : ""}`}
                          onClick={() => pickOrdering(v)}
                        >
                          {label}
                        </span>
                      ))}
                    </div>
                  </div>
                  <span className="dim-sm room-note">
                    How the strip is sorted. AutoEQ writes biggest-correction-first — "by importance"
                    shows that. Files on disk are never reordered.
                  </span>
                </div>
              )}
            </span>
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

              {[...multiSel].map((si) => {
                const sf = state.filters[si];
                return sf?.enabled && sf.responseDb.length ? (
                  <path
                    key={`sc${si}`}
                    d={pathFrom(state.freqs, sf.responseDb, yOf)}
                    className={`sel-curve ${sf.gainDb >= 0 ? "boost" : "cut"}`}
                  />
                ) : null;
              })}
              <path d={pathFrom(state.freqs, state.sumDb.map((db) => db - state.preampDb), yOf)} className="sum-curve" />

              {state.filters.map((f, i) => (
                <circle
                  key={i}
                  cx={xOf(f.fcHz)}
                  cy={yOf(f.gainDb)}
                  r={multiSel.has(i) ? 7 : 5}
                  className={`handle ${f.gainDb >= 0 ? "boost" : "cut"} ${multiSel.has(i) ? "selected" : ""} ${
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
              const isSel = multiSel.has(i);
              const boost = f.gainDb >= 0;
              const editable = f.sourceFile === OWN_FILE;
              return (
                <div
                  key={i}
                  className={`cell ${isSel ? "selected" : ""} ${f.enabled ? "" : "off"} ${editable ? "" : "locked"}`}
                  onClick={(e) => handleSelect(e, i)}
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
                        selectOnly(i);
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
                        deleteSelected(i);
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
        {trackSess && (
          <div className="transport">
            <span
              className="tr-btn"
              title={trackPos.paused ? "play" : "pause"}
              onClick={() => invoke("track_toggle").catch((e) => showNotice(String(e)))}
            >
              {trackPos.paused ? "▶" : "❚❚"}
            </span>
            <span
              className="tr-btn"
              title="stop and release the device"
              onClick={() => invoke("track_stop").catch(() => {})}
            >
              ■
            </span>
            <input
              type="range"
              className="tr-seek"
              min={0}
              max={trackSess.durationS || 0}
              step={0.1}
              value={Math.min(trackPos.posS, trackSess.durationS || 0)}
              onChange={(e) => invoke("track_seek", { seconds: +e.target.value }).catch(() => {})}
            />
            <span className="mono dim-sm">{`${fmtTime(trackPos.posS)} / ${fmtTime(trackSess.durationS)}`}</span>
            {trackSess.mode === "bypass" ? (
              trackSess.exclusive ? (
                <span
                  className="mono dim-sm tr-mode ok-text"
                  title={`Curation: the track itself — exclusive device, no EQ, level ${(trackSess.gainDb ?? 0).toFixed(1)} dB toward the reference. Other apps' audio pauses; your amp knob is the volume.`}
                >
                  bypass · the track itself
                </span>
              ) : (
                <span
                  className="mono dim-sm tr-mode warn-text"
                  title="The device refused exclusive mode, so this plays on the shared path — Equalizer APO still applies your EQ. Not a true bypass."
                >
                  bypass unavailable · via APO
                </span>
              )
            ) : (
              <span
                className="mono dim-sm tr-mode"
                title="A regular player: the normal shared path — your EQ and the level-matched A/B apply exactly as everywhere else."
              >
                through your EQ
              </span>
            )}
          </div>
        )}
        <span className="mono dim-sm">Ctrl·Shift·A</span>
        {settings?.levelMatching === false ? (
          <span
            className="matched mono unmatched"
            title="Level matching is off (Settings → Honesty) — the louder side reliably sounds better."
          >
            <span className="dot bad" />
            unmatched — louder side wins
          </span>
        ) : ab.shortfallDb > 0.05 ? (
          <span
            className="matched mono unmatched"
            title={`This chain needs more headroom than clip safety allows: it sits ${ab.shortfallDb.toFixed(1)} dB above the reference, so the comparison is only imperfectly matched (TB-08). Lower the reference in Settings or trim boosts to fully match.`}
          >
            <span className="dot bad" />
            {`matched — short ${ab.shortfallDb.toFixed(1)} dB (clip cap)`}
          </span>
        ) : (
          <span className="matched mono">
            <span className="dot" />
            {`matched · B ${fmtGain(ab.matchDb)} dB`}
          </span>
        )}
        <span className="spacer" />
      </footer>
    </div>
  );
}
