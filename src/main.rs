// inkflow · main.rs
//
// Thin orchestrator: open a Surface, start the touch supervisor and
// the LLM worker, run the frame loop. All real logic lives in the
// sibling modules (renderer, scene_anim, mood, llm_loop, fallback,
// telemetry, surface, screenshot). This file is intentionally short.
//
// Per-frame body (in `loop {}`):
//   1. read touch state → FrameMood via `mood::tick`
//   2. publish mood into the LLM worker so its next prompt is fresh
//   3. spawn glyphs + particles via `scene_anim::spawn_for_frame` (now clustered around ink_current_x(t))
//   4. compute hue, draw the frame via `renderer::draw_frame`
//   5. emit telemetry every 10 s, snapshot screen every 60 s
//   6. sleep until the next monotonic deadline, skip-ahead if late
//
// See ZERO_DEP.md for the binding spec and PRODUCTION.md for evidence.

mod drm;
mod evdev;
mod fallback;
mod font;
mod fontdata;
mod llm_loop;
mod mood;
mod net_ollama;
mod phrases_raw;
mod poetry;
mod renderer;
mod scene;
mod scene_anim;
mod screenshot;
mod surface;
mod sys;
mod telemetry;

use crate::evdev::TouchState;
use crate::scene::Scene;
use crate::surface::Surface;
use crate::sys::wall_s;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Target frame time at 60 Hz. The cadence is sleep-until-monotonic-
/// deadline, not relative-to-now, so jitter doesn't accumulate and
/// fps locks to 60 instead of drifting to ~58.
const FRAME_TARGET: Duration = Duration::from_micros(16_667);

/// Telemetry tick (10 s) and screenshot cadence (60 s).
const TELEMETRY_PERIOD: Duration = Duration::from_secs(10);
const SCREENSHOT_PERIOD: Duration = Duration::from_secs(60);

// ---------- arg parsing / subcommands ----------

fn parse_args() -> Vec<String> {
    std::env::args().collect()
}

fn run_diag(_args: &[String]) -> ! {
    diag::run_diag()
}

fn run_drm_test(args: &[String]) -> ! {
    diag::run_drm_test(args)
}

/// Run the voice-drift self-check on the most recent telemetry
/// file and exit. Used by the autoloop maintainer (and by humans
/// inspecting the piece) to read the curatorial-voice
/// distribution without going through cargo test. Exit code:
///   - 0  healthy (no voice > 70 % of the recent window)
///   - 1  stuck (one voice > 70 %)
///   - 2  could not read the file
fn run_voice_drift(_args: &[String]) -> ! {
    let state_dir = std::env::var("INKFLOW_STATE_DIR").unwrap_or_else(|_| "state".into());
    let path = format!("{state_dir}/telemetry.jsonl");
    match telemetry::voice_drift_check(&path) {
        Ok(r) => {
            eprintln!("voice_drift: total={} last_voice={}", r.total, r.last_voice);
            for (name, count, pct) in &r.per_voice {
                eprintln!("  {:<6}  {:>3}  ({:>2}%)", name, count, pct);
            }
            if r.is_stuck() {
                eprintln!("status: STUCK (> 70 % single voice)");
                std::process::exit(1);
            } else {
                eprintln!("status: healthy");
                std::process::exit(0);
            }
        }
        Err(e) => {
            eprintln!("voice_drift: {e}");
            std::process::exit(2);
        }
    }
}

// ---------- main ----------

