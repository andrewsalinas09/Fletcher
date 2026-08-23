# ADR-0001: The core is Rust

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Founding conversation (user: "I like rust. I want speed and power. Seems like rust it is?")

## Context
Fletcher needs: file watching and atomic config writes, registry access, audio-device enumeration (MMDevice), global hotkeys, a tray presence — and, for track-mode A/B, a real-time audio engine (decode, biquad DSP, sample-accurate crossfade, WASAPI output) plus loudness analysis (LUFS). The last group is latency-sensitive: GC pauses or interpreter overhead in the audio callback are audible.

## Decision
All non-UI logic — config engine, device layer, filter math, track engine, ABX core — is Rust.

## Why
- Real-time audio callbacks with no GC; the DSP and loudness math are trivial loads for native code.
- The crate ecosystem covers every need: `symphonia` (decode), `ebur128` (LUFS), `cpal`/`wasapi` (output), `windows` (registry, MMDevice), `notify` (file watch).
- User preference and the wish for "speed and power" — motivation is a real engineering input for a solo-driven project.

Rejected: **C#** (NAudio makes the Windows-audio side easy, but weaker for the real-time engine and nobody wanted it), **Electron/Node for the core** (native modules would be needed for exactly the hard parts — WASAPI, real-time DSP), **C++** (all of Rust's costs, fewer of its safety wins, no motivational pull).

## Consequences
- The UI question (Q-01) is only about the shell; whatever wins talks to a Rust core.
- Filter math is written once in Rust and shared by the curve renderer, auto-preamp, and track engine — the UI never reimplements DSP.
- Contributors need Rust; acceptable for this project's likely contributor pool.
