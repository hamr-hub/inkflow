//! Headless harness + production renderer.
//!
//! Three modes:
//!   * `--headless [N]`    — render N frames into state/rhythm-N.png using the
//!     *same* paint path as the live renderer.
//!   * `--gfx2-test [N]`   — render N frames into state/gfx2-N.png on a flat
//!     dark surface, using the SAME `glyph::draw_phrase` entry that the live
//!     renderer uses, so the captured frames are faithful glyph-quality
//!     previews.  Useful when iterating on the font atlas / renderer.
//!   * `--drm-test`        — try `/dev/dri/card0`, `/dev/dri/card1`, or
//!     `/dev/fb0` in turn; if one opens, run the live loop until killed.
//!     Headless harness skips this. (Both modes share `Scene`.)
//!   * `--width=W --height=H` — render size (default 1280x720).

use std::env;
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use inkflow::color;
use inkflow::glyph::{self, Bucket, Point};
use inkflow::phrase;
use inkflow::rhythm::{Engine, Phase};
use inkflow::scene::{self, Echo, Scene};
use inkflow::surface::Surface;

const DEFAULT_W: u32 = 1280;
const DEFAULT_H: u32 = 720;
const DEFAULT_FRAMES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Headless,
    Gfx2Test,
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
        } else if a == "--help" || a == "-h" {
            mode = Mode::Help;
        } else if a == "--headless" {
            mode = Mode::Headless;
        } else if let Some(v) = a.strip_prefix("--headless=") {
            mode = Mode::Headless;
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
  inkflow --headless[=N]   render N frames to state/rhythm-{{N}}.png (default 12)
  inkflow --gfx2-test[=N]  render N frames to state/gfx2-{{N}}.png on a flat dark
                           surface, via the same glyph::draw_phrase entry
  inkflow --drm-test       run the live renderer against the real display stack
  inkflow --width=W --height=H  output resolution (default 1280x720)
  inkflow --out=DIR        output directory for headless frames
  inkflow --help           show this help

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
        Mode::Gfx2Test => {
            let _ = std::fs::create_dir_all(&out_dir);
            run_gfx2_test(w, h, frames.max(1), &out_dir);
            ExitCode::SUCCESS
        }
        Mode::DrmTest => {
            // Try a backend in priority order: drm/card0, drm/card1, fb0.
            let backends: [&str; 3] = ["/dev/dri/card0", "/dev/dri/card1", "/dev/fb0"];
            let mut surface = None;
            for path in &backends {
                if let Ok(s) = inkflow::surface::Surface::try_drm_dumb(w, h, path) {
                    eprintln!("inkflow: opened drm dumb at {path}");
                    surface = Some(s);
                    break;
                }
                if let Ok(s) = inkflow::surface::Surface::try_framebuffer(w, h, path) {
                    eprintln!("inkflow: opened framebuffer at {path}");
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
    beat_index_for_echo: u64,
    dt: f32,
) {
    let (w, h) = (surf.width, surf.height);
    let tempo = engine.current_tempo();
    let pulse = tempo.pulse;
    let warmth = tempo.warmth;

    // 1) step simulation
    scene.step(dt, warmth, pulse);

    // 2) advance rhythm, get current beat (if any)
    let beat = engine.advance(dt);

    // 3) ensure echo fades in *after* the first beat starts.
    let echo = if beat.is_some() {
        scene.echo.as_ref()
    } else {
        None
    };

    // 4) paint background
    scene::paint_background(&mut surf.pixels, w, h, scene, pulse, warmth);

    // 5) paint echo (faint) behind the hero
    if let Some(e) = echo {
        scene::paint_echo(&mut surf.pixels, w, h, e, pulse);
    }

    // 6) paint hero phrase
    if let Some(b) = beat {
        scene::paint_phrase(&mut surf.pixels, w, h, &b, warmth, pulse, b.phrase.mood);
        // After Exit begins, snapshot the *current* (about-to-disappear) phrase
        // as the next echo so it lingers softly.
        if matches!(b.phase, Phase::Exit) && b.t_in_phase < 0.05 {
            let prev = phrase::phrase_for_beat(beat_index_for_echo);
            scene.echo = Some(Echo {
                phrase: prev,
                alpha: 0.55,
                dx: 6.0,
                dy: 4.0,
                scale: 0.96,
            });
        }
    } else {
        // fade echo when no beat is on screen.
        if let Some(e) = scene.echo.as_mut() {
            e.alpha -= dt * 0.45;
            if e.alpha <= 0.02 {
                scene.echo = None;
            }
        }
    }
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
fn run_gfx2_test(w: u32, h: u32, frames: usize, out_dir: &str) {
    let bg = color::bg::DEEP; // rgb(4, 6, 14) — deep midnight
    let ink = color::ink::CREAM;
    let glow = color::ink::GLOW;

    for frame_idx in 0..frames {
        let mut surf = Surface::memory(w, h);
        for px in surf.pixels.iter_mut() {
            *px = bg;
        }

        match frame_idx % 4 {
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
    let mut beat_idx_for_echo = 0u64;

    // Schedule a touch tap during the hold of the first beat for a touch frame.
    let mut touched = false;

    for i in 0..frames {
        render_frame(&mut surf, &mut scene, &mut engine, beat_idx_for_echo, dt);

        // Mid-hold touch: schedule a tap on the second captured frame.
        if i == 2 && !touched {
            scene.touch(w as f32 / 2.0, h as f32 / 2.0, 0.8);
            engine.tempo.touch(0.8);
            touched = true;
        }

        // Capture current beat index *after* the frame so the next frame's
        // echo points at the phrase that just exited.
        beat_idx_for_echo = engine.beat_count;

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

// ============================================================
// Live renderer (DRM dumb / fb0)
// ============================================================

fn run_live(surf: &mut inkflow::surface::Surface, t0: Instant, started_ms: u128) {
    let mut scene = Scene::new(surf.width, surf.height);
    let mut engine = Engine::new();
    let mut last = Instant::now();
    let mut beat_idx_for_echo = 0u64;
    // try to tap a touch device lazily — best effort, ignore failure.
    let _touch_path = "/dev/input/event0";
    let mut next_simulated_touch_at: f32 = 6.0;

    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05); // clamp to 50ms
        last = now;
        render_frame(surf, &mut scene, &mut engine, beat_idx_for_echo, dt);
        beat_idx_for_echo = engine.beat_count;

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
