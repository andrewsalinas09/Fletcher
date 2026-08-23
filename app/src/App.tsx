import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type Line =
  | { kind: "preamp"; text: string; db: number }
  | {
      kind: "filter";
      text: string;
      index: number | null;
      enabled: boolean;
      filterType: string;
      fcHz: number | null;
      gainDb: number | null;
      q: number | null;
    }
  | { kind: "include"; text: string; path: string }
  | { kind: "device"; text: string; pattern: string }
  | { kind: "channel"; text: string; spec: string }
  | { kind: "comment"; text: string }
  | { kind: "blank" }
  | { kind: "unknown"; text: string };

type ApoStatus = {
  installPath: string;
  configPath: string;
  files: { name: string; lines: Line[] }[];
};

type FilterLine = Extract<Line, { kind: "filter" }>;

const fmtHz = (hz: number) =>
  hz >= 1000 ? `${+(hz / 1000).toFixed(2)}k` : `${+hz.toFixed(1)}`;

const fmtGain = (g: number) => `${g > 0 ? "+" : ""}${+g.toFixed(1)}`;

function GainBar({ gain }: { gain: number | null }) {
  const clamped = Math.max(-12, Math.min(12, gain ?? 0));
  const half = Math.abs(clamped) / 12 / 2; // fraction of bar height
  const isBoost = clamped >= 0;
  return (
    <div className="gainbar" aria-hidden>
      <div
        className={`gainbar-fill ${isBoost ? "boost" : "cut"}`}
        style={
          isBoost
            ? { bottom: "50%", height: `${half * 100}%` }
            : { top: "50%", height: `${half * 100}%` }
        }
      />
      <div className="gainbar-axis" />
    </div>
  );
}

function FilterCard({ f }: { f: FilterLine }) {
  return (
    <div className={`fcard ${f.enabled ? "" : "off"}`}>
      <span className="ftype">{f.filterType}</span>
      <GainBar gain={f.gainDb} />
      <span
        className={`fgain ${
          f.gainDb == null ? "" : f.gainDb >= 0 ? "boost" : "cut"
        }`}
      >
        {f.gainDb != null ? fmtGain(f.gainDb) : "—"}
      </span>
      <span className="ffc">{f.fcHz != null ? fmtHz(f.fcHz) : "—"}</span>
      <span className="fq">{f.q != null ? `Q ${f.q}` : " "}</span>
    </div>
  );
}

function FilePanel({ name, lines }: { name: string; lines: Line[] }) {
  const filters = lines
    .filter((l): l is FilterLine => l.kind === "filter")
    .sort((a, b) => (a.fcHz ?? Infinity) - (b.fcHz ?? Infinity));
  const preamps = lines.filter((l) => l.kind === "preamp");
  const scopes = lines.filter(
    (l) => l.kind === "device" || l.kind === "channel",
  );
  const includes = lines.filter((l) => l.kind === "include");
  const other = lines.filter(
    (l) => (l.kind === "unknown" && l.text.trim()) || l.kind === "comment",
  );
  const empty =
    !filters.length && !preamps.length && !scopes.length && !includes.length;

  return (
    <section className="panel">
      <div className="panel-head">
        <h2>{name}</h2>
        <div className="chips">
          {scopes.map((s, i) => (
            <span className="chip scope" key={`s${i}`}>
              {s.kind === "device" ? "Device" : "Channel"}{" "}
              <b>{s.kind === "device" ? s.pattern : s.spec}</b>
            </span>
          ))}
          {includes.map((inc, i) => (
            <span className="chip include" key={`i${i}`}>
              → {inc.kind === "include" ? inc.path : ""}
            </span>
          ))}
        </div>
      </div>

      {(preamps.length > 0 || filters.length > 0) && (
        <div className="strip">
          {preamps.map((p, i) => (
            <div className="fcard preamp" key={`p${i}`}>
              <span className="ftype">PRE</span>
              <GainBar gain={p.kind === "preamp" ? p.db : 0} />
              <span
                className={`fgain ${
                  p.kind === "preamp" && p.db >= 0 ? "boost" : "cut"
                }`}
              >
                {p.kind === "preamp" ? fmtGain(p.db) : ""}
              </span>
              <span className="ffc">amp</span>
              <span className="fq">dB</span>
            </div>
          ))}
          {filters.map((f, i) => (
            <FilterCard f={f} key={i} />
          ))}
        </div>
      )}

      {empty && other.length === 0 && <p className="empty">empty</p>}
      {other.length > 0 && (
        <details className="raw">
          <summary>
            {other.length} other line{other.length > 1 ? "s" : ""}
          </summary>
          <pre>
            {other
              .map((l) => ("text" in l ? l.text : ""))
              .join("\n")}
          </pre>
        </details>
      )}
    </section>
  );
}

export default function App() {
  const [status, setStatus] = useState<ApoStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = () =>
    invoke<ApoStatus>("apo_status")
      .then((s) => {
        setStatus(s);
        setError(null);
      })
      .catch((e) => setError(String(e)));

  useEffect(() => {
    refresh();
  }, []);

  return (
    <div className="frame">
      <header>
        <span className="wordmark">Fletcher</span>
        <span className="sub">honest EQ</span>
        <span className="spacer" />
        {status && (
          <span className="engine">
            Equalizer APO · <span className="path">{status.installPath}</span>
          </span>
        )}
        <button onClick={refresh}>Refresh</button>
      </header>

      {error && (
        <section className="panel alert">
          <h2>Equalizer APO problem</h2>
          <p>{error}</p>
          <p className="dim">
            Fletcher drives Equalizer APO — install it from
            sourceforge.net/projects/equalizerapo, enable it for your output
            device, then refresh.
          </p>
        </section>
      )}

      {status?.files.map((f) => (
        <FilePanel key={f.name} name={f.name} lines={f.lines} />
      ))}
    </div>
  );
}
