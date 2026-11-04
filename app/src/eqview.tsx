// The EQ page's view settings (⚙): filter order, ears, and the ghosts behind
// the curve, as a layer list (design A, 2026-11-04). One component, used in the
// popover and in its own window: the main window owns the state and sends it
// (`ev-sync`); the window sends commands back (`ev-cmd`), the Q-20 pattern
// (rule 10). Redesigning the panel means touching this file.
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { TargetPicker, type GhostPrefs, type TargetLibrary } from "./targets";

export type EarView = "left" | "right" | "both" | "split";
export type Ordering = "freq" | "importance" | "file";

/** Everything the panel shows, as main sees it. */
export type EqViewState = {
  ordering: Ordering;
  earView: EarView;
  /** Settings › EARS › Edit each ear separately. */
  perEar: boolean;
  /** The active preset's ears differ, so they can be joined. */
  canJoin: boolean;
  otherEar: boolean;
  ghosts: GhostPrefs;
  /** The current output's headphone ("Sennheiser HD 650 · oratory1990"). */
  headphone: string | null;
  measurementError: string | null;
  targetError: string | null;
  hasBounds: boolean;
  deviceName: string | null;
};

export type EqViewCmd =
  | { type: "ordering"; v: Ordering }
  | { type: "earView"; v: EarView }
  | { type: "otherEar"; on: boolean }
  | { type: "ghosts"; prefs: GhostPrefs }
  | { type: "join"; keep: "left" | "right" }
  | { type: "openDevice" };

const ORDERINGS: [Ordering, string][] = [
  ["freq", "Frequency"],
  ["importance", "Importance"],
  ["file", "File"],
];
const EAR_VIEWS: [EarView, string][] = [
  ["left", "Left"],
  ["right", "Right"],
  ["both", "Both"],
  ["split", "Split"],
];
const SMOOTHINGS: [string, string][] = [
  ["none", "none"],
  ["1/24", "1/24"],
  ["1/12", "1/12"],
  ["1/6", "1/6"],
  ["1/3", "1/3"],
  ["erb", "ERB"],
];

/** A ghost's swatch: the same line the graph draws for it. */
const Swatch = ({ cls }: { cls: string }) => (
  <svg width="22" height="10" className="ev-swatch">
    <line x1="1" x2="21" y1="5" y2="5" className={cls} />
  </svg>
);

function Switch({ on, enabled = true, onToggle }: { on: boolean; enabled?: boolean; onToggle: () => void }) {
  return (
    <div className={`switch ${on ? "on" : ""} ${enabled ? "" : "disabled"}`} onClick={() => enabled && onToggle()}>
      <div className="knob" />
    </div>
  );
}

function Seg<T extends string>({ opts, value, enabled = true, onPick }: { opts: [T, string][]; value: T; enabled?: boolean; onPick: (v: T) => void }) {
  return (
    <div className="seg seg-sm">
      {opts.map(([v, label]) => (
        <span
          key={v}
          className={`seg-opt ${enabled && value === v ? "on" : ""} ${enabled ? "" : "disabled"}`}
          onClick={() => enabled && onPick(v)}
        >
          {label}
        </span>
      ))}
    </div>
  );
}

