# Open Questions

Unresolved design debates. Each gets a Q-ID; when settled, mark `Resolved → ADR-XXXX` and write the ADR. Statuses: **Open**, **Leaning** (candidate answer named, not committed), **Resolved**.

---

## Q-01 · UI shell — **Resolved → ADR-0005**
Tauri 2 + TypeScript/React. The one identified risk (IPC bandwidth for live visualizations) is mitigated by pushing compact Rust-computed binary frames; UI only draws.

## Q-02 · Audience and the meaning of "most user-friendly ever" — **Resolved (UI model) → ADR-0002**
One UI for everyone; advanced features greyed with hover explainers, never hidden. Also settled 2026-08-22: Fletcher does **not** install Equalizer APO itself — "no, at least not yet" — it detects and guides instead (revisit post-v1). Residual open: exact first-run wizard design.

## Q-03 · Coexistence with Peace / existing configs — **Resolved (wiring) → ADR-0007**
One `Include: fletcher.txt` line; Fletcher owns only its own file. Residual open: importing Peace presets (`.peace` files) and raw APO configs into Fletcher's preset store.

## Q-04 · Track-mode output path — **Resolved → ADR-0009**
Spike-verified: WASAPI exclusive bypasses APO. Ships as the default mode, with journaled config-blank as a selectable second mode / automatic fallback.

## Q-05 · Test protocols and statistics design — **Open (requirements sharpened 2026-08-22, twice)**
Late additions from Lab-home review: **(d) Protocol controls are prominent, not buried** — trial count customizable front-and-center; an "until statistically conclusive" adaptive mode with a Cancel (it may never converge). **(e) Sequential honesty**: the adaptive mode must use proper sequential statistics (checking after every trial inflates false positives — SPRT or alpha-spending corrected thresholds, not naive p&lt;0.05 per look), and the UI carries a plain professional note explaining that repeated looks raise the odds of a fluke "difference." **(f) Replication is first-class**: from any result, "run it again with the same N" — a conclusive result you can't reproduce gets flagged as such in the record. **(g) Layout note: Begin button right-aligned.**
New settled requirements from the user: **(a) Live stats are hidable-by-default during a session** — the user may reveal them mid-test, but the UI states in professional language that viewing interim results can bias sequential testing, and the session record notes when stats were viewed (provenance, TB-24). **(b) Labeled replay is mandatory**: every session stores the exact played sequence — trial order, X assignments, clips, matched levels — so the user can replay the whole session afterwards *with labels revealed* ("trial 7, X was B — listen again"). A test you can't relive is a test you can't learn from. **(c) The Lab is the retrieval surface**: Clip Studio annotates richly; the Lab's job is finding and searching exactly the material you want and assembling custom batteries from the database.
Two distinct test types are now in scope: **discrimination** (ABX — has a correct answer; binomial p-value) and **preference** (blind A/B vote — no correct answer; preference proportion vs. 50% null, needs its own stats treatment, TB-15). Design: trial counts fixed vs. sequential; what's reported; per-clip/per-band breakdowns from the clip battery (Q-11); history per user / per headphone / per preset pair; ear-training/gamification. Guard against misleading small-N (TB-10).

## Q-06 · Level-matching semantics — **Resolved (principle) → ADR-0003; estimation v1 shipped**
Flat reference always exists; everything matched against it by default, app-wide. **Estimation v1 (2026-08-23, in ab_set):** the chain's mean response over a 200-point log-frequency grid — equal weight per octave ≈ pink-noise weighting — applied as B's preamp, clamped to never boost. Residual open: validate v1 against real LUFS measurements once the track engine exists (Phase 3 can A/B the estimate against measured matching); rails behavior (TB-08); manual trim UX.

## Q-07 · Scope model — **Resolved → ADR-0006**
Nothing is cut; everything in FEATURES.md is committed scope, ordered into phases by importance and dependency. The phase plan itself lives in ROADMAP.md (the scope authority).

## Q-08 · AutoEQ database delivery — **Resolved → ADR-0008**
Fetch on demand with mandatory caching; no bundled snapshot. Residual open: which targets to surface (Harman variants, oratory1990) and the offline-degradation UX.

