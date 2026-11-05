// The preset palette (Q-33, design C): one search over your presets and
// AutoEQ, the list, and a last row, "New preset…", that turns the palette into
// the list of sources. Rename, duplicate and delete live in the preset's own
// settings (App's PRESET SETTINGS modal); rows show only what you read.
// App places this and supplies every action, so the menu can be redesigned
// here alone.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

export type AutoeqEntry = { name: string; path: string; note: string };

/** Another tool's file in APO's config (Peace, …): a source to copy from. */
export type ExternalSource = { file: string; label: string; filtersOn: number };

type Row = { key: string; run: () => void };

export function PresetPalette({
  presets,
  active,
  hotkeyOf,
  externals,
  busy,
  flatTip,
  onPick,
  onImportAeq,
  onCreate,
  onRew,
  onCopyExternal,
  onSettings,
  onClose,
  onError,
}: {
  presets: string[];
  active: string | null;
  /** The preset's hotkey as shown ("Ctrl+Alt+1"), when it has one on. */
  hotkeyOf: (name: string) => string | null;
  externals: ExternalSource[];
  /** An import is running. */
  busy: boolean;
  /** Hover props for Flat's explanation (App's tooltip). */
  flatTip: object;
  onPick: (name: string | null) => void;
  onImportAeq: (e: AutoeqEntry) => void;
  onCreate: (fromLive: boolean) => void;
  onRew: () => void;
  onCopyExternal: (file: string) => void;
  onSettings: (name: string) => void;
  onClose: () => void;
  onError: (msg: string) => void;
}) {
  const [page, setPage] = useState<"list" | "new">("list");
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const [aeq, setAeq] = useState<AutoeqEntry[]>([]);
  const query = q.trim().toLowerCase();

  // AutoEQ's index is cached by the backend; it's searched once you've typed
  // two letters, a moment after you stop.
  useEffect(() => {
    if (query.length < 2) return setAeq([]);
    const t = window.setTimeout(() => {
      invoke<AutoeqEntry[]>("autoeq_search", { query })
        .then(setAeq)
        .catch((e) => onError(String(e)));
    }, 300);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query]);

  const mine = presets
    .filter((p) => !query || p.toLowerCase().includes(query))
    .sort((x, y) => {
      const sx = x.toLowerCase().startsWith(query) ? 0 : 1;
      const sy = y.toLowerCase().startsWith(query) ? 0 : 1;
      return sx - sy || x.localeCompare(y);
    });
  const showFlat = !query || "flat".includes(query);

  const sources: (Row & { title: string; note: string; off?: boolean })[] = [
    { key: "empty", title: "Empty", note: "no filters, build your own", run: () => onCreate(false) },
    { key: "live", title: "What's playing", note: "everything you hear now, other tools' filters too", run: () => onCreate(true) },
    { key: "rew", title: "REW file…", note: "a Room EQ Wizard filter export, read as APO plays it", run: onRew },
    ...externals.map((x) => ({
      key: `ext:${x.file}`,
      title: `${x.label}'s chain`,
      note: `${x.filtersOn ? `${x.filtersOn} filters on` : "inactive"} · copied even while it's off`,
      off: !x.filtersOn,
      run: () => onCopyExternal(x.file),
    })),
  ];

  const rows: Row[] =
    page === "new"
      ? sources
      : [
          ...(showFlat ? [{ key: "flat", run: () => onPick(null) }] : []),
          ...mine.map((p) => ({ key: `p:${p}`, run: () => onPick(p) })),
          ...aeq.map((r) => ({ key: `aeq:${r.path}/${r.name}`, run: () => onImportAeq(r) })),
          { key: "new", run: () => openNew() },
        ];
  const cur = Math.min(sel, Math.max(0, rows.length - 1));
  const kbd = (key: string) => (rows[cur]?.key === key ? "kbd" : "");
  const openNew = () => {
    setPage("new");
    setSel(0);
  };
  const back = () => {
    setPage("list");
    setSel(0);
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel(Math.min(cur + 1, rows.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel(Math.max(cur - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      rows[cur]?.run();
    } else if (e.key === "Escape") {
      e.preventDefault();
      if (page === "new") back();
      else onClose();
    }
  };

  const mark = (text: string) => {
    const i = query ? text.toLowerCase().indexOf(query) : -1;
    if (i < 0) return text;
    return (
      <>
        {text.slice(0, i)}
        <mark className="pal-mark">{text.slice(i, i + query.length)}</mark>
        {text.slice(i + query.length)}
      </>
    );
  };

  return (
    <>
      <div className="hist-backdrop" onClick={onClose} />
      <div className="palette" onKeyDown={onKey}>
        {page === "list" ? (
          <input
            className="pal-input mono"
            autoFocus
            placeholder="search presets and 5,000+ AutoEQ headphones…"
            value={q}
            onChange={(e) => {
              setQ(e.target.value);
              setSel(0);
            }}
          />
        ) : (
          // Focusable, so the arrow keys and Esc keep working on this page.
          <div className="pal-back mono" tabIndex={0} ref={(el) => el?.focus()} onClick={back}>
            ‹ NEW PRESET
          </div>
        )}

        {page === "list" ? (
          <div className="pal-list">
            {showFlat && (
              <div className={`preset-row ${active == null ? "current" : ""} ${kbd("flat")}`} onClick={() => onPick(null)} {...flatTip}>
                <span>Flat</span>
                <span className="spacer" />
                <span className="pal-src">no Fletcher filters</span>
              </div>
            )}
            {mine.length > 0 && <span className="mono lab-label c-sect">YOUR PRESETS</span>}
            {mine.map((p) => {
              const hk = hotkeyOf(p);
              return (
                <div key={p} className={`preset-row pal-preset ${active === p ? "current" : ""} ${kbd(`p:${p}`)}`} onClick={() => onPick(p)}>
                  <span>{mark(p)}</span>
                  <span className="spacer" />
                  {hk && <span className="pal-hk mono">{hk}</span>}
                  <span
                    className="row-act pal-cfg"
                    title="Settings: name, hotkey, duplicate, delete"
                    onClick={(e) => {
                      e.stopPropagation();
                      onSettings(p);
                    }}
                  >
                    ⚙
                  </span>
                </div>
              );
            })}
            {query && !mine.length && !showFlat && <div className="preset-row dim-sm">none of your presets match</div>}
            {(aeq.length > 0 || busy) && <span className="mono lab-label c-sect">FROM AUTOEQ · imports as a new preset</span>}
            {busy && (
              <div className="preset-row">
                <span className="dim-sm">importing…</span>
              </div>
            )}
            {!busy &&
              aeq.map((r) => (
                <div key={`${r.path}/${r.name}`} className={`preset-row ${kbd(`aeq:${r.path}/${r.name}`)}`} onClick={() => onImportAeq(r)}>
                  <span>{mark(r.name)}</span>
                  <span className="spacer" />
                  <span className="pal-src">{r.note.replace(/^by /, "")}</span>
                </div>
              ))}
            <div className={`preset-row pal-add ${kbd("new")}`} onClick={openNew}>
              <span className="pal-plus mono">+</span>
              <span>New preset…</span>
            </div>
          </div>
        ) : (
          <div className="pal-list pal-sources">
            {sources.map((s) => (
              <div key={s.key} className={`preset-row ${kbd(s.key)}`} onClick={s.run}>
                <b className={s.off ? "pal-off" : ""}>{s.title}</b>
                <span className="dim-sm">{s.note}</span>
              </div>
            ))}
          </div>
        )}
        <div className="pal-foot mono dim-sm">↑↓ navigate · Enter select · Esc {page === "new" ? "back" : "close"}</div>
      </div>
    </>
  );
}
