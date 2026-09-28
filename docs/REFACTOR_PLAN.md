# 重构计划 (Refactoring plan)

Auto-generated. Review and refine by hand.

## Largest files (split candidates)

- `src/glyph_table.rs` — 474024 LOC
- `src/fontdata.rs` — 29027 LOC
- `src/phrases_raw.rs` — 28250 LOC
- `src/scene.rs` — 9940 LOC
- `src/drm.rs` — 859 LOC
- `src/scene_anim.rs` — 811 LOC
- `src/poetry.rs` — 657 LOC
- `src/main.rs` — 655 LOC
- `src/net_ollama.rs` — 641 LOC
- `src/renderer.rs` — 634 LOC
- `src/telemetry.rs` — 534 LOC

## TODO / FIXME / HACK density

- `TODO`: 0
- `FIXME`: 0
- `HACK`: 0
- `XXX`: 0
- `console.log`: 5

## Suggested next steps

- Pick the largest module and extract pure helpers into `lib/`.
- Convert the highest-density TODO cluster into issues.
- Add tests for any module with 0% test/sLOC ratio before further changes.
