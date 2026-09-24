//! Headless harness + production renderer.
//!
//! Three modes:
//!   * `--headless [N]`    — render N frames into state/rhythm-N.png using the
//!     *same* paint path as the live renderer.
//!   * `--layout-test [N]` — render N frames into state/layout-N.png capturing
//!     the full composition (initial populate, several staggered refreshes,
//!     touch accent) so density/legibility can be inspected by vision.
//!   * `--gfx2-test [N]`   — render N frames into state/gfx2-N.png on a flat
//!     dark surface, using the SAME `glyph::draw_phrase` entry that the live
//!     renderer uses, so the captured frames are faithful glyph-quality
//!     previews.  Useful when iterating on the font atlas / renderer.
//!   * `--drm-test`        — try `/dev/dri/card0`, `/dev/dri/card1`, or
//!     `/dev/fb0` in turn; if one opens, run the live loop until killed.
//!     Headless harness skips this. (All modes share `Scene`.)
//!   * `--width=W --height=H` — render size (default 1280x720).

use std::env;
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use inkflow::color;
use inkflow::glyph::{self, Bucket, Point};
use inkflow::phrase;
use inkflow::rhythm::{Engine, Phase};
use inkflow::scene::{self, Scene};
use inkflow::surface::Surface;

const DEFAULT_W: u32 = 1280;
const DEFAULT_H: u32 = 720;
const DEFAULT_FRAMES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Headless,
    LayoutTest,
    ComposeTest,
    Gfx2Test,
    PoemTest,
    DrmTest,
    Help,
}

fn parse_args() -> (Mode, u32, u32, usize, String) {
    let mut mode = Mode::Headless;
    let mut w = DEFAULT_W;
    let mut h = DEFAULT_H;
    let mut frames = DEFAULT_FRAMES;
    let mut out_dir = String::from("state");
    for a in env::args().skip(1) {
        if a == "--drm-test" {
            mode = Mode::DrmTest;
        } else if a == "--gfx2-test" {
            mode = Mode::Gfx2Test;
        } else if a == "--poem-test" {
            mode = Mode::PoemTest;
        } else if a == "--layout-test" {
            mode = Mode::LayoutTest;
        } else if a == "--compose-test" {
            mode = Mode::ComposeTest;
        } else if a == "--help" || a == "-h" {
            mode = Mode::Help;
        } else if a == "--headless" {
            mode = Mode::Headless;
        } else if let Some(v) = a.strip_prefix("--headless=") {
            mode = Mode::Headless;
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--layout-test=") {
            mode = Mode::LayoutTest;
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--compose-test=") {
            mode = Mode::ComposeTest;
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--gfx2-test=") {
            mode = Mode::Gfx2Test;
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--frames=") {
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--width=") {
            if let Ok(n) = v.parse::<u32>() {
                w = n;
            }
        } else if let Some(v) = a.strip_prefix("--height=") {
            if let Ok(n) = v.parse::<u32>() {
                h = n;
            }
        } else if let Some(v) = a.strip_prefix("--out=") {
            out_dir = v.to_string();
        } else if a.starts_with("--") {
            eprintln!("inkflow: unknown flag: {a}");
            mode = Mode::Help;
        }
    }
    (mode, w, h, frames, out_dir)
}

fn print_help() {
    println!(
        "inkflow — poetic phrases on a beat (zero-dep Rust, DRM/fb0/headless)

Usage:
  inkflow --headless[=N]    render N frames to state/rhythm-{{N}}.png (default 12)
  inkflow --layout-test[=N] render N frames to state/layout-{{N}}.png — full
                            composition with staggered refreshes (default 24)
  inkflow --compose-test[=N] render N frames to state/compose-{{N}}.png — clean
                            multi-phrase composition (hero + 3 supporting);
                            default 12
  inkflow --gfx2-test[=N]   render N frames to state/gfx2-{{N}}.png on a flat
                            dark surface, via the same glyph::draw_phrase entry
  inkflow --drm-test        run the live renderer against the real display stack
  inkflow --width=W --height=H  output resolution (default 1280x720)
  inkflow --out=DIR         output directory for headless frames
  inkflow --help            show this help

The headless harness uses the SAME paint path as the live renderer so the
captured PNGs are faithful previews of what the live device shows.",
    );
}

