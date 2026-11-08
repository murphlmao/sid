# sid-term (vt100 terminal screen)

`vt100`-backed `sid_core::term::TerminalScreen`; the only crate that names `vt100`.

## Surface
`Vt100Screen::new(rows, cols)` implementing `TerminalScreen` (feed bytes, read a `TermCell` grid,
resize). Source: `src/screen.rs`.

## Invariants
- vt100 `Parser`/`Color`/`Cell` never escape; callers see only `TerminalScreen` + `TermCell`.
- Fidelity was A/B-verified against kitty (2026-07-06) — changes to cell/attribute mapping need
  a re-check on the SSH tab (`scripts/ab.sh`, `sid-cap.sh`).

## Test
`nice -n 10 cargo test -p sid-term -j 3`.

## Landmines
- Zoom reflows the grid (cells, not just fonts): the resize path must stay in sync with UI scale.
- Wide/combining characters and scrollback are vt100-version-sensitive — pin before bumping.
