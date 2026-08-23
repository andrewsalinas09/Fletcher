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

function FilterRow({ line }: { line: Extract<Line, { kind: "filter" }> }) {
  return (
    <div className={`row filter ${line.enabled ? "" : "off"}`}>
      <span className="badge type">{line.filterType}</span>
      <span className="num">
        {line.fcHz != null ? `${line.fcHz} Hz` : "—"}
      </span>
      <span className={`num gain ${line.gainDb != null && line.gainDb < 0 ? "cut" : "boost"}`}>
        {line.gainDb != null ? `${line.gainDb > 0 ? "+" : ""}${line.gainDb} dB` : ""}
      </span>
      <span className="num q">{line.q != null ? `Q ${line.q}` : ""}</span>
    </div>
  );
}

function LineView({ line }: { line: Line }) {
  switch (line.kind) {
    case "filter":
      return <FilterRow line={line} />;
    case "preamp":
      return (
        <div className="row">
          <span className="badge preamp">Preamp</span>
          <span className="num">{line.db} dB</span>
        </div>
      );
    case "include":
      return (
        <div className="row">
          <span className="badge include">Include</span>
          <span className="mono">{line.path}</span>
        </div>
      );
    case "device":
    case "channel":
      return (
        <div className="row">
          <span className="badge scope">{line.kind === "device" ? "Device" : "Channel"}</span>
          <span className="mono">{line.kind === "device" ? line.pattern : line.spec}</span>
        </div>
      );
    case "comment":
      return <div className="row dim mono">{line.text}</div>;
    case "blank":
      return null;
    case "unknown":
      return <div className="row dim mono">{line.text || "‹empty›"}</div>;
  }
}

export default function App() {
  const [status, setStatus] = useState<ApoStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = () =>
    invoke<ApoStatus>("apo_status").then(setStatus).catch((e) => setError(String(e)));

  useEffect(() => {
    refresh();
  }, []);

  return (
    <main>
      <header>
        <h1>Fletcher</h1>
        <span className="tagline">honest EQ</span>
        <button onClick={refresh}>Refresh</button>
      </header>

      {error && (
        <div className="card error">
          <strong>Equalizer APO problem:</strong> {error}
          <p>
            Fletcher drives Equalizer APO — install it from sourceforge.net/projects/equalizerapo
            and enable it for your output device, then hit Refresh.
          </p>
        </div>
      )}

      {status && (
        <>
          <div className="card status">
            Equalizer APO found at <span className="mono">{status.installPath}</span>
          </div>
          {status.files.map((f) => (
            <div className="card" key={f.name}>
              <h2 className="mono">{f.name}</h2>
              {f.lines.map((l, i) => (
                <LineView key={i} line={l} />
              ))}
              {f.lines.every((l) => l.kind === "blank") && (
                <div className="row dim">(empty)</div>
              )}
            </div>
          ))}
        </>
      )}
    </main>
  );
}
