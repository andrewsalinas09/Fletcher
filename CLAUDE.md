# Fletcher

A modern, open-source frontend for Equalizer APO on Windows — parametric EQ that is genuinely pleasant to use, plus the feature no EQ frontend has ever shipped: honest, level-matched, blind ABX listening tests with real statistics. Named for the Fletcher–Munson equal-loudness curves.

## Current status

**Design phase, late.** `src/main.rs` is an empty scaffold. Committed: Rust core (ADR-0001), Tauri 2 + TypeScript/React shell (ADR-0005), grey-don't-hide UX (ADR-0002), level-matching by default (ADR-0003), user-curated clip libraries (ADR-0004), everything-ships-in-phases scope (ADR-0006). See ROADMAP for current focus.

## Doc map — where everything lives

| File | Contains | Read when |
|---|---|---|
| `docs/VISION.md` | Why this exists, north-star goals, what it feels like to use | Starting any session |
| `docs/ARCHITECTURE.md` | Current-state design: components, data flow, the two A/B engines | Touching design |
| `docs/FEATURES.md` | The full feature map from idea-mapping; tiering pending Q-07 | Scoping any feature work |
| `docs/decisions/` | One ADR per settled decision, with the *why* | Before proposing any design change |
| `docs/OPEN-QUESTIONS.md` | Unresolved debates, each with a Q-ID and status | Before designing anything new |
| `docs/TESTBED.md` | Every known edge case (TB-IDs); the design acceptance suite | Before AND after any design change |
| `docs/GLOSSARY.md` | Domain terms (APO, biquad, LUFS, ABX…) | A term is unfamiliar |
| `docs/research/` | External facts with sources (APO mechanics, competitor landscape) | Verifying a claim about the outside world |
| `docs/ROADMAP.md` | Phases and **current focus** | Starting any session |
| `docs/WORKLOG.md` | Append-only session journal | Resuming after a gap |
| `docs/archive/` | Raw brain-dumps and chat digests, verbatim | Almost never (history only) |

## Rules for every agent session

1. **Orient first.** Read this file, `docs/ROADMAP.md` (current focus), and the last 2–3 entries of `docs/WORKLOG.md` before doing anything.
2. **Never re-litigate an Accepted ADR.** If new information genuinely contradicts one, don't argue in chat — write a new ADR that supersedes it (and mark the old one `Superseded by ADR-XXXX`). The user decides.
3. **Every settled design decision becomes an ADR** before the session ends, using `docs/decisions/ADR-0000-template.md`. A decision that lives only in chat history is lost.
4. **Every unresolved debate goes to OPEN-QUESTIONS** with a Q-ID. When it's later settled, mark it `Resolved → ADR-XXXX`.
5. **Every edge case goes to TESTBED.md the moment it's raised** — before it's solved. Every design change must be checked against the TB cases. Prime directive: Fletcher must never damage a user's existing audio setup — failure modes may lose Fletcher's own state, never the user's config or hearing (no surprise full-volume output, ever).
6. **End every session with a WORKLOG entry.** What changed, what was learned, what's next.
7. **Living docs describe *now*.** `ARCHITECTURE.md`, `GLOSSARY.md`, `ROADMAP.md` are always current-state — update them in place. History belongs in ADRs, the worklog, and git.
8. **Claims about Equalizer APO's behavior go in `docs/research/` with a source** (its wiki, source code, or a local experiment written up). Design built on an unverified assumption about APO is a bug.
9. **Document authority:** ADRs and `TESTBED.md` are normative; `ARCHITECTURE.md` and `README.md` are derived — on conflict, normative wins.
