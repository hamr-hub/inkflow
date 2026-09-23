// inkflow — a self-iterating generative ambience object.
// Local LLM text stream + particles, shaped by touch; never goes dark.
use macroquad::prelude::*;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------- touch -------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Contact {
    x: f32, // normalized 0..1
    y: f32,
}

#[derive(Default)]
struct TouchState {
    contacts: Vec<Contact>,
    energy: f32,   // recent touch intensity 0..1, decays
    warmth: f32,   // smoothed x 0..1 (left=cold right=warm)
    last: Option<Instant>,
    device: String,
}

fn input_dir() -> std::path::PathBuf {
    std::env::var("INKFLOW_INPUT_DIR")
        .unwrap_or_else(|_| "/dev/input".to_string())
        .into()
}

fn is_touch_device(path: &std::path::Path) -> bool {
    if let Ok(mut d) = evdev::Device::open(path) {
        use evdev::AttributeSet;
        let props: std::collections::HashSet<evdev::InputProperty> = d.properties().collect();
        if props.contains(&evdev::InputProperty::INPUT_PROP_DIRECT) {
            return true;
        }        let abs = d.supported_absolute_axes();
        let has_x = abs.contains(&evdev::AbsoluteAxisType::ABS_MT_POSITION_X)
            || abs.contains(&evdev::AbsoluteAxisType::ABS_X);
        let has_touch = d
            .supported_keys()
            .map(|k| k.contains(&evdev::Key::BTN_TOUCH))
            .unwrap_or(false);
        return has_x && has_touch;
    }
    false
}

