# ADR-0005: The UI shell is Tauri 2 with TypeScript/React

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Idea-mapping session (user: "i agree unless you have any reason native is better / typescript/react would hold us back… I don't want it to get in the way __at all__")

## Context
The core is Rust (ADR-0001); the shell was open (Q-01). Fletcher's UI surfaces — draggable curve editor, live spectrograms, waveform clip-marking, stats dashboards, confidence-banded plots — are heavy on interactive 2D graphics and data visualization. The user works in TypeScript/React regularly; the one concern was whether the web stack could get in the way at all.

## Decision
Tauri 2, with a React + TypeScript frontend. All DSP, audio I/O, measurement, and statistics computation stays in the Rust core; the UI receives compact, precomputed frames and only draws.

## Why
- The UI surface list is exactly what the web stack is strongest and fastest at; a polished curve editor in egui/Slint/Iced is weeks of custom widget work.
- WebView2 is mature on Windows 11; installer stays ~10 MB (vs. Electron's ~150 MB — and Electron would still need the Rust core, plus native-module friction).
- The one identified risk — IPC bandwidth for live visualizations — is mitigated by design, not luck: Rust computes FFT/downsampled frames and pushes binary frames over Tauri channels; Canvas/WebGL renders at 60 fps.

Rejected: **native Rust GUI** (graphics-polish cost, weaker dataviz ecosystem), **Electron** (size, no benefit over Tauri given a Rust core).

## Consequences
- Two-language repo (Rust core, TS/React shell); the boundary is a designed API, and the UI never implements DSP (reaffirms ADR-0001).
- Visualization data flows are push-based binary frames — this shapes the core's public interface from the start.
- The audio path is untouchable by webview behavior; UI jank can never become audio jank.
