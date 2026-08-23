# ADR-0002: One UI for everyone — grey advanced features, never hide them

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Idea-mapping session (user: "standard is _identical_ but just slightly greyed and if you hover over it says why it's advanced and what it does"; confirmed "adr-0002")

## Context
Fletcher is "for everyone" — from laptop-speaker users to people measuring headphone transfer functions — but a single audience-neutral UI risks either overwhelming beginners or ghettoizing them in a dumbed-down mode with a cliff to expert mode.

## Decision
There is one layout. In standard mode it is *pixel-identical* to advanced mode, with advanced features slightly greyed out; hovering a greyed feature explains what it does and why it's advanced. Mode switching changes availability, never arrangement.

## Why
The disabled state becomes the documentation: users discover the advanced surface by seeing it, and graduate by curiosity rather than by finding a hidden settings toggle. Rejected: separate beginner/expert layouts (relearning cost at the cliff, double UI maintenance); hiding advanced features entirely (undiscoverable — the opposite of "a ton of features, most user friendly ever").

## Consequences
- Every advanced feature must ship with its hover explanation ("what it does + why it's advanced") as part of the feature's definition — not optional copy.
- UI components need a first-class "greyed + explainer" state.
- Feature specs carry a standard/advanced flag from day one.
