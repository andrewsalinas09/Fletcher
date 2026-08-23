# ADR-0007: Fletcher wires in as one Include line, owning only its own file

- **Status:** Accepted
- **Date:** 2026-08-22
- **Source:** Development-kickoff decisions (user selection); validated against the live Peace install on the dev machine

## Context
Equalizer APO reads `config.txt`; existing frontends already write there (Peace's convention, observed live: `config.txt` contains only `Include: peace.txt`, with a `configbeforePeace.txt` backup). Fletcher must coexist with that world (TB-02) and be trivially removable.

## Decision
Fletcher adds a single `Include: fletcher.txt` line to `config.txt` and writes all of its output only to `fletcher.txt` (and files it includes from there). It never modifies or reorders lines it does not own; it backs up `config.txt` before first touch; uninstall = remove one line and its files.

## Why
Same pattern Peace uses, so the ecosystem's conventions compose — Peace and Fletcher can literally coexist in the same config during development and migration. Smallest possible blast radius; the "never destroy what we don't understand" requirement (Q-09) falls out naturally. Rejected: owning `config.txt` outright (clobbers coexistence, raises TB-02 stakes for zero functional gain).

## Consequences
- A/B and bypass are implemented inside `fletcher.txt` (or by commenting Fletcher's include), never by rewriting the user's other lines.
- Fletcher must handle `config.txt` states it didn't create (Peace line present/absent, hand-written directives) — parse, preserve, append.
- During dev on this machine: Peace's include stays; the user toggles Peace's EQ off when testing Fletcher's output to avoid stacked EQ.
