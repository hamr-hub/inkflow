//! Headless harness + production renderer.
//!
//! Harness modes: `--headless`, `--layout-test`, `--compose-test`,
//! `--works-gallery`, and `--drm-test` (real display live loop).

use std::env;
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use inkflow::phrase;
use inkflow::rhythm::{Beat, Engine, Phase};
use inkflow::scene::{self, Scene, BEATS_PER_LINE};
use inkflow::surface::Surface;

const DEFAULT_W: u32 = 1280;
const DEFAULT_H: u32 = 720;
const DEFAULT_FRAMES: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Headless,
    LayoutTest,
    ComposeTest,
    WorksGallery,
    DrmTest,
    Help,
    /// A flag we don't recognise — print help *and* exit non-zero, so a typo in
    /// a systemd unit or a deploy script fails loudly instead of silently
    /// rendering a default headless run.
    BadFlag,
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
        } else if a == "--layout-test" {
            mode = Mode::LayoutTest;
        } else if a == "--compose-test" {
            mode = Mode::ComposeTest;
        } else if a == "--works-gallery" {
            mode = Mode::WorksGallery;
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
            mode = Mode::BadFlag;
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
  inkflow --works-gallery   render one hero frame per line for every complete
                            work into state/works/{{theme}}-{{line}}.png
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
    match mode {
        Mode::Help => {
            print_help();
            return ExitCode::SUCCESS;
        }
        Mode::BadFlag => {
            print_help();
            return ExitCode::from(2);
        }
        _ => {}
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
            run_capture(
                w,
                h,
                &Capture {
                    prefix: "layout",
                    label: "layout suite",
                    dt: 0.10,
                    frames: frames.max(1),
                    // Mid-sequence, so the accent is framed by settled content
                    // on both sides.
                    touch_at: frames / 2,
                    touch: (0.5, 0.55, 0.9),
                    marked_slots: false,
                },
                &out_dir,
            );
            ExitCode::SUCCESS
        }
        Mode::ComposeTest => {
            let _ = std::fs::create_dir_all(&out_dir);
            let frames = frames.max(1);
            run_capture(
                w,
                h,
                &Capture {
                    prefix: "compose",
                    label: "compose suite",
                    // Slower dt so entrance + hold + touch + refresh all land
                    // inside one captured run. 0.4 s/step × 12 frames ≈ 4.8 s,
                    // covering one hero beat plus a second entrance.
                    dt: 0.40,
                    frames,
                    // Near the end, so the touch frame is meaningful.
                    touch_at: frames.saturating_sub(3).max(2),
                    // Upper-right, where the supporting echo sits.
                    touch: (0.86, 0.18, 0.9),
                    marked_slots: true,
                },
                &out_dir,
            );
            ExitCode::SUCCESS
        }
        Mode::WorksGallery => {
            let _ = std::fs::create_dir_all(&out_dir);
            run_works_gallery(w, h, &out_dir);
            ExitCode::SUCCESS
        }
        Mode::DrmTest => match open_display(w, h) {
            Some(mut surf) => {
                run_live(&mut surf, t0);
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("inkflow: --drm-test could not open any display backend; falling back to headless (12 frames).");
                let _ = std::fs::create_dir_all(&out_dir);
                run_headless(w, h, 12, &out_dir, t0, started_ms);
                ExitCode::SUCCESS
            }
        },
        // Handled above, before the render paths are entered.
        Mode::Help | Mode::BadFlag => unreachable!(),
    }
}

/// Open the first display backend that works. `/dev/fb0` comes first: the
/// driver owns the modeset and scans out on every vsync with no DRM master,
/// so an unprivileged process can light the panel directly. Dumb buffers are
/// the fallback but need a modeset (DRM master) to be visible.
fn open_display(w: u32, h: u32) -> Option<inkflow::surface::Surface> {
    const BACKENDS: [&str; 3] = ["/dev/fb0", "/dev/dri/card0", "/dev/dri/card1"];
    for path in BACKENDS {
        match inkflow::surface::Surface::try_framebuffer(w, h, path) {
            Ok(s) => {
                eprintln!("inkflow: opened framebuffer at {path}");
                return Some(s);
            }
            Err(fb_err) => match inkflow::surface::Surface::try_drm_dumb(w, h, path) {
                Ok(s) => {
                    eprintln!("inkflow: opened drm dumb at {path}");
                    return Some(s);
                }
                Err(drm_err) => {
                    eprintln!("inkflow: {path} unavailable — fb: {fb_err}; drm dumb: {drm_err}");
                }
            },
        }
    }
    None
}

// ============================================================
// Shared render routine — used by both headless and live paths.
// ============================================================

fn render_frame(
    surf: &mut inkflow::surface::Surface,
    scene: &mut Scene,
    engine: &mut Engine,
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

/// One headless capture run. The three harness modes differ only in naming,
/// step size, and where the scripted tap lands — so they share one loop that
/// drives the SAME production paint path as the live renderer.
struct Capture {
    /// File-name prefix: `layout` → `state/layout-0.png`, …
    prefix: &'static str,
    /// Label used in the trailing density summary.
    label: &'static str,
    /// Fixed simulation step, so captures are reproducible across machines.
    dt: f32,
    frames: usize,
    /// Frame index that fires the scripted tap.
    touch_at: usize,
    /// Tap position as a fraction of the surface, plus its strength.
    touch: (f32, f32, f32),
    /// Prefix each slot line with a ✓/· visibility marker.
    marked_slots: bool,
}

fn run_capture(w: u32, h: u32, cfg: &Capture, out_dir: &str) {
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

fn run_headless(w: u32, h: u32, frames: usize, out_dir: &str, t0: Instant, started_ms: u128) {
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

// ============================================================
// Live renderer (DRM dumb / fb0)
// ============================================================

/// Deterministically render the hero frame of every line, for every complete
/// work — one image per (theme, line) so every curated poem can be verified.
fn run_works_gallery(w: u32, h: u32, out_dir: &str) {
    let _ = std::fs::create_dir_all(format!("{out_dir}/works"));
    let (warmth, pulse) = (0.35_f32, 0.20_f32);

    for theme in 0..phrase::THEMES.len() {
        let mut surf = Surface::memory(w, h);
        let mut scene = Scene::new(w, h);
        // One frame per line of the pinned work, so the gallery stays correct
        // if a poem ever gains or loses lines.
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

fn run_live(surf: &mut inkflow::surface::Surface, t0: Instant) {
    let mut scene = Scene::new(surf.width, surf.height);
    let mut engine = Engine::new();
    // Populate the full composition on the first frame (same as the layout
    // verification). Without this the piece opens on an empty composition
    // and waits for the first natural beat before any phrase appears.
    engine.force_beat();
    let mut last = Instant::now();
    let mut next_simulated_touch_at: f32 = 6.0;

    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.05); // clamp to 50ms
        last = now;
        render_frame(surf, &mut scene, &mut engine, dt);

        // Best-effort simulated touch every ~6 seconds (real touch would come
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
        // The fb0/DRM scanout paces us, but a memory-mapped surface does not
        // block — sleep to keep this from spinning a core flat out.
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}
