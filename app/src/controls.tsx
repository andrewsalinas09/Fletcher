// Shared controls: one look and one behavior everywhere (owner, 2026-11-04:
// "everywhere this should be standard"). A new slider anywhere uses
// SliderField, not a bare <input type="range">.
import { useState } from "react";

/** A labeled slider with a typed value and its unit: the EQ page's scale
 *  sliders, as the standard. Dragging changes the value live; typing changes
 *  it live whenever the text is a number (half-typed "-" waits); Enter or
 *  leaving the box commits. `onRelease` fires when a drag or an edit ends. */
export function SliderField({
  label,
  value,
  min,
  max,
  step,
  unit,
  onChange,
  onRelease,
  onTyped,
  digits,
  title,
  className = "",
}: {
  label: React.ReactNode;
  value: number;
  min: number;
  max: number;
  step: number;
  unit?: string;
  onChange: (v: number) => void;
  /** A drag or a typed edit ended (e.g. to re-center a track). */
  onRelease?: (v: number) => void;
  /** A typed value, which may lie outside min–max (defaults to onChange). */
  onTyped?: (v: number) => void;
  /** Decimals shown in the box (default: from `step`). */
  digits?: number;
  title?: string;
  className?: string;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const places = digits ?? (step >= 1 ? 0 : step >= 0.1 ? 1 : 2);
  const release = (e: React.SyntheticEvent<HTMLInputElement>) => onRelease?.(+e.currentTarget.value);
  return (
    <label className={`slider-field ${className}`} title={title}>
      <span className="dim-sm">{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={Math.max(min, Math.min(max, value))}
        onChange={(e) => {
          setDraft(null);
          onChange(+e.target.value);
        }}
        onPointerUp={release}
        onKeyUp={release}
      />
      <input
        className="scale-num mono"
        type="number"
        step={step}
        value={draft ?? value.toFixed(places)}
        onChange={(e) => {
          setDraft(e.target.value);
          const v = Number(e.target.value);
          if (e.target.value.trim() !== "" && Number.isFinite(v)) (onTyped ?? onChange)(v);
        }}
        onBlur={() => {
          setDraft(null);
          onRelease?.(value);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        }}
      />
      {unit && <span className="dim-sm">{unit}</span>}
    </label>
  );
}
