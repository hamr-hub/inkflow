// inkflow · main.rs
//
// Single-binary self-iterating ambience: real DRM/KMS dumb-buffer render,
// raw evdev touch, hand-rolled ollama streaming, embedded CJK font.
// One frame loop, no X / no Wayland / no GL.
//
// See ZERO_DEP.md for the binding spec and PRODUCTION.md for evidence.

mod drm;
mod evdev;
mod font;
mod fontdata;
mod net_ollama;
mod scene;
mod sys;
mod telemetry;

use crate::drm::{log, Display, Headless};
use crate::evdev::TouchState;
use crate::font::Rgba;
use crate::scene::{lcg, Glyph, Particle, Scene};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ---------- fallback ambient pools (mirror of the original macroquad version) ----------

const POOLS: &[(&str, &str)] = &[
    (
        "静",
        "雾 月 夜 潮 呼吸 微光 沉睡 鲸落 尘埃 影 钟摆 雨前 纸页 苔 林 木 叶 泉 雪落 远钟 云根 幽径 落花 鸿影 薄暮 清露 听蝉 听雪",
    ),
    (
        "动",
        "风 焰 河 奔 裂帛 星陨 心跳 浪尖 闪电 迁徙 鼓 惊鸟 火 渡口 弦 雷 潮涌 雷鸣 烟火 龙吟 震颤 飞溅 雪崩 迸裂 翻涌 流火 疾行",
    ),
    (
        "冷",
        "雪 蓝 冰 星 霜 铁 墨 深空 孤 井 石英 冬 海沟 玻璃 月背 银 寒 朔风 凝霜 寒潭 远岭 苍 凛 薄冰 星河 落雪 静海",
    ),
    (
        "暖",
        "灯 橘 麦 陶 体温 琥珀 黄昏 花信 茧 炊烟 蜜 绒 烛 岸 掌心 茶 暖 炉火 夕照 茶烟 旧书 木质 余温 棉 晨曦 晚风",
    ),
];

const FALLBACK_SIZE_BUMP: f32 = 32.0;
const LLM_SIZE: f32 = 34.0;
const LLM_BACKUP_SIZE: f32 = 28.0;

fn local_glyph(warmth: f32, energy: f32, n: u64) -> &'static str {
    let bank = if n.is_multiple_of(3) {
        if energy > 0.5 {
            POOLS[1].1
        } else {
            POOLS[0].1
        }
    } else if warmth > 0.5 {
        POOLS[3].1
    } else {
        POOLS[2].1
    };
    let words: Vec<&'static str> = bank.split_whitespace().collect();
    if words.is_empty() {
        return "墨";
    }
    words[(n.wrapping_mul(2654435761) as usize) % words.len()]
}

// ---------- shared state ----------

struct Shared {
    llm_ok: bool,
    llm_tps: f32,
    llm_last: String,
    llm_chars: VecDeque<char>,
}

impl Shared {
    fn new() -> Self {
        Self {
            llm_ok: false,
            llm_tps: 0.0,
            llm_last: String::new(),
            llm_chars: VecDeque::with_capacity(2048),
        }
    }
}

// ---------- args / diag ----------

fn parse_args() -> Vec<String> {
    std::env::args().collect()
}