fn spawn_reader(path: std::path::PathBuf, st: Arc<Mutex<TouchState>>) {
    std::thread::spawn(move || loop {
        if let Ok(mut d) = evdev::Device::open(&path) {
            let name = d.name().unwrap_or("touch").to_string();
            let mut prev: Vec<(f32, f32)> = vec![];
            let mut win = d.abs_position(evdev::AbsoluteAxisType::ABS_MT_POSITION_X)
                .or_else(|| d.abs_position(evdev::AbsoluteAxisType::ABS_X));
            let mut win_y = d.abs_position(evdev::AbsoluteAxisType::ABS_MT_POSITION_Y)
                .or_else(|| d.abs_position(evdev::AbsoluteAxisType::ABS_Y));
            if win.is_none() { win = Some((0, 4096)); }
            if win_y.is_none() { win_y = Some((0, 4096)); }
            {
                let mut s = st.lock().unwrap();
                s.device = name.clone();
            }
            let mut slots: std::collections::BTreeMap<i32, (i32, i32)> = Default::default();
            let mut slot = 0i32;
            let mut legacy: Option<(i32, i32)> = None;
            while let Ok(events) = d.fetch_events() {
                for ev in events {
                    use evdev::{EventSummary, AbsoluteAxisType};
                    if let EventSummary::AbsoluteAxis(_, axis, v) = ev.summary() {
                        match axis {
                            AbsoluteAxisType::ABS_MT_SLOT => slot = v as i32,
                            AbsoluteAxisType::ABS_MT_POSITION_X => {
                                let e = slots.entry(slot).or_insert((0, 0));
                                e.0 = v;
                            }
                            AbsoluteAxisType::ABS_MT_POSITION_Y => {
                                let e = slots.entry(slot).or_insert((0, 0));
                                e.1 = v;
                            }
                            AbsoluteAxisType::ABS_X => {
                                legacy = Some((v, legacy.map(|p| p.1).unwrap_or(0)));
                            }
                            AbsoluteAxisType::ABS_Y => {
                                legacy = Some((legacy.map(|p| p.0).unwrap_or(0), v));
                            }
                            _ => {}
                        }
                        let (xmin, xmax) = win.unwrap((0, 4096));
                        let (ymin, ymax) = win_y.unwrap((0, 4096));
                        let pts: Vec<(f32, f32)> = if !slots.is_empty() {
                            slots.values()
                                .map(|(x, y)| {
                                    (((x - xmin) as f32 / (xmax - xmin) as f32).clamp(0., 1.),
                                     ((y - ymin) as f32 / (ymax - ymin) as f32).clamp(0., 1.))
                                })
                                .collect()
                        } else if let Some((x, y)) = legacy {
                            vec![(((x - xmin) as f32 / (xmax - xmin) as f32).clamp(0., 1.),
                                  ((y - ymin) as f32 / (ymax - ymin) as f32).clamp(0., 1.))]
                        } else {
                            vec![]
                        };
                        if !pts.is_empty() {
                            let mut s = st.lock().unwrap();
                            let mut speed = 0f32;
                            for (i, (x, y)) in pts.iter().enumerate() {
                                if let Some((px, py)) = prev.get(i) {
                                    speed += ((x - px).powi(2) + (y - py).powi(2)).sqrt();
                                }
                            }
                            prev = pts.clone();
                            s.contacts = pts.iter().map(|(x, y)| Contact { x: *x, y: *y }).collect();
                            s.warmth = s.warmth * 0.9
                                + pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32 * 0.1;
                            let n = pts.len() as f32;
                            s.energy = (s.energy + (speed * 8.0 + 0.15) * (0.5 + n * 0.3)).min(1.0);
                            s.last = Some(Instant::now());
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    });
}

fn start_touch_monitor(st: Arc<Mutex<TouchState>>) {
    std::thread::spawn(move || loop {
        if let Ok(rd) = std::fs::read_dir(input_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.file_name().map(|n| n.to_string_lossy().starts_with("event")).unwrap_or(false)
                    && is_touch_device(&p)
                {
                    spawn_reader(p, st.clone());
                }
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    });
}

// ---------- local LLM (ollama) -----------------------------------------

struct LlmHealth {
    ok: bool,
    toks_per_s: f32,
    last_text: String,
    model: String,
}

fn start_llm(model: String, mood_rx: Receiver<(f32, f32)>) -> (Receiver<char>, Arc<Mutex<LlmHealth>>) {
    let (tx, rx) = channel();
    let health = Arc::new(Mutex::new(LlmHealth {
        ok: false,
        toks_per_s: 0.,
        last_text: String::new(),
        model: model.clone(),
    }));
    let h2 = health.clone();
    std::thread::spawn(move || {
        let host = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "127.0.0.1:11434".into());
        let agent = ureq::AgentBuilder::new()
            .timeoutConnect(Duration::from_secs(5))
            .timeoutRead(Duration::from_secs(120))
            .build();
        let mut last_mood = (0.5f32, 0.5f32);
        loop {
            if let Ok(m) = mood_rx.try_recv() {
                last_mood = m;
            }
            let (warmth, energy) = last_mood;
            let mood_zh = if energy > 0.6 {
                if warmth > 0.55 { "炽烈、奔涌" } else { "凛冽、激荡" }
            } else if energy > 0.3 {
                if warmth > 0.55 { "温暖、流动" } else { "清冷、微澜" }
            } else if warmth > 0.55 {
                "静谧、温柔"
            } else {
                "幽深、寂静"
            };
            let prompt = format!(
                "你是一件数字艺术品的氛围文字源。用中文，只输出 30-60 个字，\
                 写一段{mood_zh}的意象碎片，像梦话，不解释，不断句成诗行，无标点堆砌，允许短句。"
            );
            let body = serde_json::json!({
                "model": model,
                "prompt": prompt,
                "stream": true,
                "options": {"num_predict": 90, "temperature": 1.05, "top_p": 0.92}
            });
            let t0 = Instant::now();
            let mut ntok = 0u32;
            let mut got = String::new();
            let req = agent.post(&format!("http://{host}/api/generate")).send_json(body);
            match req {
                Ok(resp) => {
                    let reader = resp.into_reader();
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(reader).lines().flatten() {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                            if let Some(s) = v.get("response").and_then(|x| x.as_str()) {
                                got.push_str(s);
                                ntok += 1;
                                for c in s.chars() {
                                    if tx.send(c).is_err() {
                                        return;
                                    }
                                }
                            }
                            if v.get("done").and_then(|x| x.as_bool()).unwrap_or(false) {
                                break;
                            }
                        }
                    }
                    let tps = ntok as f32 / t0.elapsed().as_secs_f32().max(0.001);
                    let mut h = h2.lock().unwrap();
                    h.ok = !got.is_empty();
                    h.toks_per_s = h.toks_per_s * 0.7 + tps * 0.3;
                    h.last_text = got.chars().take(80).collect();
                }
                Err(_) => {
                    h2.lock().unwrap().ok = false;
                    std::thread::sleep(Duration::from_secs(4));
                }
            }
        }
    });
    (rx, health)
}

// ---------- fallback ambient feed (never dark) -------------------------

const POOLS: &[(&str, &str)] = &[
    ("静", "雾 月 夜 潮 呼吸 微光 深处 沉睡 鲸落 尘埃 影 钟摆 雨前 纸页 苔"),
    ("动", "风 焰 河 奔 裂帛 星陨 心跳 浪尖 闪电 迁徙 鼓 惊鸟 火 渡口 弦"),
    ("冷", "雪 蓝 冰 星 霜 铁 墨 深空 孤 井 石英 冬 海沟 玻璃 月背"),
    ("暖", "灯 橘 麦 陶 体温 琥珀 黄昏 花信 茧 炊烟 蜜 绒 烛 岸 掌心"),
];

fn local_char(warmth: f32, energy: f32, n: u64) -> char {
    let bank = if n % 3 == 0 {
        if energy > 0.5 { POOLS[1].1 } else { POOLS[0].1 }
    } else if warmth > 0.5 {
        POOLS[3].1
    } else {
        POOLS[2].1
    };
    let words: Vec<char> = bank.chars().filter(|c| !c.is_whitespace()).collect();
    let i = (n.wrapping_mul(2654435761) as usize) % words.len();
    words[i]
}

// ---------- rendering ---------------------------------------------------

#[derive(Clone, Copy)]
struct Glyph {
    ch: char,
    x: f32,
    y: f32,
    vy: f32,
    vx: f32,
    life: f32,
    max_life: f32,
    size: f32,
}

#[derive(Clone, Copy)]
struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: f32,
    max_life: f32,
    r: f32,
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Color {
    let h = (h * 360.0).rem_euclid(360.);
    let c = (1. - (2. * l - 1.).abs()) * s;
    let x = c * (1. - (((h / 60.) % 2.) - 1.).abs());
    let m = l - c / 2.;
    let (r, g, b) = match h as u32 / 60 {
        0 => (c, x, 0.), 1 => (x, c, 0.), 2 => (0., c, x),
        3 => (0., x, c), 4 => (x, 0., c), _ => (c, 0., x),
    };
    Color::new(r + m, g + m, b + m, 1.)
}

#[derive(Serialize)]
struct Telemetry {
    ts: u64,
    fps: f32,
    warmth: f32,
    energy: f32,
    contacts: usize,
    touch_device: String,
    llm_ok: bool,
    llm_toks_per_s: f32,
    llm_model: String,
    llm_last: String,
    glyphs: usize,
    particles: usize,
}

fn append_jsonl(path: &str, v: &Telemetry) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", serde_json::to_string(v).unwrap_or_default());
    }
}

fn conf() -> Conf {
    Conf {
        window_title: "inkflow".into(),
        window_width: 1280,
        window_height: 800,
        fullscreen: true,
        ..Default::default()
    }
}

#[macroquad::main(conf)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--diag") {
        let mut found = vec![];
        if let Ok(rd) = std::fs::read_dir(input_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if is_touch_device(&p) {
                    if let Ok(d) = evdev::Device::open(&p) {
                        found.push(format!("{}: {}", p.display(), d.name().unwrap_or("?")));
                    }
                }
            }
        }
        let up = std::net::TcpStream::connect("127.0.0.1:11434").is_ok();
        println!("touch_devices={}", found.join(" | "));
        println!("ollama_tcp_11434={up}");
        std::process::exit(0);
    }

    let state_dir = std::env::var("INKFLOW_STATE_DIR").unwrap_or_else(|_| "state".into());
    std::fs::create_dir_all(&state_dir).ok();
    let tel_path = format!("{state_dir}/telemetry.jsonl");

    let font_path = std::env::var("INKFLOW_FONT")
        .unwrap_or_else(|_| "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into());
    let font = std::fs::read(&font_path).ok().and_then(|b| load_ttf_font_from_bytes(&b).ok());

    let touch: Arc<Mutex<TouchState>> = Arc::new(Mutex::new(TouchState::default()));
    start_touch_monitor(touch.clone());

    let model = std::env::var("INKFLOW_MODEL").unwrap_or_else(|_| "gemma3:1b".into());
    let (mood_tx, mood_rx) = channel();
    let (llm_chars, llm_health) = start_llm(model.clone(), mood_rx);

    let mut glyphs: Vec<Glyph> = vec![];
    let mut particles: Vec<Particle> = vec![];
    let mut tick: u64 = 0;
    let mut spawn_acc = 0f32;
    let mut last_tel = Instant::now();
    let start = Instant::now();
    let mut llm_starved_for = 0f32;

    loop {
        let dt = get_frame_time().clamp(0.001, 0.05);
        tick += 1;

        // mood
        let (energy, warmth, contacts, dev) = {
            let mut s = touch.lock().unwrap();
            s.energy *= 0.965f32.powf(dt * 60.);
            let stale = s.last.map(|l| l.elapsed().as_secs() > 2).unwrap_or(true);
            if stale {
                s.contacts.clear();
            }
            (s.energy, s.warmth, s.contacts.len(), s.device.clone())
        };
        // autonomous idle drift so the piece breathes alone
        let t = start.elapsed().as_secs_f32();
        let idle = ((t * 0.13).sin() * 0.5 + 0.5) * 0.25;
        let energy = energy.max(idle);
        let warmth = (warmth * 0.99 + 0.5 * 0.01).max(0.2).min(0.8);

        // mouse = fallback/pointer touch (also test path)
        let (mx, my) = mouse_position();
        let (sw, sh) = (screen_width(), screen_height());
        let mouse_down = is_mouse_button_down(MouseButton::Left);
        let eff_warmth = if mouse_down { mx / sw } else { warmth };
        let eff_energy = if mouse_down { energy.max(0.55) } else { energy };

        if tick % 60 == 0 {
            let _ = mood_tx.send((eff_warmth, eff_energy));
        }

        // pull LLM chars
        let mut llm_char: Option<char> = None;
        loop {
            match llm_chars.try_recv() {
                Ok(c) => {
                    llm_starved_for = 0.;
                    llm_char = Some(c);
                    break;
                }
                Err(TryRecvError::Empty) => {
                    llm_starved_for += dt;
                    break;
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }

        // spawn glyphs
        spawn_acc += dt * (1.5 + eff_energy * 9.0);
        while spawn_acc >= 1.0 {
            spawn_acc -= 1.0;
            let from_llm = llm_char.is_some();
            let ch = llm_char.take().unwrap_or_else(|| local_char(eff_warmth, eff_energy, tick + glyphs.len() as u64));
            if ch.is_whitespace() {
                continue;
            }
            let (x, y, vx, vy) = if mouse_down {
                (mx + (rand_fast(tick) - 0.5) * 60.,
                 my + (rand_fast(tick.wrapping_add(7)) - 0.5) * 60.,
                 (rand_fast(tick.wrapping_add(3)) - 0.5) * 30.,
                 -20. - eff_energy * 60.)
            } else {
                (rand_fast(tick.wrapping_add(11)) * sw,
                 sh + 20.,
                 (rand_fast(tick.wrapping_add(5)) - 0.5) * (10. + eff_energy * 40.),
                 -(12. + eff_energy * 70.))
            };
            let max_life = 6. + rand_fast(tick.wrapping_add(13)) * 6.;
            glyphs.push(Glyph {
                ch, x, y, vx, vy,
                life: max_life,
                max_life,
                size: (if from_llm { 26. } else { 20. }) + eff_energy * 14.,
            });
            if glyphs.len() > 220 {
                glyphs.remove(0);
            }
        }

        // spawn particles at contacts / pointer
        let pts: Vec<(f32, f32)> = {
            let s = touch.lock().unwrap();
            s.contacts.iter().map(|c| (c.x * sw, c.y * sh)).collect()
        };
        let mut sources = pts;
        if mouse_down {
            sources.push((mx, my));
        }
        for (px, py) in sources {
            let n = if eff_energy > 0.6 { 3 } else { 1 };
            for k in 0..n {
                let ang = rand_fast(tick.wrapping_add(k as u64 * 31)) * std::f32::consts::TAU;
                let sp = 20. + eff_energy * 90.;
                particles.push(Particle {
                    x: px, y: py,
                    vx: ang.cos() * sp,
                    vy: ang.sin() * sp - 20.,
                    life: 1.5 + rand_fast(tick.wrapping_add(99)) * 2.,
                    max_life: 3.5,
                    r: 1.5 + rand_fast(tick.wrapping_add(77)) * 3.,
                });
            }
        }
        // ambient drifting particles
        if particles.len() < 90 && tick % 8 == 0 {
            particles.push(Particle {
                x: rand_fast(tick.wrapping_add(41)) * sw,
                y: sh + 4.,
                vx: (rand_fast(tick.wrapping_add(43)) - 0.5) * 12.,
                vy: -8. - eff_energy * 20.,
                life: 4., max_life: 6., r: 1. + rand_fast(tick.wrapping_add(47)) * 2.,
            });
        }
        if particles.len() > 260 {
            particles.drain(0..particles.len() - 260);
        }

        // hue: cold blue 210deg .. warm amber 30deg
        let hue = (0.58 - eff_warmth * 0.5).rem_euclid(1.0);

        // draw
        clear_background(Color::new(0.012, 0.012, 0.02, 1.));
        for p in particles.iter_mut() {
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.vy -= 6. * dt;
            p.life -= dt;
            let a = (p.life / p.max_life).clamp(0., 1.);
            let mut c = hsl_to_rgb(hue, 0.7, 0.6);
            c.a = a * 0.5;
            draw_circle(p.x, p.y, p.r, c);
        }
        particles.retain(|p| p.life > 0. && p.y > -20.);

        for g in glyphs.iter_mut() {
            g.x += g.vx * dt;
            g.y += g.vy * dt;
            g.life -= dt;
            let a = ((g.life / g.max_life) as f32).clamp(0., 1.);
            let mut c = hsl_to_rgb(hue, 0.55, 0.72);
            c.a = a * 0.9;
            let params = TextParams {
                font: font.as_ref(),
                font_size: g.size as u16,
                color: c,
                ..Default::default()
            };
            draw_text_ex(&g.ch.to_string(), g.x, g.y, params);
        }
        glyphs.retain(|g| g.life > 0. && g.y > -40.);

        // soft vignette
        draw_rectangle(0., 0., sw, 3., Color::new(0., 0., 0., 0.25));

        if last_tel.elapsed().as_secs() >= 10 {
            last_tel = Instant::now();
            let h = llm_health.lock().unwrap();
            append_jsonl(&tel_path, &Telemetry {
                ts: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                fps: get_fps(),
                warmth: eff_warmth,
                energy: eff_energy,
                contacts: if mouse_down { 1 } else { contacts },
                touch_device: if mouse_down { "mouse".into() } else { dev.clone() },
                llm_ok: h.ok,
                llm_toks_per_s: (h.toks_per_s * 100.).round() / 100.,
                llm_model: h.model.clone(),
                llm_last: h.last_text.clone(),
                glyphs: glyphs.len(),
                particles: particles.len(),
            });
        }

        next_frame().await;
    }
}

fn rand_fast(seed: u64) -> f32 {
    let x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((x >> 33) as f32) / (1u64 << 31) as f32
}