fn main() {
    let args = parse_args();
    if args.iter().any(|a| a == "--diag") {
        run_diag(&args);
    }
    if args.iter().any(|a| a == "--drm-test") {
        run_drm_test(&args);
    }
    if args.iter().any(|a| a == "--voice-drift") {
        run_voice_drift(&args);
    }

    let state_dir = std::env::var("INKFLOW_STATE_DIR").unwrap_or_else(|_| "state".into());
    std::fs::create_dir_all(&state_dir).ok();
    let tel_path = format!("{state_dir}/telemetry.jsonl");
    let shot_path = format!("{state_dir}/screen.png");
    let ppm_path = format!("{state_dir}/screen.ppm");

    // 1. open the display — DRM/KMS dumb-buffer, /dev/fb0, or headless
    let mut display = Surface::open();
    let fb_w = display.w();
    let fb_h = display.h();

    // 2. start the touch supervisor (inotify on /dev/input + reader threads)
    let touch = Arc::new(Mutex::new(TouchState::default()));
    evdev::start_supervisor(touch.clone());

    // 3. start the LLM worker
    let shared = Arc::new(Mutex::new(llm_loop::Shared::new()));
    let model = std::env::var("INKFLOW_MODEL").unwrap_or_else(|_| "gemma3:1b".into());
    let llm_client = net_ollama::OllamaClient::new(&model);
    let mood_state = Arc::new(Mutex::new((0.5f32, 0.5f32)));
    {
        let shared = shared.clone();
        let mood_state = mood_state.clone();
        std::thread::spawn(move || llm_loop::run_worker(llm_client, shared, mood_state));
    }

    // 4. allocate the scene (pre-sized object pools) and frame-loop state
    let mut scene = Scene::new();
    scene.seed_stars(fb_w as f32, fb_h as f32);
    let mut spawn_acc = scene_anim::SpawnAccum::default();
    let mut poetry_cursor = poetry::PoetryCursor::new();
    let mut tick: u64 = 0;
    let mut last_tel = Instant::now();
    let mut last_shot = Instant::now();
    let mut fps_acc = 0.0f32;
    let mut fps_n = 0u32;
    let start = Instant::now();
    let mut prev_frame = start;
    let mut next_frame = start + FRAME_TARGET;
    let mut frame_min_us: u32 = u32::MAX;
    let mut frame_max_us: u32 = 0;

    loop {
        let frame_start = Instant::now();
        // raw cadence from previous frame start (µs, unclamped) — feeds
        // min/max tracking so we can see the actual jitter envelope
        // rather than the [1ms, 50ms] clamp that dt uses for motion.
        let raw_dt_us = frame_start.duration_since(prev_frame).as_micros() as u32;
        let dt = (raw_dt_us as f32 / 1_000_000.0).clamp(0.001, 0.05);
        prev_frame = frame_start;
        if raw_dt_us < frame_min_us {
            frame_min_us = raw_dt_us;
        }
        if raw_dt_us > frame_max_us {
            frame_max_us = raw_dt_us;
        }
        let t = start.elapsed().as_secs_f32();
        tick = tick.wrapping_add(1);

        // ----- mood read -----
        let frame = {
            let mut s = touch.lock().unwrap();
            mood::tick(&mut s, dt, t)
        };

        // publish mood into the LLM worker so the next prompt is fresh
        llm_loop::publish_mood(&mood_state, &frame);

        // ----- spawn glyphs + particles -----
        poetry_cursor.tick_breath(dt);
        if !poetry_cursor.is_breathing() {
            poetry_cursor.advance_after_silence();
        }
        scene_anim::spawn_for_frame(
            &mut scene,
            &mut spawn_acc,
            &mut poetry_cursor,
            &frame,
            &touch,
            &shared,
            fb_w,
            fb_h,
            tick,
            dt,
            t,
        );

        // ----- compute hue, draw the frame -----
        let hue = mood::hue_at(t, frame.warmth);
        renderer::draw_frame(
            &mut display,
            &mut scene.glyphs,
            &mut scene.particles,
            &scene.stars,
            dt,
            t,
            hue,
        );

        // ----- telemetry + frame grab -----
        fps_acc += 1.0 / dt;
        fps_n += 1;
        if last_tel.elapsed() >= TELEMETRY_PERIOD {
            last_tel = Instant::now();
            let avg_fps = if fps_n > 0 {
                fps_acc / fps_n as f32
            } else {
                0.0
            };
            fps_acc = 0.0;
            fps_n = 0;
            // Window-local frame_min_us must be either the real
            // minimum of the elapsed window or 0 if no frames
            // landed (defended against future refactors that might
            // forget to reset between ticks).
            let window_min_us = if frame_min_us == u32::MAX {
                0
            } else {
                frame_min_us
            };
            let window_max_us = frame_max_us;
            frame_min_us = u32::MAX;
            frame_max_us = 0;
            let dev_str = if frame.touch_device.is_empty() {
                "none"
            } else {
                frame.touch_device.as_str()
            };
            let (llm_ok, llm_tps, llm_last) = llm_loop::snapshot(&shared);
            // Aesthetic telemetry — what the piece is "saying" right
            // now, not just how it's running. Per ARTIFACT.md
            // 'telemetry is the work's visible breath': voice is
            // which of the five curatorial voices is currently in
            // play; ink_x is where the glyph column sits; hue is
            // where on the warm/cool wheel the palette currently
            // sits. The autoloop reads these to verify the
            // art-direction contract is being held.
            let voice = net_ollama::style_for(frame.warmth, frame.energy);
            let ink_x = scene_anim::ink_current_x(t);
            let hue = mood::hue_at(t, frame.warmth);
            telemetry::append(
                &tel_path,
                &telemetry::Telemetry {
                    ts: wall_s(),
                    fps: avg_fps,
                    warmth: frame.warmth,
                    energy: frame.energy,
                    contacts: frame.contacts,
                    touch_device: dev_str,
                    llm_ok,
                    llm_toks_per_s: llm_tps,
                    llm_model: &model,
                    llm_last: &llm_last,
                    glyphs: scene.glyphs.len(),
                    particles: scene.particles.len(),
                    voice,
                    ink_x,
                    hue,
                    frame_min_us: window_min_us,
                    frame_max_us: window_max_us,
                },
            );
        }
        if last_shot.elapsed() >= SCREENSHOT_PERIOD {
            last_shot = Instant::now();
            // Snapshot pixels into an owned Vec on the render thread
            // (one synchronous 4 MB copy per minute — bounded), then
            // hand everything else to the worker: PPM write + ffmpeg
            // PPM→PNG. ffmpeg previously ran synchronously here via
            // .status(), which blocked the render loop for 100-500 ms
            // every 60 s and violated the "single-frame cost < 2 ms"
            // production contract.
            let bytes: Vec<u32> = display.snapshot_owned();
            let pitch_px = display.pitch_px();
            screenshot::spawn_grab(
                bytes,
                fb_w,
                fb_h,
                pitch_px,
                ppm_path.clone(),
                shot_path.clone(),
            );
        }

        // Aligned cadence: sleep until the absolute deadline `next_frame`,
        // then advance it by exactly one frame. If work blew past the
        // budget, skip-ahead instead of catch-up — the deadline stays
        // monotonic and never silently slips behind wall-clock.
        let work_done = Instant::now();
        if work_done < next_frame {
            std::thread::sleep(next_frame - work_done);
            next_frame += FRAME_TARGET;
        } else {
            next_frame = work_done + FRAME_TARGET;
        }
    }
}