## Q-09 · APO config-grammar coverage — **Open**
Full grammar (Convolution, Copy, Delay, expressions, device/channel selectors, If/Eval) or the parametric-EQ subset first? Round-tripping *unknown* directives without loss is the likely requirement either way (never destroy what we don't understand).

## Q-10 · Community features — **Open**
Preset sharing, published ABX results ("N users failed to distinguish these two targets"), profiles synced across machines? In scope at all?

## Q-11 · Clip-battery extraction — **Resolved → ADR-0004**
User-curated in a dedicated waveform/spectrogram view; personal per-track libraries with tags and genre organization. Auto-extraction deferred to a future suggestion assistant at most.

## Q-12 · The Fingerprint Lab: capture, library, bridging — **Open (vision clarified 2026-08-22)**
Its own tab, visibly its own setup. **Capture:** a guided "capture headphone fingerprint" flow — sweeps + multiple reseatings per headphone, measured at *your* ears, so each fingerprint is the coupled HpTF∘ear response (HpTF and HRTF are not separable here, and don't need to be: within one person's library the ear factor cancels when comparing two fingerprints). **Library:** fingerprints accumulate per headphone; exportable/importable, so reviewers (headphones.com, The Headphone Show) can publish libraries. **Bridging:** given a shared headphone between two libraries, transfer other fingerprints across — (X_on_reviewer − shared_on_reviewer) applied to shared_on_you ≈ X for you — enabling at-home virtual auditioning of "basically any headphone," trustworthy mainly below ~5–6 kHz. Open: sweep method details, reseating protocol and acceptance stats (TB-14), regularization/boost caps (TB-16), and what "statistically identical" means as a per-band tolerance.

## Q-13 · Binaural-recording playback as its own feature? — **Resolved (2026-08-22, clarification — no ADR needed)**
It was the matching stack all along: "the binaural thing was the matching thing." HRTF in Fletcher lives entirely inside the fingerprint coupling (Q-12). No separate binaural-playback feature.

## Q-15 · Fingerprint interchange format and bridging math — **Open**
What a fingerprint file contains beyond the response: rig/mic type, smoothing, number of reseats, per-band variance, capture-app version — the metadata that decides whether two libraries may be bridged at all (TB-19). Bridging computation: chain length limits (A→B→C compounds error), per-frequency confidence weighting, and how confidence is displayed so sub-5–6 kHz trust and treble skepticism are visible, not implied (TB-18).

## Q-16 · Loudness-compensation volume anchoring — **Open (calibration flow settled 2026-08-23)**
Settled UX: **Settings hosts a reference-level calibration** — Fletcher plays noise (from the shared signal generator, the same code that serves Clip Studio and the mic-free hearing test; one generator, never duplicated), the user adjusts their volume to a comfortable reference, hits Accept, and that becomes the anchor. Requires the in-app playback engine (Phase 3); the flow is committed now.
The mode needs to know listening level, and an external amp's analog knob is invisible. Design sketch: track digital volume (Windows endpoint volume; many USB DACs sync their knob to it over USB HID — per-device capability detection needed) **relative to a user-set anchor** ("this is my normal level" at a known digital volume). Works fully when volume is ridden digitally; if the user turns the amp knob, the anchor silently breaks (TB-21) — detect what we can, communicate honestly. Optional upgrade: one-time absolute SPL calibration with a mic or phone SPL app to anchor in real dB. Also open: which contour family (ISO 226:2023) and how aggressively to apply the delta.

## Q-17 · Preset-history tree — **v2 shipped 2026-08-23; one residual**
Shipped: undo *graph* with gesture-completion nodes (drop / Enter / wheel settled), branching on post-undo edits, family-tree canvas panel (cursor-anchored zoom, drag pan, armed subtree deletes), **the app's first pop-out window** (live-synced both ways over events), **persistence** (per-preset trees in `history/`, auto-saved, resumed with divergence nodes), and **export/import** as shareable files. Residual: wiring "blind A/B any two tree nodes" into the Lab.

## Q-18 · Moment isolation and spectral similarity — **Open**
Raised 2026-08-22 mid-development. (a) Isolation UX: band-solo/band-sweep while looping a moment ("what frequencies are the thing I love here?") — essentially an interactive band-pass explorer over a clip. (b) Similarity: what spectral signature to extract (band-energy profile? MFCC-like? spectral centroid/tilt over the moment) and how to search the user's clip library / tracks for matches. (c) The MCP agent hook builds on both — API surface deferred to the ecosystem phase.

## Q-19 · Mic-free hearing profile: procedure design — **Open**
Raised 2026-08-22. The user's sketch: match loudness of adjacent noise bands (0–5k vs 5k–10k), recursively subdivide (0–2.5k vs 2.5k–5k, …), overlapping bands to isolate narrow regions, all statistics-driven yes/no answers. Design questions: (a) adaptive procedure — standard psychoacoustics is 2AFC with a transformed staircase (e.g. 2-down-1-up → 70.7% convergence), which beats fixed-step yes/no on trials-to-convergence; (b) band spacing — log/ERB (critical bands) rather than linear halves; (c) overlapping-band isolation propagates error — direct narrow-band trials with adaptive tracking may beat subtraction; (d) **target semantics**: equal-loudness across bands is NOT flat-preference — normal hearing has the ISO 226 contour shape built in, so the profile must be interpreted *relative to population-average contours*, not flattened absolutely (TB-23); (e) level anchoring — results are only valid at the test loudness (Fletcher–Munson again), so the session must fix and record its level; (f) fatigue management — session chunking, resume, convergence display.

## Q-20 · Pop-out windows with synced state — **v1 shipped (history); platform law learned**
The history tree is the first pop-out: shared component, main window owns truth, satellites send commands over targeted events (`emitTo`), live-synced both ways. **Platform law (2026-08-23, hard-won):** runtime-created WebView2 windows do NOT deliver DOM keyboard events to the page — capture-phase listeners, focus grabbing, all dead ends. Any pop-out needing keys uses **focus-scoped OS shortcuts** (register on window focus, unregister on blur/close, fire in Rust, forward as events). This is the pattern for Clip Studio's J/K/L, the Lab's A/B/X, and every future room.
Raised 2026-08-22 during design: any tab can pop out into its own OS window (curve editor on one monitor, Listening Lab on another), and everything updates together in real time. Architecturally cheap for us: the Rust core is already the single source of truth and windows are views (ADR-0001/0005) — Tauri supports multiple windows natively; core broadcasts state changes to every window. Open: which tabs earn pop-out first, window-state persistence, and how the A/B bar behaves across windows (in every window? only the focused one?).

## Q-21 · The track/clip flow: what happens after "Load a track"? — **Open**
"Load a track" on the A/B bar is a doorway, but the destination isn't designed yet: where do you switch between clips, search/browse the clip library, pick loop points mid-listen? Likely answer: loading a track lands you in the Listening Lab with the track's clip library docked (Clip Studio one click/pop-out away), and the A/B bar grows track-transport controls while a track session is active. Resolve by designing the Listening Lab + Clip Studio artboards — the next design work.

## Q-22 · Mini clip picker on the EQ tab — **Open (deferred until real use)**
User idea 2026-08-22: the EQ tab's spare space above the A/B bar could host a compact track/clip picker so you can flip test material without leaving the curve. Explicitly gated on dogfooding ("we'd have to really use it first") — decide after living with the app, not on the mockup.

## Q-23 · Rich preset/profile search and management — **Open (deliberately deferred)**
User direction 2026-08-22: presets start simple (list, switch, duplicate-from-anything, delete) but will eventually deserve a full first-class subsystem — rich search, organization, metadata — on the product principle that **anything non-trivial gets built out into its own complete "software within the software"** (as Clip Studio and the Lab are). Filed to keep v1 lean without forgetting the ambition. Relates: Q-17 (history tree), per-device profiles.

## Q-24 · The history inspector — **Open (designed 2026-08-23, build pending)**
A right-side inspector in the history view; clicking a node fills it. Candidate feature set (user + discussion):
- **Diff vs. parent by default**; a compare mode picks any node (cross-branch) as base.
- **Parametric diff**: added/removed filters, Fc moves, gain/Q deltas, type changes, preamp delta — boost/cut colored.
- **Spectral diff**: mini overlay of both response curves + the **difference curve** (node − base in dB, boost/cut shaded) — the sonic change as one picture.
- **Listen node / listen base** (level-matched apply) and **"Blind test these two nodes"** — resolves Q-17's last residual; the inspector is the natural home for node-vs-node ABX.
- **Audibility estimate** from the difference curve ("max 0.4 dB near 2 kHz — likely inaudible; blind test to confirm") — a clearly-labeled heuristic that sets honest expectations, never a verdict.
- **Node utilities**: rename/annotate (gesture labels become intentions), pin/star, promote-to-preset, copy chain as APO text.
- **Edge weight on the canvas**: mean |spectral diff| per edge rendered as edge thickness — the tree's shape shows where the big sonic decisions happened.
Open: layout (fixed right panel vs. collapsible), compare-mode UX, audibility heuristic choice.

## Q-14 · Tuning × testing fusion mechanics — **Open**
"Move a slider and it goes through the clips to highlight specific stuff." How literally: does adjusting a bass filter auto-queue the bass-heavy clip? Replay the full battery on demand? A/B against the pre-tweak state per clip, with optional randomized voting? Define the interaction loop so tuning and testing share one engine.
