# ADR-0010: The visual direction — Console, locked

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Design session, five iterations on the canvas (claude.ai/code/artifact/759d400e-…); user approved v5 EQ tab, Settings, Clip Studio v2 ("this is an excellent start. We can do this for now").

## Context
Two quick CSS passes read as generic AI slop. A real design exploration (three directions, real HD650 data) plus five reaction-driven iterations converged on a direction the user approves. Without locking it, every future screen re-litigates taste.

## Decision
Fletcher's UI speaks **Console**: light instrument aesthetic, lab-equipment-on-paper.

- **Palette:** paper `#f2efe9`, panel `#f9f7f2`, ink `#14171a`, dim `#8b8578`, hairlines `#e5dfd1`/`#c9c2b0`/`#b3ac9c`, ok-green `#3d7d43`. **Semantic law, app-wide: boost = orange `#c85a13` (`#e89a5f` on dark), cut = blue `#3f6d9e`.** Dark surfaces (`#14171a`) are reserved for scopes (spectrograms) and selection states.
- **Type:** IBM Plex Sans (UI) + IBM Plex Mono (data, labels, tabs), bundled locally.
- **Anatomy:** single-row header (wordmark · tabs · device chip); **persistent A/B bar** docked at the bottom of every tab (A/B pair, hotkey, level-match light, context actions — grows transport during track sessions); content in hairline-divided strips and labeled cells, never boxed card grids.
- **Component vocabulary:** arc-fill gain gauges (ink-amount, not needle angle); filter-type curve glyphs with name + caret (dropdown affordance); labeled inspector cells (tiny mono field labels over values); tooltips over side panels; NLE idiom in the Studio (in/out, markers, J·K·L); FFT pane shades EQ boost/cut regions.
- **Contracts:** bidirectional editing (graph ⇄ strip, one model); grey-don't-hide (ADR-0002) rendered as reduced opacity + hover explainer; every label traces to FEATURES.md vocabulary.

## Why
Chosen by the user across five iterations against real alternatives. Rejected: **Notebook** and **Swiss** directions (unchosen/killed on the canvas), dark-mode default, needle knobs (unglanceable), vertical filter rails and card grids (redundant with the graph, "messy"), floating action buttons (testing lives in the A/B bar), invented UI vocabulary (reads as hallucination).

## Consequences
- The app implements these as CSS tokens; `design/*.dc.html` artboards are the visual reference.
- New screens (Listening Lab, Fingerprints) are designed in this language — artboard first, then port.
- Tweaks are expected ("we'll have to tween and edit") but happen inside the language; changing the language itself means superseding this ADR.