fn main() -> ExitCode {
    let (mode, w, h, frames, out_dir) = parse_args();
    if mode == Mode::Help {
        print_help();
        return ExitCode::SUCCESS;
    }

    // Wall-clock anchor for the telemetry log.
    let t0 = Instant::now();
    let started_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    eprintln!(
        "inkflow: mode={:?} size={}x{} frames={} out={}",
        mode, w, h, frames, out_dir
    );

    match mode {
        Mode::Headless => {
            let _ = std::fs::create_dir_all(&out_dir);
            run_headless(w, h, frames, &out_dir, t0, started_ms);
            ExitCode::SUCCESS
        }
        Mode::LayoutTest => {
            let _ = std::fs::create_dir_all(&out_dir);
            run_layout_test(w, h, frames.max(1), &out_dir);
            ExitCode::SUCCESS
        }
        Mode::ComposeTest => {
            let _ = std::fs::create_dir_all(&out_dir);
            run_compose_test(w, h, frames.max(1), &out_dir);
            ExitCode::SUCCESS
        }
        Mode::Gfx2Test => {
            let _ = std::fs::create_dir_all(&out_dir);
            run_gfx2_test(w, h, frames.max(1), &out_dir);
            ExitCode::SUCCESS
        }
        Mode::PoemTest => {
            let _ = std::fs::create_dir_all(&out_dir);
            run_poem_test(w, h, &out_dir);
            ExitCode::SUCCESS
        }
        Mode::DrmTest => {
            // /dev/fb0 first: the driver owns the modeset and scans out
            // on every vsync with no DRM master, so an unprivileged user
            // process can light the panel directly. Dumb buffers are the
            // fallback but need a modeset (DRM master) to be visible.
            let backends: [&str; 3] = ["/dev/fb0", "/dev/dri/card0", "/dev/dri/card1"];
            let mut surface = None;
            for path in &backends {
                if let Ok(s) = inkflow::surface::Surface::try_framebuffer(w, h, path) {
                    eprintln!("inkflow: opened framebuffer at {path}");
                    surface = Some(s);
                    break;
                }
                if let Ok(s) = inkflow::surface::Surface::try_drm_dumb(w, h, path) {
                    eprintln!("inkflow: opened drm dumb at {path}");
                    surface = Some(s);
                    break;
                }
            }
            let Some(mut surf) = surface else {
                eprintln!("inkflow: --drm-test could not open any display backend; falling back to headless (12 frames).");
                let _ = std::fs::create_dir_all(&out_dir);
                run_headless(w, h, 12, &out_dir, t0, started_ms);
                return ExitCode::SUCCESS;
            };
            run_live(&mut surf, t0, started_ms);
            ExitCode::SUCCESS
        }
        Mode::Help => unreachable!(),
    }
}

// ============================================================
// Shared render routine — used by both headless and live paths.
// ============================================================

