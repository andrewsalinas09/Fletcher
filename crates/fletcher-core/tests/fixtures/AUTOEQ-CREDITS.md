# AutoEq fixtures

`autoeq-*.csv` are copied unchanged from the AutoEq project
(https://github.com/jaakkopasanen/AutoEq), MIT License, © 2018–2022 Jaakko Pasanen,
fetched 2026-11-04:

- `autoeq-harman-over-ear-2018.csv` and `autoeq-harman-over-ear-2018-without-bass.csv`: `targets/`
- `autoeq-oratory1990-hd650.csv`: `measurements/oratory1990/data/over-ear/Sennheiser HD 650.csv`,
  measured by oratory1990
- `autoeq-result-oratory1990-hd650.csv`: `results/oratory1990/over-ear/Sennheiser HD 650/Sennheiser HD 650.csv`,
  AutoEq's own output for that headphone and Harman over-ear 2018 (every step
  of its processing as columns)

They test `response.rs` against AutoEq's own grid, files and bass boost, and
`fit.rs` against AutoEq's processing and its published parametric EQ.
