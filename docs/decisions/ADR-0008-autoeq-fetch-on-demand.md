# ADR-0008: AutoEQ data is fetched on demand, not bundled

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Development-kickoff decisions (user selection, overriding the bundled-snapshot recommendation)

## Context
The AutoEQ headphone database (thousands of presets, multiple targets) must reach users somehow: bundled snapshot (offline, stale, bigger installer) vs. fetched from the AutoEQ project on demand (fresh, needs network) — Q-08.

## Decision
Fletcher fetches AutoEQ data on demand and caches what it fetches. No snapshot is bundled.

## Why
User's call: always-fresh data and a lean installer outweigh offline-first-run. The stale-data failure mode of bundling bothered them more than the network dependency.

## Consequences
- First-run headphone search requires network; the UI must degrade gracefully offline (clear message + previously-cached entries still available — cache-on-fetch is required, not optional).
- The AutoEQ repo's file layout becomes a runtime interface — the fetch layer needs version tolerance and a testbed case when its structure shifts.
- Installer stays small; no snapshot-refresh machinery needed.