fn render_frame(
    surf: &mut inkflow::surface::Surface,
    scene: &mut Scene,
    engine: &mut Engine,
    _beat_index_for_echo: u64,
    dt: f32,
) {
    let (w, h) = (surf.width, surf.height);
    let tempo = engine.current_tempo();
    let pulse = tempo.pulse;
    let warmth = tempo.warmth;

    // 1) advance rhythm first so we can detect a fresh beat.
    let beat = engine.advance(dt);
    let mut proposed_theme = scene.theme_idx;
    if let Some(b) = beat.as_ref() {
        if matches!(b.phase, Phase::Entrance) && b.t_in_phase < 0.05 {
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

// ============================================================
// Glyph quality harness (gfx2)
// ============================================================

/// Render a few curated sample phrases on a flat dark surface to inspect
/// glyph quality directly through the SAME `glyph::draw_phrase` entry that
/// the live renderer uses.  Output goes to `{out_dir}/gfx2-{i}.png`.
///
/// Frames:
///   0 — single short hero phrase centred
///   1 — long hero phrase centred
///   2 — hero + body stacked (different buckets in one frame)
///   3 — body-only multi-line strip showing the body bucket
/// Lay one complete ordered poem group out as a vertical reading column —
/// all four lines from one source, equally centred, over the same mist
/// background the live piece uses. This is the "one complete work, not a
/// themed collage" layout preview.
fn run_poem_test(w: u32, h: u32, out_dir: &str) {
    run_poem_test_impl(w, h, out_dir);
}

fn run_poem_test_impl(w: u32, h: u32, out_dir: &str) {
    let mut surf = Surface::memory(w, h);
    let scene = Scene::new(w, h);
    let warmth = 0.55_f32;
    let pulse = 0.30_f32;
    scene::paint_background(&mut surf.pixels, w, h, &scene, pulse, warmth);

    let line_indices = phrase::poem_group_line_indices(0);
    let texts: Vec<&str> = line_indices
        .iter()
        .map(|&i| phrase::PHRASES[i as usize].text)
        .collect();
    let n = texts.len() as i32;
    let em = glyph::BODY_EM_PX as i32;
    let line_step = (em as f32 * 1.62) as i32;
    let first_baseline = (h as i32) / 2 - (n - 1) * line_step / 2 + em / 6;
    let ink = color::ink::CREAM;
    let glow = color::ink::GLOW;

    for (i, text) in texts.iter().enumerate() {
        let baseline_y = first_baseline + i as i32 * line_step;
        let pen_x = center_pen_x(w, text, glyph::BODY_EM_PX);
        glyph::draw_phrase(
            &mut surf.pixels,
            w as usize,
            h as usize,
            text,
            Point::new(pen_x, baseline_y),
            Bucket::Body,
            glow,
            0.16,
        );
        glyph::draw_phrase(
            &mut surf.pixels,
            w as usize,
            h as usize,
            text,
            Point::new(pen_x, baseline_y),
            Bucket::Body,
            ink,
            1.0,
        );
    }

    let path = format!("{out_dir}/poem-0.png");
    match surf.write_png(&path) {
        Ok(_) => eprintln!("inkflow: wrote {path} ({} lines, one complete work)", n),
        Err(e) => eprintln!("inkflow: write {path}: {e}"),
    }
}

fn run_gfx2_test(w: u32, h: u32, frames: usize, out_dir: &str) {
    let bg = color::bg::DEEP; // rgb(4, 6, 14) — deep midnight
    let ink = color::ink::CREAM;
    let glow = color::ink::GLOW;

    for frame_idx in 0..frames {
        let mut surf = Surface::memory(w, h);
        for px in surf.pixels.iter_mut() {
            *px = bg;
        }

        match frame_idx % 5 {
            0 => {
                // Short hero phrase — biggest, smoothest.
                let phrase = "月色";
                let by = (h as i32) / 2 + glyph::HERO_EM_PX as i32 * 5 / 100;
                let pen_x = center_pen_x(w, phrase, glyph::HERO_EM_PX);
                glyph::draw_phrase(
                    &mut surf.pixels,
                    w as usize,
                    h as usize,
                    phrase,
                    Point::new(pen_x, by),
                    Bucket::Hero,
                    ink,
                    1.0,
                );
                // Faint halo behind — same path, lower alpha, glow colour.
                glyph::draw_phrase(
                    &mut surf.pixels,
                    w as usize,
                    h as usize,
                    phrase,
                    Point::new(pen_x, by),
                    Bucket::Hero,
                    glow,
                    0.20,
                );
            }
            1 => {
                // Long hero phrase — fills more of the frame.
                let phrase = "万物静默如谜";
                let by = (h as i32) / 2 + glyph::HERO_EM_PX as i32 * 5 / 100;
                let pen_x = center_pen_x(w, phrase, glyph::HERO_EM_PX);
                glyph::draw_phrase(
                    &mut surf.pixels,
                    w as usize,
                    h as usize,
                    phrase,
                    Point::new(pen_x, by),
                    Bucket::Hero,
                    ink,
                    1.0,
                );
            }
            2 => {
                // Hero stacked over a smaller body line — both buckets in
                // one frame.
                let hero_phrase = "月色入海";
                let body_phrase = "此心光明";
                let hero_y = (h as i32) / 2 - glyph::HERO_EM_PX as i32 / 2;
                let body_y = hero_y + glyph::HERO_EM_PX as i32 + glyph::BODY_EM_PX as i32 / 2;
                let hpx = center_pen_x(w, hero_phrase, glyph::HERO_EM_PX);
                let bpx = center_pen_x(w, body_phrase, glyph::BODY_EM_PX);
                glyph::draw_phrase(
                    &mut surf.pixels,
                    w as usize,
                    h as usize,
                    hero_phrase,
                    Point::new(hpx, hero_y),
                    Bucket::Hero,
                    ink,
                    1.0,
                );
                glyph::draw_phrase(
                    &mut surf.pixels,
                    w as usize,
                    h as usize,
                    body_phrase,
                    Point::new(bpx, body_y),
                    Bucket::Body,
                    color::ink::SHADOW,
                    0.85,
                );
            }
            3 => {
                // DEBUG: render a few supporting-size (0.30 em) phrases at
                // fixed pixel positions to inspect glyph quality at the
                // small supporting scale.  Each char is ~38 px tall at
                // scale_q8=77; phrase is drawn char-by-char via draw_glyph.
                let phrases = ["松下问童子", "万物静默", "寒山", "静夜"];
                let scale_q8: u32 = 77; // 0.30 * 256
                let y_start = 100;
                for (i, ph) in phrases.iter().enumerate() {
                    let mut pen_x_q8 = 80 * 256;
                    let py = y_start + (i as i32) * 70;
                    let fy = py * 256;
                    for ch in ph.chars() {
                        let idx = glyph::index_for(ch as u32) as usize;
                        let adv = glyph::HERO_TABLE[idx].advance as i32;
                        glyph::draw_glyph(
                            &mut surf.pixels,
                            w as usize,
                            h as usize,
                            idx as u8,
                            ink,
                            ink,
                            pen_x_q8,
                            fy,
                            scale_q8,
                            1.0,
                        );
                        pen_x_q8 += adv * (scale_q8 as i32);
                    }
                }
            }
            _ => {
                // Body-only strip — show several body-sized phrases vertically.
                let phrases = ["春去秋来", "山高月小", "水落石出"];
                let body_em = glyph::BODY_EM_PX as i32;
                let row_h = body_em * 16 / 10;
                let total_h = row_h * (phrases.len() as i32);
                let y0 = (h as i32 - total_h) / 2 + body_em;
                for (i, ph) in phrases.iter().enumerate() {
                    let pen_x = center_pen_x(w, ph, glyph::BODY_EM_PX);
                    let by = y0 + (i as i32) * row_h;
                    glyph::draw_phrase(
                        &mut surf.pixels,
                        w as usize,
                        h as usize,
                        ph,
                        Point::new(pen_x, by),
                        Bucket::Body,
                        ink,
                        1.0,
                    );
                }
            }
        }

        let path = format!("{out_dir}/gfx2-{frame_idx}.png");
        if let Err(e) = surf.write_png(&path) {
            eprintln!("inkflow: write {path}: {e}");
        } else {
            eprintln!("inkflow: wrote {path}");
        }
    }
}

/// Centre the pen for a phrase whose glyphs have native em `em_px`.
#[inline]
fn center_pen_x(w: u32, phrase: &str, em_px: u16) -> i32 {
    let char_count = phrase.chars().count() as i32;
    let approx_w = char_count * (em_px as i32) * 106 / 100;
    ((w as i32 - approx_w) / 2).max(8)
}

// Silence the unused-import for `glyph` when only `--drm-test` runs.
#[allow(dead_code)]
fn _baseline_y_for(_: &str) -> i32 {
    0
}

fn run_headless(w: u32, h: u32, frames: usize, out_dir: &str, t0: Instant, started_ms: u128) {
    let mut surf = inkflow::surface::Surface::memory(w, h);
    let mut scene = Scene::new(w, h);
    let mut engine = Engine::new();
    // First beat starts immediately so frame 0 already shows the entrance.
    engine.force_beat();

    // Use a fixed dt so frames are reproducible across machines.
    let dt = 0.10_f32; // 100 ms per step → 10 fps logical

    // Schedule a touch tap during the hold of the first beat for a touch frame.
    let mut touched = false;

    for i in 0..frames {
        render_frame(&mut surf, &mut scene, &mut engine, 0, dt);

        // Mid-hold touch: schedule a tap on the second captured frame.
        if i == 2 && !touched {
            scene.touch(w as f32 / 2.0, h as f32 / 2.0, 0.8);
            engine.tempo.touch(0.8);
            touched = true;
        }

        let path = format!("{out_dir}/rhythm-{i}.png");
        if let Err(e) = surf.write_png(&path) {
            eprintln!("inkflow: write {}: {}", path, e);
        } else if i == 0 {
            eprintln!("inkflow: wrote {path}");
        }
    }

    let elapsed = t0.elapsed();
    eprintln!(
        "inkflow: headless done — {} frames in {:.2}s ({:.1} fps logical) since {} ms",
        frames,
        elapsed.as_secs_f32(),
        frames as f32 / elapsed.as_secs_f32().max(0.001),
        started_ms,
    );
}

/// Run a layout verification suite — render a longer sequence capturing the
/// initial populate, several staggered refreshes, and a touch accent. Output
/// goes to `state/layout-N.png`.
fn run_layout_test(w: u32, h: u32, frames: usize, out_dir: &str) {
    let mut surf = inkflow::surface::Surface::memory(w, h);
    let mut scene = Scene::new(w, h);
    let mut engine = Engine::new();
    engine.force_beat();
    let dt = 0.10_f32;

    let mut touched = false;
    let mut reported = false;
    for i in 0..frames {
        render_frame(&mut surf, &mut scene, &mut engine, 0, dt);

        // Mid-sequence touch — around the middle of the captured run.
        if i == frames / 2 && !touched {
            scene.touch(w as f32 * 0.5, h as f32 * 0.55, 0.9);
            engine.tempo.touch(0.9);
            touched = true;
        }

        let path = format!("{out_dir}/layout-{i}.png");
        if let Err(e) = surf.write_png(&path) {
            eprintln!("inkflow: write {}: {}", path, e);
        } else if !reported {
            eprintln!("inkflow: wrote {path}");
            reported = true;
        }
    }

    // Print a density summary so the report can quote it.
    let mut phrase_count = 0usize;
    let mut visible_chars = 0usize;
    for slot in &scene.composition.slots {
        let n = slot.phrase.text.chars().count();
        let a = slot.alpha_now();
        if a > 0.04 {
            visible_chars += n;
            phrase_count += 1;
        }
        eprintln!(
            "  slot[{:?}] alpha={:.2} phrase={:?}",
            slot.def.role, a, slot.phrase.text
        );
    }
    eprintln!(
        "inkflow: layout suite — {} frames, {} visible phrases ({} chars), theme={:?}",
        frames,
        phrase_count,
        visible_chars,
        phrase::THEME_NAMES[scene.theme_idx]
    );
}

/// Run a clean multi-phrase composition verification — render a sequence of
/// frames to `state/compose-N.png` capturing: the initial populate (every
/// slot fading in cleanly), a full composition hold, two refreshes after the
/// rhythm engine fires a new hero beat, and a touch accent.  Uses the SAME
/// production paint path as the live renderer (no separate preview path).
///
/// Default 12 frames at 0.40 s/step = ~4.8 s, covering one hero beat +
/// entrance of a second beat.  Tune with `--compose-test=N`.
fn run_compose_test(w: u32, h: u32, frames: usize, out_dir: &str) {
    let mut surf = inkflow::surface::Surface::memory(w, h);
    let mut scene = Scene::new(w, h);
    let mut engine = Engine::new();
    // Kick the engine — frame 0 already shows the entrance.
    engine.force_beat();
    // Slower dt so we can see the entrance + hold + touch + refresh across
    // the captured run.  Each step = 0.4 s.
    let dt = 0.40_f32;

    // Touch happens near the end so the captured touch frame is meaningful.
    let touch_frame = frames.saturating_sub(3).max(2);
    let mut touched = false;
    let mut reported = false;

    // Pre-touch: ensure we have a beat running so warmth/pulse ride along.
    for i in 0..frames {
        render_frame(&mut surf, &mut scene, &mut engine, 0, dt);

        if i == touch_frame && !touched {
            // Tap near the upper-right corner slot — accent it.
            let (tw, th) = (w as f32, h as f32);
            scene.touch(tw * 0.86, th * 0.18, 0.9);
            engine.tempo.touch(0.9);
            touched = true;
        }

        let path = format!("{out_dir}/compose-{i}.png");
        if let Err(e) = surf.write_png(&path) {
            eprintln!("inkflow: write {}: {}", path, e);
        } else if !reported {
            eprintln!("inkflow: wrote {path}");
            reported = true;
        }
    }

    // Density summary — verifies every phrase is fully visible at the end.
    let mut phrase_count = 0usize;
    let mut visible_chars = 0usize;
    for slot in &scene.composition.slots {
        let n = slot.phrase.text.chars().count();
        let a = slot.alpha_now();
        let marker = if a > 0.04 { "✓" } else { "·" };
        eprintln!(
            "  {marker} slot[{:?}] alpha={:.2} chars={} phrase={:?}",
            slot.def.role, a, n, slot.phrase.text
        );
        if a > 0.04 {
            visible_chars += n;
            phrase_count += 1;
        }
    }
    eprintln!(
        "inkflow: compose suite — {} frames, {} visible phrases ({} chars), theme={:?}",
        frames,
        phrase_count,
        visible_chars,
        phrase::THEME_NAMES[scene.theme_idx]
    );
}

// ============================================================
// Live renderer (DRM dumb / fb0)
// ============================================================

fn run_live(surf: &mut inkflow::surface::Surface, t0: Instant, started_ms: u128) {
    let mut scene = Scene::new(surf.width, surf.height);
    let mut engine = Engine::new();
    // Populate the full composition on the first frame (same as the layout
    // verification). Without this the piece opens on an empty composition
    // and waits for the first natural beat before any phrase appears.
    engine.force_beat();
    let mut last = Instant::now();
    // try to tap a touch device lazily — best effort, ignore failure.
    let _touch_path = "/dev/input/event0";
    let mut next_simulated_touch_at: f32 = 6.0;

    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05); // clamp to 50ms
        last = now;
        render_frame(surf, &mut scene, &mut engine, 0, dt);

        // Best-effort simulated touch every 6 seconds (real touch would come
        // from /dev/input via evdev; we keep zero-dep and skip parsing).
        let elapsed = t0.elapsed().as_secs_f32();
        if elapsed >= next_simulated_touch_at {
            let (w, h) = (surf.width, surf.height);
            scene.touch(w as f32 * 0.5, h as f32 * 0.6, 0.7);
            engine.tempo.touch(0.7);
            next_simulated_touch_at = elapsed + 5.0 + (elapsed.sin().abs() * 2.0);
        }

        if let Err(e) = surf.present() {
            eprintln!("inkflow: present failed: {e}");
            return;
        }
        // touch_path reserved for future evdev integration
        let _ = _touch_path;
        // avoid hot spin
        std::thread::sleep(std::time::Duration::from_millis(8));
        let _ = started_ms;
    }
}

// Suppress an unused-import warning for `glyph` from header exploration.
#[allow(dead_code)]
fn _suppress_unused() {
    let _ = glyph::bitmap_width();
}
