//! Works gallery — render one hero frame per pinned work (theme, line) so
//! every curated poem can be visually verified.
//!
//! Run with `--works-gallery`. Output goes to `<out_dir>/works/<theme>-<line>.png`.

use inkflow::phrase;
use inkflow::rhythm::{Beat, Phase};
use inkflow::scene::{self, Scene, BEATS_PER_LINE};
use inkflow::surface::Surface;

pub fn run(w: u32, h: u32, out_dir: &str) {
    let _ = std::fs::create_dir_all(format!("{out_dir}/works"));
    let (warmth, pulse) = (0.35_f32, 0.20_f32);

    for theme in 0..phrase::THEMES.len() {
        let mut surf = Surface::memory(w, h);
        let mut scene = Scene::new(w, h);
        // One frame per line of the pinned work, so the gallery stays
        // correct if a poem ever gains or loses lines.
        let n_lines = phrase::poem_group_line_indices(phrase::POEM_BY_THEME[theme]).len();
        for line in 0..n_lines {
            let beat_index = (line as u64) * BEATS_PER_LINE;
            scene.theme_idx = theme;
            scene
                .composition
                .on_hero_beat(beat_index, 1, &phrase::PHRASES[0], theme);
            let hero = scene.composition.slots[scene.composition.hero_idx].phrase;
            let beat = Beat {
                index: beat_index,
                phase: Phase::Hold,
                t_in_phase: 1.0,
                t_in_beat: 2.0,
                phrase: hero,
                enter: 0.55,
                hold: 2.65,
                exit: 0.8,
                rest: 1.5,
            };
            scene::paint_background(&mut surf.pixels, w, h, &scene, pulse, warmth);
            scene::paint_composition(
                &mut surf.pixels,
                w,
                h,
                &scene,
                Some(&beat),
                warmth,
                pulse,
                10.0,
            );
            let path = format!("{out_dir}/works/{theme}-{line}.png");
            if let Err(e) = surf.write_png(&path) {
                eprintln!("inkflow: write {path}: {e}");
            } else {
                eprintln!("inkflow: wrote {path} hero={:?}", hero.text);
            }
        }
    }
}