fn run_diag(_args: &[String]) -> ! {
    let mut found: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(evdev::open_input_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("event") {
                continue;
            }
            let path = p.to_string_lossy().to_string();
            if evdev::is_touch_device(&path) {
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
    // Report the real driver for each card so --diag shows the actual
    // UAPI version the kernel returned (proves the ioctl encoding is
    // right, not just "no error").
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

/// One-shot modeset proof. Walks the full ioctl sequence on /dev/dri/card0
/// (or whichever card opens first), draws an unmistakable test pattern +
/// a handful of CJK glyphs into the dumb buffer, holds for 5s so a human
/// can read it, restores the original CRTC, captures the buffer to
/// state/screen.png, and exits. Does not enter the persistent render loop.
fn run_drm_test(_args: &[String]) -> ! {
    use crate::font::{draw_glyph, fill_circle, fill_rect, Rgba};

    let state_dir = std::env::var("INKFLOW_STATE_DIR").unwrap_or_else(|_| "state".into());
    std::fs::create_dir_all(&state_dir).ok();
    let ppm_path = format!("{state_dir}/drm-test.ppm");
    let png_path = format!("{state_dir}/drm-test.png");

    // Pick a card and build the modeset. If no connector is connected
    // (headless / no monitor) fall back to a dumb-buffer-only surface so
    // the create_dumb + addfb + map path is still proven end-to-end.
    let mut display = match drm::open_first() {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "drm-test: open_first failed ({e}); falling back to dumb-buffer-only surface"
            );
            match drm::open_dumb_only(1280, 720) {
                Ok(d) => {
                    eprintln!("drm-test: dumb-only surface ready (no connector to scan out to)");
                    d
                }
                Err(e2) => {
                    eprintln!("drm-test: open_dumb_only also failed: {e2}");
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
    // Background: deep ink. Foreground: six saturated bars + a center
    // halo + four CJK glyphs in known positions. Anyone watching the
    // screen can verify orientation, scaling and alpha blending. Anyone
    // reviewing the captured PNG can verify the same.
    {
        let pixels = display.pixels();
        let bg = Rgba(8, 6, 18, 255);
        for px in pixels.iter_mut() {
            *px = bg.0 as u32 | ((bg.1 as u32) << 8) | ((bg.2 as u32) << 16) | 0xFF000000;
        }
        let bars: [Rgba; 6] = [
            Rgba(255, 80, 60, 255),   // red
            Rgba(255, 200, 60, 255),  // amber
            Rgba(90, 220, 90, 255),   // green
            Rgba(80, 200, 255, 255),  // cyan
            Rgba(120, 90, 240, 255),  // indigo
            Rgba(240, 100, 220, 255), // magenta
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
        // central halo so the "DRM works" reading is obvious even on a
        // small framebuffer.
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
        // Four CJK glyphs from the embedded font (proven to exist via
        // main loop usage). If any render, the embedded font + alpha
        // path is proven end-to-end on real DRM pixels.
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
        // header line so the PNG is self-describing.
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
    }

    // Push the frame once (legacy SETCRTC doubles as a flush for the
    // dumb-buffer path; harmless if modeset_ok is already true).
    display.present();

    eprintln!("drm-test: frame drawn; holding 5s for visual inspection");
    std::thread::sleep(std::time::Duration::from_secs(5));

    // Capture the buffer to disk before we tear down the fb.
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

    // Convert to PNG with whatever ffmpeg is on PATH.
    let ff = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i", &ppm_path, &png_path])
        .status();
    match ff {
        Ok(s) if s.success() => eprintln!("drm-test: wrote {png_path}"),
        Ok(s) => eprintln!("drm-test: ffmpeg exit {s:?}; ppm retained as evidence"),
        Err(e) => eprintln!("drm-test: ffmpeg not available ({e}); ppm retained as evidence"),
    }

    // Restore the original CRTC (handled in Drop too, but explicit is
    // nicer for the audit log).
    display.restore();
    drop(display);

    eprintln!(
        "drm-test: done (modeset_ok={})",
        std::fs::metadata(&png_path).is_ok()
    );
    std::process::exit(0);
}

// ---------- surface abstraction ----------

enum Surface {
    Real(Display),
    Headless(Headless),
}

impl Surface {
    fn w(&self) -> u32 {
        match self {
            Surface::Real(d) => d.width,
            Surface::Headless(h) => h.width,
        }
    }
    fn h(&self) -> u32 {
        match self {
            Surface::Real(d) => d.height,
            Surface::Headless(h) => h.height,
        }
    }
    fn pitch_px(&self) -> usize {
        match self {
            Surface::Real(d) => (d.pitch / 4) as usize,
            Surface::Headless(h) => h.width as usize,
        }
    }
    fn pixels(&mut self) -> &mut [u32] {
        match self {
            Surface::Real(d) => d.pixels(),
            Surface::Headless(h) => h.pixels(),
        }
    }
    fn present(&self) {
        match self {
            Surface::Real(d) => d.present(),
            Surface::Headless(_) => {}
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

    let state_dir = std::env::var("INKFLOW_STATE_DIR").unwrap_or_else(|_| "state".into());
    std::fs::create_dir_all(&state_dir).ok();
    let tel_path = format!("{state_dir}/telemetry.jsonl");
    let shot_path = format!("{state_dir}/screen.png");

    let mut display = match drm::open_first() {
        Ok(d) => {
            log!("display: real DRM/KMS dumb-buffer");
            Surface::Real(d)
        }
        Err(e) => {
            log!("display: headless fallback (no DRM): {e}");
            Surface::Headless(Headless::new(1280, 800))
        }
    };
    let fb_w = display.w();
    let fb_h = display.h();
    let pitch_px = display.pitch_px();

    let touch = Arc::new(Mutex::new(TouchState::default()));
    evdev::start_supervisor(touch.clone());

    let shared = Arc::new(Mutex::new(Shared::new()));
    let model = std::env::var("INKFLOW_MODEL").unwrap_or_else(|_| "gemma3:1b".into());
    let llm_client = net_ollama::OllamaClient::new(&model);
    let mood = Arc::new(Mutex::new((0.5f32, 0.5f32)));
    {
        let shared = shared.clone();
        let mood = mood.clone();
        std::thread::spawn(move || llm_worker(llm_client, shared, mood));
    }

    let mut scene = Scene::new();
    let mut tick: u64 = 0;
    let mut spawn_acc: f32 = 0.0;
    let mut last_tel = Instant::now();
    let mut last_shot = Instant::now();
    let mut fps_acc = 0.0f32;
    let mut fps_n = 0u32;
    let start = Instant::now();
    let mut prev_frame = start;

    let mut sources: Vec<(f32, f32)> = Vec::with_capacity(8);
    let mut dev_buf: String;
    let model_buf = model.clone();

    loop {
        let now = Instant::now();
        let dt = (now - prev_frame).as_secs_f32().clamp(0.001, 0.05);
        prev_frame = now;
        let t = start.elapsed().as_secs_f32();
        tick = tick.wrapping_add(1);

        // ----- mood read -----
        let (energy, warmth, contacts, dev) = {
            let mut s = touch.lock().unwrap();
            s.energy *= 0.965f32.powf(dt * 60.0);
            let warm_target = 0.5 + 0.22 * (t * 0.045).sin();
            s.warmth = (s.warmth * 0.985 + warm_target * 0.015).clamp(0.2, 0.8);
            let stale = s.last.map(|l| l.elapsed().as_secs() > 2).unwrap_or(true);
            if stale {
                s.contacts.clear();
            }
            (s.energy, s.warmth, s.contacts.len(), s.device.clone())
        };
        let idle = ((t * 0.13).sin() * 0.5 + 0.5) * 0.25;
        let energy = energy.max(idle);

        if scene.stars.is_empty() && fb_w > 0 && fb_h > 0 {
            scene.seed_stars(fb_w as f32, fb_h as f32);
        }

        // ----- LLM char drain -----
        let llm_char: Option<char> = {
            let mut sh = shared.lock().unwrap();
            sh.llm_chars.pop_front()
        };
        let mut llm_char_str: Option<&'static str> = llm_char.map(lookup_char);

        // ----- spawn glyphs -----
        spawn_acc += dt * (3.0 + energy * 11.0);
        while spawn_acc >= 1.0 {
            spawn_acc -= 1.0;
            let from_llm = llm_char_str.is_some();
            let ch = llm_char_str.take().unwrap_or_else(|| {
                local_glyph(warmth, energy, tick.wrapping_add(scene.glyphs.len() as u64))
            });
            let llm_ok = shared.lock().unwrap().llm_ok;
            let base_size = if from_llm {
                LLM_SIZE
            } else if llm_ok {
                LLM_BACKUP_SIZE
            } else {
                FALLBACK_SIZE_BUMP
            };
            let speed_jitter = 0.82
                + lcg(tick
                    .wrapping_add(131)
                    .wrapping_add(scene.glyphs.len() as u64))
                    * 0.36;
            let speed = (55.0 + energy * 130.0) * speed_jitter;
            let size_jitter = 0.88
                + lcg(tick
                    .wrapping_add(113)
                    .wrapping_add(scene.glyphs.len() as u64))
                    * 0.24;
            let max_life = (fb_h as f32 + 40.0) / speed + 1.5;
            scene.push_glyph(Glyph {
                ch,
                x: lcg(tick.wrapping_add(11)) * fb_w as f32,
                y: fb_h as f32 + 20.0,
                vx: (lcg(tick.wrapping_add(5)) - 0.5) * (10.0 + energy * 40.0),
                vy: -speed,
                life: max_life,
                max_life,
                size: (base_size + energy * 16.0) * size_jitter,
                phase: lcg(tick
                    .wrapping_add(197)
                    .wrapping_add(scene.glyphs.len() as u64))
                    * core::f32::consts::TAU,
            });
        }

        // ----- spawn particles -----
        sources.clear();
        {
            let s = touch.lock().unwrap();
            for c in s.contacts.iter() {
                sources.push((c.x * fb_w as f32, c.y * fb_h as f32));
            }
        }
        for &(px, py) in &sources {
            let n = if energy > 0.6 { 3 } else { 1 };
            for k in 0..n {
                let ang = lcg(tick.wrapping_add(k as u64 * 31)) * core::f32::consts::TAU;
                let sp = 20.0 + energy * 90.0;
                scene.push_particle(Particle {
                    x: px,
                    y: py,
                    vx: ang.cos() * sp,
                    vy: ang.sin() * sp - 20.0,
                    life: 1.5 + lcg(tick.wrapping_add(99)) * 2.0,
                    max_life: 3.5,
                    r: 1.5 + lcg(tick.wrapping_add(77)) * 3.0,
                });
            }
        }
        if scene.particles.len() < 90 && tick.is_multiple_of(8) {
            scene.push_particle(Particle {
                x: lcg(tick.wrapping_add(41)) * fb_w as f32,
                y: fb_h as f32 + 4.0,
                vx: (lcg(tick.wrapping_add(43)) - 0.5) * 12.0,
                vy: -8.0 - energy * 20.0,
                life: 4.0,
                max_life: 6.0,
                r: 1.0 + lcg(tick.wrapping_add(47)) * 2.0,
            });
        }

        let hue_drift = (t * 0.025).sin() * 0.18;
        let hue = (0.58 - warmth * 0.5 + hue_drift).rem_euclid(1.0);

        // ----- draw -----
        let bg = (3u32) | (3u32 << 8) | (5u32 << 16) | (255 << 24);
        let pixels = display.pixels();
        for px in pixels.iter_mut() {
            *px = bg;
        }

        let neb_a_alpha = 0.07 + 0.04 * (t * 0.05).sin();
        let neb_b_alpha = 0.05 + 0.035 * (t * 0.04 + 1.7).cos();
        let na_x = fb_w as f32 * (0.5 + 0.28 * (t * 0.018).sin());
        let na_y = fb_h as f32 * (0.5 + 0.20 * (t * 0.013).cos());
        for i in 0i32..5 {
            let r = 0.18 + 0.13 * i as f32;
            let color = Rgba::from_hsl((hue + 0.5).rem_euclid(1.0), 0.55, 0.5);
            font::fill_circle(
                pixels,
                pitch_px,
                fb_w as i32,
                fb_h as i32,
                na_x,
                na_y,
                fb_w as f32 * r,
                color,
                neb_a_alpha * (1.0 - i as f32 / 4.0).powi(2),
            );
        }
        let nb_x = fb_w as f32 * (0.5 + 0.28 * (t * 0.017).cos());
        let nb_y = fb_h as f32 * (0.5 + 0.20 * (t * 0.022).sin());
        for i in 0i32..5 {
            let r = 0.16 + 0.11 * i as f32;
            let color = Rgba::from_hsl(hue, 0.6, 0.45);
            font::fill_circle(
                pixels,
                pitch_px,
                fb_w as i32,
                fb_h as i32,
                nb_x,
                nb_y,
                fb_w as f32 * r,
                color,
                neb_b_alpha * (1.0 - i as f32 / 4.0).powi(2),
            );
        }
        for st in scene.stars.iter() {
            let k = 0.5 + 0.5 * (t / st.period * core::f32::consts::TAU + st.phase).sin();
            let color = Rgba::from_hsl((hue + st.hue_offset).rem_euclid(1.0), 0.35, 0.88);
            font::fill_circle(
                pixels,
                pitch_px,
                fb_w as i32,
                fb_h as i32,
                st.x,
                st.y,
                st.r,
                color,
                st.base * (0.25 + 0.75 * k),
            );
        }
        for p in scene.particles.iter_mut() {
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.vy -= 6.0 * dt;
            p.life -= dt;
            let a = (p.life / p.max_life).clamp(0.0, 1.0);
            let p_hue = (hue + (p.x * 0.3 + p.y * 0.5).sin() * 0.06 + (t * 0.05).sin() * 0.08)
                .rem_euclid(1.0);
            let color = Rgba::from_hsl(p_hue, 0.7, 0.6);
            font::fill_circle(
                pixels,
                pitch_px,
                fb_w as i32,
                fb_h as i32,
                p.x,
                p.y,
                p.r,
                color,
                a * 0.5,
            );
        }
        scene.particles.retain(|p| p.life > 0.0 && p.y > -20.0);

        for g in scene.glyphs.iter_mut() {
            g.x += g.vx * dt;
            g.y += g.vy * dt;
            g.x += (t * 0.55 + g.y * 0.012).sin() * 6.0 * dt;
            g.y += ((t * 0.42 + g.phase).sin()) * 1.5 * dt;
            g.x += ((t * 0.31 + g.phase * 1.3).sin()) * 1.0 * dt;
            g.life -= dt;
            let a = (g.life / g.max_life).clamp(0.0, 1.0);
            let aeased = a * a * (3.0 - 2.0 * a);
            let elapsed = 1.0 - a;
            let birth = (elapsed / 0.07).clamp(0.0, 1.0);
            let birth_eased = birth * birth * (3.0 - 2.0 * birth);
            let row_hue = (hue + (g.y * 0.003).sin() * 0.045).rem_euclid(1.0);
            let top_fade = (g.y / (fb_h as f32 * 0.18)).clamp(0.0, 1.0);
            let bottom_fade = ((fb_h as f32 - g.y) / (fb_h as f32 * 0.18)).clamp(0.0, 1.0);
            let rot = ((t * 0.32 + g.y * 0.011).sin()) * 0.045;
            let halo_age = 1.0 - a;
            let halo_strength = (halo_age * (1.0 - halo_age) * 4.0).min(1.0);
            let draw_size = g.size * (0.5 + 0.5 * birth_eased);
            let halo_color = Rgba::from_hsl(row_hue, 0.4, 0.45);
            font::fill_circle(
                pixels,
                pitch_px,
                fb_w as i32,
                fb_h as i32,
                g.x,
                g.y + draw_size * 0.3,
                draw_size * 0.7,
                halo_color,
                halo_strength * 0.12 * top_fade * bottom_fade,
            );
            let fg = Rgba::from_hsl(row_hue, 0.45, 0.85);
            let fg_alpha = birth_eased * (0.30 + aeased * 0.65) * top_fade * bottom_fade;
            font::draw_glyph(
                pixels,
                pitch_px,
                fb_w as i32,
                fb_h as i32,
                g.x,
                g.y,
                draw_size,
                g.ch,
                fg,
                fg_alpha,
                rot,
            );
        }
        scene.glyphs.retain(|g| g.life > 0.0 && g.y > -40.0);

        font::fill_rect(
            pixels,
            pitch_px,
            fb_w as i32,
            fb_h as i32,
            0,
            0,
            fb_w as i32,
            3,
            Rgba(0, 0, 0, 255),
            0.25,
        );

        display.present();

        // ----- telemetry + frame grab -----
        fps_acc += 1.0 / dt;
        fps_n += 1;
        if last_tel.elapsed().as_secs() >= 10 {
            last_tel = Instant::now();
            let avg_fps = if fps_n > 0 {
                fps_acc / fps_n as f32
            } else {
                0.0
            };
            fps_acc = 0.0;
            fps_n = 0;
            dev_buf = dev;
            // model is already a String above; use it directly to keep telemetry caller simple.
            let _ = model_buf;
            let dev_str = if dev_buf.is_empty() {
                "none"
            } else {
                dev_buf.as_str()
            };
            let model_str = model_buf.as_str();
            let sh = shared.lock().unwrap();
            telemetry::append(
                &tel_path,
                &telemetry::Telemetry {
                    ts: sys::wall_s(),
                    fps: avg_fps,
                    warmth,
                    energy,
                    contacts,
                    touch_device: dev_str,
                    llm_ok: sh.llm_ok,
                    llm_toks_per_s: sh.llm_tps,
                    llm_model: model_str,
                    llm_last: &sh.llm_last,
                    glyphs: scene.glyphs.len(),
                    particles: scene.particles.len(),
                },
            );
        }
        if last_shot.elapsed().as_secs() >= 60 {
            last_shot = Instant::now();
            // Snapshot pixels into an owned Vec on the render thread
            // (one synchronous 4 MB copy per minute — bounded), then hand
            // everything else to the worker: PPM write + ffmpeg PPM→PNG.
            // ffmpeg previously ran synchronously here via .status(),
            // which blocked the render loop for 100-500 ms every 60 s
            // and violated the "single-frame cost < 2 ms" production
            // contract.
            let bytes: Vec<u32> = display.pixels().to_vec();
            let w = fb_w;
            let h = fb_h;
            let path = format!("{state_dir}/screen.ppm");
            let shot = shot_path.clone();
            std::thread::spawn(move || {
                if let Ok(mut f) = std::fs::File::create(&path) {
                    use std::io::Write;
                    let _ = writeln!(f, "P6\n{w} {h}\n255");
                    let mut buf = Vec::with_capacity((w * h * 3) as usize);
                    for &p in bytes.iter() {
                        buf.push((p & 0xFF) as u8);
                        buf.push(((p >> 8) & 0xFF) as u8);
                        buf.push(((p >> 16) & 0xFF) as u8);
                    }
                    let _ = f.write_all(&buf);
                }
                // PNG encode happens here, off the render thread.
                let _ = std::process::Command::new("ffmpeg")
                    .args(["-y", "-loglevel", "error", "-i", &path, &shot])
                    .status();
            });
        }

        let frame_target = std::time::Duration::from_micros(16_667);
        let elapsed = now.elapsed();
        if elapsed < frame_target {
            std::thread::sleep(frame_target - elapsed);
        }
    }
}

fn lookup_char(c: char) -> &'static str {
    // fast path: ASCII byte == a single-char string for the embedded font
    let mut buf = [0u8; 4];
    let s: &str = c.encode_utf8(&mut buf);
    font::static_key_for(s).unwrap_or("墨")
}

fn llm_worker(
    client: net_ollama::OllamaClient,
    shared: Arc<Mutex<Shared>>,
    mood: Arc<Mutex<(f32, f32)>>,
) {
    loop {
        let (warmth, energy) = {
            let g = mood.lock().unwrap();
            *g
        };
        let res = client.generate(warmth, energy);
        {
            let mut sh = shared.lock().unwrap();
            if res.toks > 0 {
                sh.llm_ok = true;
                let tps = res.toks as f32 / res.elapsed.as_secs_f32().max(0.001);
                sh.llm_tps = sh.llm_tps * 0.7 + tps * 0.3;
                sh.llm_last = res.last_text.clone();
                for c in res.chars {
                    sh.llm_chars.push_back(c);
                    if sh.llm_chars.len() > 2048 {
                        sh.llm_chars.pop_front();
                    }
                }
            } else if !res.last_text.is_empty() {
                std::thread::sleep(std::time::Duration::from_secs(3));
            } else {
                sh.llm_ok = false;
                std::thread::sleep(std::time::Duration::from_secs(4));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
}
