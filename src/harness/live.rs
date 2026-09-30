//! Live frame loop — runs against the real `/dev/fb0` or DRM card for the
//! `--drm-test` mode. Every real-deployed frame comes through this path.

use std::time::Instant;

use inkflow::rhythm::Engine;
use inkflow::scene::Scene;
use inkflow::surface::Surface;

use super::capture::render_frame;

/// Open the first display backend that works. `/dev/fb0` comes first: the
/// driver owns the modeset and scans out on every vsync with no DRM master,
/// so an unprivileged process can light the panel directly. Dumb buffers are
/// the fallback but need a modeset (DRM master) to be visible.
pub fn open_display(w: u32, h: u32) -> Option<Surface> {
    const BACKENDS: [&str; 3] = ["/dev/fb0", "/dev/dri/card0", "/dev/dri/card1"];
    for path in BACKENDS {
        match Surface::try_framebuffer(w, h, path) {
            Ok(s) => {
                eprintln!("inkflow: opened framebuffer at {path}");
                return Some(s);
            }
            Err(fb_err) => match Surface::try_drm_dumb(w, h, path) {
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

pub fn run(surf: &mut Surface, t0: Instant) {
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
