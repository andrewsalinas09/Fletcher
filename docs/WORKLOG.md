# Worklog

Append-only session journal. Newest entry first.

**Entry template**
```
## YYYY-MM-DD — short title
- **Did:** what changed in the repo/design
- **Learned:** surprises, verified facts, dead ends
- **Next:** the single most useful next step
```

---

## 2026-08-22 — Idea-mapping session: the vision lands, six ADRs

- **Did:** Full feature brain-dump captured into FEATURES.md. Decisions locked: ADR-0002 (grey-don't-hide UI), ADR-0003 (flat reference + level matching by default), ADR-0004 (user-curated clip libraries / Clip Studio), ADR-0005 (Tauri 2 + TS/React shell), ADR-0006 (scope by sequencing, not cutting; everything-recorded provenance). Fingerprint Lab vision clarified: fingerprints are coupled HpTF∘ear measurements; personal libraries; export/import; bridging via shared headphones for at-home virtual auditioning (valid mainly <5–6 kHz). Q-01/02/06/07/11/13 resolved; Q-15 (fingerprint interchange format) opened; TB-13…TB-19 added. ROADMAP rewritten as six everything-ships phases. APO auto-install: no, for now.
- **Learned:** The reviewer-ecosystem/bridging idea makes the fingerprint *format* strategic — design it early (Phase 0/1), ship the Lab late. Preference vs. discrimination tests need separate statistical treatment (TB-15). Clip libraries being per-track + genre-tagged gives the stats engine content-type dimensions for free.
- **Next:** Q-04 research spike (how track-mode playback bypasses APO) + verify APO hot-reload semantics locally; then rewrite VISION as normative and scaffold Phase 1.

## 2026-08-22 — Founding session: name, stack, doc system

- **Did:** Project named **Fletcher** (after Fletcher–Munson). Rust core committed (ADR-0001). Documentation system bootstrapped mirroring the Humanity-Tech-Tree standard: CLAUDE.md rules, ADRs, OPEN-QUESTIONS (Q-01…Q-10), TESTBED (TB-01…TB-12), GLOSSARY, ROADMAP, research survey of the frontend landscape. Git remote added (github.com/andrewsalinas09/Fletcher).
- **Learned:** The modern-frontend niche is open — AQUA (Electron/React) was the only serious attempt and is abandoned. APO's config hot-reload makes system-wide A/B nearly free, but track-mode A/B **cannot** ride it (double-processing trap, TB-03) and needs an in-app engine. Level matching (LUFS) is what makes ABX statistics honest — and it's the feature no incumbent has.
- **Next:** Idea-mapping conversation with the user — capture the full feature vision, then rewrite VISION.md and start resolving Q-01/Q-02/Q-07.
