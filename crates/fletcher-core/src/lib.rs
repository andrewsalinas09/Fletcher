//! Fletcher's engine: Equalizer APO config parsing, filter math, device layer.
//!
//! Design constraints (see docs/decisions/):
//! - ADR-0007: Fletcher owns only its own files; anything it did not write must
//!   round-trip byte-for-byte ("never destroy what we don't understand", Q-09).
//! - ADR-0001: all DSP and parsing logic lives here, never in the UI.

pub mod apo;
pub mod config;
pub mod devices;
pub mod dsp;
pub mod fsx;
pub mod peace;
pub mod presets;