// ---------- diagnostic subcommands ----------
//
// Kept in their own submodule so the `--diag` and `--drm-test` paths
// don't pollute `main()` with print-heavy one-shot code.

mod diag {
    use crate::drm;
    use crate::font::{draw_glyph, fill_circle, fill_rect, Rgba};

    pub fn run_diag() -> ! {
        let mut found: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(crate::evdev::open_input_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !name.starts_with("event") {
                    continue;
                }
                let path = p.to_string_lossy().to_string();
                if crate::evdev::is_touch_device(&path) {
                    found.push(path);
                }
            }
        }
        println!("touch_devices={}", found.join(" | "));
        let up = std::net::TcpStream::connect_timeout(
            &"127.0.0.1:11434".parse().unwrap(),
            std::time::Duration::from_millis(300),
        )
        .is_ok();
        println!("ollama_tcp_11434={up}");
        // Report the real driver for each card so --diag shows the
        // actual UAPI version the kernel returned (proves the ioctl
        // encoding is right, not just "no error").
        for n in 0..4 {
            match drm::probe(n) {
                Ok(r) => {
                    let conns: Vec<String> = r
                        .connected
                        .iter()
                        .map(|(id, ty, m)| format!("id={id} type={ty} modes={m}"))
                        .collect();
                    println!(
                        "drm[{}]={} v{}.{}.{} connected=[{}]",
                        r.path,
                        if r.driver.is_empty() { "?" } else { &r.driver },
                        r.major,
                        r.minor,
                        r.patch,
                        conns.join(",")
                    );
                }
                Err(e) => {
                    println!("drm[card{n}]=err:{e}");
                }
            }
        }
        std::process::exit(0);
    }

    /// One-shot modeset proof. Walks the full ioctl sequence on
    /// /dev/dri/card0 (or whichever card opens first), draws an
    /// unmistakable test pattern + a handful of CJK glyphs into the
    /// dumb buffer, holds for 5s so a human can read it, restores
    /// the original CRTC, captures the buffer to state/screen.png,
    /// and exits. Does not enter the persistent render loop.
    pub fn run_drm_test(_args: &[String]) -> ! {
        let state_dir = std::env::var("INKFLOW_STATE_DIR").unwrap_or_else(|_| "state".into());
        std::fs::create_dir_all(&state_dir).ok();
        let ppm_path = format!("{state_dir}/drm-test.ppm");
        let png_path = format!("{state_dir}/drm-test.png");

        let mut display = match drm::open_first() {
            Ok(d) => d,
            Err(e) => {
                // No real DRM — fall through to the dumb-only path so
                // we can still produce a screenshot.
                drm::log!("drm-test: open_first failed ({e}); trying open_dumb_only");
                match drm::open_dumb_only(1280, 800) {
                    Ok(d) => d,
                    Err(e2) => {
                        eprintln!("drm-test: no usable surface: {e2}");
                        std::process::exit(2);
                    }
                }
            }
        };

        let w = display.width as i32;
        let h = display.height as i32;
        let pitch_px = (display.pitch / 4) as usize;
        eprintln!(
            "drm-test: {}x{} pitch={} fb_id={} crtc_id={} modeset_ok={}",
            display.width,
            display.height,
            display.pitch,
            display.fb_id,
            display.crtc_id,
            display.modeset_ok
        );

        // ----- draw test pattern -----
        let pixels = display.pixels();
        let bg = Rgba(8, 6, 18, 255);
        for px in pixels.iter_mut() {
            *px = bg.0 as u32 | ((bg.1 as u32) << 8) | ((bg.2 as u32) << 16) | 0xFF000000;
        }
        let bars: [Rgba; 6] = [
            Rgba(255, 80, 60, 255),
            Rgba(255, 200, 60, 255),
            Rgba(90, 220, 90, 255),
            Rgba(80, 200, 255, 255),
            Rgba(120, 90, 240, 255),
            Rgba(240, 100, 220, 255),
        ];
        let bar_h = h / 6;
        for (i, &color) in bars.iter().enumerate() {
            fill_rect(
                pixels,
                pitch_px,
                w,
                h,
                0,
                (i as i32) * bar_h,
                w,
                bar_h,
                color,
                0.85,
            );
        }
        fill_circle(
            pixels,
            pitch_px,
            w,
            h,
            w as f32 * 0.5,
            h as f32 * 0.5,
            (w.min(h)) as f32 * 0.18,
            Rgba(255, 255, 255, 255),
            0.45,
        );
        let glyphs: [(&str, f32, f32, f32); 4] = [
            ("潮", w as f32 * 0.18, h as f32 * 0.82, h as f32 * 0.16),
            ("汐", w as f32 * 0.36, h as f32 * 0.82, h as f32 * 0.16),
            ("月", w as f32 * 0.58, h as f32 * 0.82, h as f32 * 0.16),
            ("光", w as f32 * 0.78, h as f32 * 0.82, h as f32 * 0.16),
        ];
        for (ch, x, y, sz) in glyphs {
            draw_glyph(
                pixels,
                pitch_px,
                w,
                h,
                x,
                y,
                sz,
                ch,
                Rgba(255, 255, 255, 255),
                1.0,
                0.0,
            );
        }
        draw_glyph(
            pixels,
            pitch_px,
            w,
            h,
            24.0,
            36.0,
            h as f32 * 0.06,
            "墨",
            Rgba(255, 255, 255, 255),
            0.9,
            0.0,
        );

        display.present();

        eprintln!("drm-test: frame drawn; holding 5s for visual inspection");
        std::thread::sleep(std::time::Duration::from_secs(5));

        let w_u32 = display.width;
        let h_u32 = display.height;
        let dump = display.pixels().to_vec();
        let bytes: Vec<u8> = dump
            .iter()
            .flat_map(|p| {
                [
                    (p & 0xFF) as u8,
                    ((p >> 8) & 0xFF) as u8,
                    ((p >> 16) & 0xFF) as u8,
                ]
            })
            .collect();
        let write_ppm = std::fs::File::create(&ppm_path).and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "P6\n{w_u32} {h_u32}\n255")?;
            f.write_all(&bytes)?;
            Ok(())
        });
        match write_ppm {
            Ok(_) => eprintln!("drm-test: wrote {ppm_path} ({}x{})", w_u32, h_u32),
            Err(e) => eprintln!("drm-test: ppm write failed: {e}"),
        }

        let ff = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-i", &ppm_path, &png_path])
            .status();
        match ff {
            Ok(s) if s.success() => eprintln!("drm-test: wrote {png_path}"),
            Ok(s) => eprintln!("drm-test: ffmpeg exit {s:?}; ppm retained as evidence"),
            Err(e) => eprintln!("drm-test: ffmpeg not available ({e}); ppm retained as evidence"),
        }

        display.restore();
        drop(display);

        eprintln!(
            "drm-test: done (modeset_ok={})",
            std::fs::metadata(&png_path).is_ok()
        );
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_includes_binary_name() {
        let a = parse_args();
        assert!(!a.is_empty());
    }

    #[test]
    fn frame_target_is_60hz() {
        let us = FRAME_TARGET.as_micros();
        assert_eq!(us, 16_667);
    }
}