export function EqViewPanel({
  st,
  send,
  library,
  onPopOut,
}: {
  st: EqViewState;
  send: (c: EqViewCmd) => void;
  library: TargetLibrary | null;
  /** Present in the popover; the window has no pop-out of its own. */
  onPopOut?: () => void;
}) {
  const [picking, setPicking] = useState(false);
  const g = st.ghosts;
  const set = (patch: Partial<GhostPrefs>) => send({ type: "ghosts", prefs: { ...g, ...patch } });
  const hp = st.headphone;
  const layer = (swatch: string, name: string, detail: React.ReactNode, on: boolean, enabled: boolean, toggle: () => void, sub = false) => (
    <div className={`ev-layer ${on && enabled ? "" : "off"} ${sub ? "sub" : ""}`}>
      <Swatch cls={swatch} />
      <div className="ev-what">
        <b>{name}</b>
        {detail && <span className="dim-sm">{detail}</span>}
      </div>
      <Switch on={on && enabled} enabled={enabled} onToggle={toggle} />
    </div>
  );
  const noHp = (
    <>
      {`none on ${st.deviceName ?? "this output"} · `}
      <span className="link-like" onClick={() => send({ type: "openDevice" })}>
        set one
      </span>
    </>
  );
  return (
    <div className="ev-panel">
      {onPopOut && (
        <div className="ev-head">
          <span className="mono lab-label">EQ VIEW</span>
          <span className="spacer" />
          <span className="ev-popout" title="Pop out into its own window" onClick={onPopOut}>
            ⧉
          </span>
        </div>
      )}
      <div className="ev-sect">
        <div className="ev-row">
          <span className="room-key">Filter order</span>
          <Seg opts={ORDERINGS} value={st.ordering} onPick={(v) => send({ type: "ordering", v })} />
        </div>
      </div>

      <div className="ev-sect">
        <span className="mono lab-label">EARS</span>
        <div className="ev-row">
          <span className="room-key">Show</span>
          <Seg opts={EAR_VIEWS} value={st.earView} enabled={st.perEar} onPick={(v) => send({ type: "earView", v })} />
        </div>
        {!st.perEar && <span className="dim-sm">Each ear's own EQ is off (Settings › EARS).</span>}
        {st.canJoin && (
          <div className="ev-row">
            <span className="room-key">Join ears</span>
            <div className="room-row">
              {(["left", "right"] as const).map((keep) => (
                <span
                  key={keep}
                  className="fe-btn mono"
                  title={`One chain for both ears: the ${keep} ear's, with its history. The other ear's chain and history are dropped.`}
                  onClick={() => send({ type: "join", keep })}
                >
                  keep {keep}
                </span>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="ev-sect">
        <span className="mono lab-label">GHOSTS</span>
        {layer("ghost-other", "Other ear", null, st.otherEar, st.perEar, () => send({ type: "otherEar", on: !st.otherEar }))}
        {layer("ghost-meas", "Measurement", hp ?? noHp, g.measurement, !!hp, () => set({ measurement: !g.measurement }))}
        {st.measurementError && hp && <span className="dim-sm warn-text ev-indent">{st.measurementError}</span>}
        {layer("ghost-pred", "With your EQ", null, g.predicted, !!hp, () => set({ predicted: !g.predicted }))}
        <div className={`ev-layer ${g.target ? "" : "off"}`}>
          <Swatch cls="ghost-target" />
          <div className="ev-what ev-inline">
            <b>Target</b>
            <span className="ev-field" onClick={() => setPicking((p) => !p)}>
              <span className="t">{g.targetName ?? "choose a target"}</span>
              <span className="v">{picking ? "▲" : "▼"}</span>
            </span>
          </div>
          <Switch on={g.target} enabled={!!g.targetRef || g.target} onToggle={() => set({ target: !g.target })} />
        </div>
        {picking && (
          <div className="ev-pick">
            <TargetPicker
              value={g.targetRef}
              library={library}
              onPick={(i) => {
                set({ targetRef: i?.target ?? null, targetName: i?.name ?? null, target: !!i });
                setPicking(false);
              }}
            />
          </div>
        )}
        {st.targetError && g.target && <span className="dim-sm warn-text ev-indent">{st.targetError}</span>}
        {layer(
          "ghost-bounds-sw",
          "Bounds",
          st.hasBounds ? null : "none for this target",
          g.bounds,
          st.hasBounds && g.target,
          () => set({ bounds: !g.bounds }),
          true,
        )}
        <div className="ev-row">
          <span className="room-key">Smoothing</span>
          <Seg opts={SMOOTHINGS} value={g.smoothing} onPick={(v) => set({ smoothing: v })} />
        </div>
      </div>
    </div>
  );
}

/** The panel in its own window. It loads the target library itself; the rest
 *  comes from the main window. */
export function PopoutEqView() {
  const [st, setSt] = useState<EqViewState | null>(null);
  const [library, setLibrary] = useState<TargetLibrary | null>(null);
  useEffect(() => {
    // Say hello only after the listener is registered (the reply is a one-shot).
    let un: (() => void) | undefined;
    let gone = false;
    listen<EqViewState>("ev-sync", (e) => !gone && setSt(e.payload)).then((f) => {
      if (gone) return f();
      un = f;
      emitTo("main", "ev-hello", {});
    });
    invoke<TargetLibrary>("targets").then(setLibrary).catch(() => {});
    return () => {
      gone = true;
      un?.();
    };
  }, []);
  if (!st) return <div className="fe-window-empty dim">waiting for the main window…</div>;
  return (
    <div className="ev-window">
      <EqViewPanel st={st} library={library} send={(c) => emitTo("main", "ev-cmd", c).catch(() => {})} />
    </div>
  );
}
