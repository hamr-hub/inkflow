//! Headless capture harness — runs the production paint path for a fixed
//! number of steps and writes PNGs after each frame.
//!
//! `--headless`, `--layout-test`, and `--compose-test` differ only in naming,
//! step size, and where the scripted tap lands — they share this one loop
//! because the captured frames are the verification artefact.

use std::time::Instant;

use inkflow::phrase;
use inkflow::rhythm::Engine;
use inkflow::scene::{self, Scene};

/// One headless capture run configuration.
pub struct Capture {
    /// File-name prefix: `layout` → `state/layout-0.png`, …
    pub prefix: &'static str,
    /// Label used in the trailing density summary.
    pub label: &'static str,
    /// Fixed simulation step, so captures are reproducible across machines.
    pub dt: f32,
    pub frames: usize,
    /// Frame index that fires the scripted tap.
    pub touch_at: usize,
    /// Tap position as a fraction of the surface, plus its strength.
    pub touch: (f32, f32, f32),
    /// Prefix each slot line with a ✓/· visibility marker.
    pub marked_slots: bool,
}

pub fn render_frame(
    surf: &mut inkflow::surface::Surface,
    scene: &mut Scene,
    engine: &mut Engine,
    dt: f32,
) {
    let (w, h) = (surf.width, surf.height);
    let tempo = engine.current_tempo();
    let pulse = tempo.pulse;
    // Slow ambient breath — the warmth lifts and falls on the engine's
    // own ~15.7 s sine so the sky and halo warm and cool between phrases.
    let breath = 0.06 * (engine.time * 0.4).sin();
    let warmth = (tempo.warmth + breath).clamp(0.0, 1.0);

    // 1) advance rhythm first so we can detect a fresh beat.
    let beat = engine.advance(dt);
    let mut proposed_theme = scene.theme_idx;
    if let Some(b) = beat.as_ref() {
        if matches!(b.phase, inkflow::rhythm::Phase::Entrance) && b.t_in_phase < 0.05 {
            // A new beat just started — refresh one supporting slot in the
            // composition and possibly rotate the theme.
            proposed_theme = scene.composition.on_hero_beat(
                engine.beat_count.saturating_sub(1),
                scene.rng.next(),
                b.phrase,
                scene.theme_idx,
            );
        }
    }

    // 2) step simulation (dust, sparks, composition ages).
    scene.step(dt, warmth, pulse);
    scene.theme_idx = proposed_theme;

    // 3) paint background
    scene::paint_background(&mut surf.pixels, w, h, scene, pulse, warmth);

    // 4) paint the composition — supporting slots first, then hero.
    scene::paint_composition(
        &mut surf.pixels,
        w,
        h,
        scene,
        beat.as_ref(),
        warmth,
        pulse,
        scene.ambient_pulse,
    );
}

pub fn run_capture(w: u32, h: u32, cfg: &Capture, out_dir: &str) {
    let mut surf = inkflow::surface::Surface::memory(w, h);
    let mut scene = Scene::new(w, h);
    let mut engine = Engine::new();
    // Kick the engine so frame 0 already shows the entrance.
    engine.force_beat();

    let mut touched = false;
    let mut reported = false;
    for i in 0..cfg.frames {
        render_frame(&mut surf, &mut scene, &mut engine, cfg.dt);

        if i == cfg.touch_at && !touched {
            let (fx, fy, mag) = cfg.touch;
            scene.touch(w as f32 * fx, h as f32 * fy, mag);
            engine.tempo.touch(mag);
            touched = true;
        }

        let path = format!("{out_dir}/{}-{i}.png", cfg.prefix);
        if let Err(e) = surf.write_png(&path) {
            eprintln!("inkflow: write {path}: {e}");
        } else if !reported {
            eprintln!("inkflow: wrote {path}");
            reported = true;
        }
    }

    // Density summary — verifies every phrase is visible at the end of the run.
    let mut phrase_count = 0usize;
    let mut visible_chars = 0usize;
    for slot in &scene.composition.slots {
        let n = slot.phrase.text.chars().count();
        let a = slot.alpha_now();
        let visible = a > 0.04;
        if visible {
            visible_chars += n;
            phrase_count += 1;
        }
        let marker = match (cfg.marked_slots, visible) {
            (true, true) => "✓",
            (true, false) => "·",
            (false, _) => "",
        };
        eprintln!(
            "  {marker}slot[{:?}] alpha={:.2} chars={} phrase={:?}",
            slot.def.role, a, n, slot.phrase.text
        );
    }
    eprintln!(
        "inkflow: {} — {} frames, {} visible phrases ({} chars), theme={:?}",
        cfg.label,
        cfg.frames,
        phrase_count,
        visible_chars,
        phrase::THEME_NAMES[scene.theme_idx]
    );
}

pub fn run_headless(w: u32, h: u32, frames: usize, out_dir: &str, t0: Instant, started_ms: u128) {
    run_capture(
        w,
        h,
        &Capture {
            prefix: "rhythm",
            label: "headless",
            // 100 ms per step → 10 fps logical.
            dt: 0.10,
            frames,
            // During the hold of the first beat, so the tap registers on a
            // frame that already carries a full composition.
            touch_at: 2,
            touch: (0.5, 0.5, 0.8),
            marked_slots: false,
        },
        out_dir,
    );

    let elapsed = t0.elapsed();
    eprintln!(
        "inkflow: headless done — {} frames in {:.2}s ({:.1} fps logical) since {} ms",
        frames,
        elapsed.as_secs_f32(),
        frames as f32 / elapsed.as_secs_f32().max(0.001),
        started_ms,
    );
}
