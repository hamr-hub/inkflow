// inkflow — a self-iterating generative ambience object.
// Local LLM text stream + particles, shaped by touch; never goes dark.
use macroquad::prelude::*;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::mpsc::{channel, Receiver};
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
    energy: f32, // recent touch intensity 0..1, decays
    warmth: f32, // smoothed x 0..1 (left=cold right=warm)
    last: Option<Instant>,
    device: String,
}

fn input_dir() -> std::path::PathBuf {
    std::env::var("INKFLOW_INPUT_DIR")
        .unwrap_or_else(|_| "/dev/input".to_string())
        .into()
}

fn is_touch_device(path: &std::path::Path) -> bool {
    if let Ok(d) = evdev::Device::open(path) {
        if d.properties().contains(evdev::PropType::DIRECT) {
            return true;
        }
        let has_x = d
            .supported_absolute_axes()
            .map(|abs| {
                abs.contains(evdev::AbsoluteAxisType::ABS_MT_POSITION_X)
                    || abs.contains(evdev::AbsoluteAxisType::ABS_X)
            })
            .unwrap_or(false);
        let has_touch = d
            .supported_keys()
            .map(|k| k.contains(evdev::Key::BTN_TOUCH))
            .unwrap_or(false);
        return has_x && has_touch;
    }
    false
}

#[derive(Clone, Copy)]
struct AxisWin {
    xmin: i32,
    xmax: i32,
    ymin: i32,
    ymax: i32,
}

fn axis_window(d: &evdev::Device) -> AxisWin {
    let mut w = AxisWin {
        xmin: 0,
        xmax: 4096,
        ymin: 0,
        ymax: 4096,
    };
    if let Ok(state) = d.get_abs_state() {
        let ix = if d
            .supported_absolute_axes()
            .map(|a| a.contains(evdev::AbsoluteAxisType::ABS_MT_POSITION_X))
            .unwrap_or(false)
        {
            evdev::AbsoluteAxisType::ABS_MT_POSITION_X
        } else {
            evdev::AbsoluteAxisType::ABS_X
        };
        let iy = if d
            .supported_absolute_axes()
            .map(|a| a.contains(evdev::AbsoluteAxisType::ABS_MT_POSITION_Y))
            .unwrap_or(false)
        {
            evdev::AbsoluteAxisType::ABS_MT_POSITION_Y
        } else {
            evdev::AbsoluteAxisType::ABS_Y
        };
        let ax = state[ix.0 as usize];
        let ay = state[iy.0 as usize];
        if ax.maximum > ax.minimum {
            w.xmin = ax.minimum;
            w.xmax = ax.maximum;
        }
        if ay.maximum > ay.minimum {
            w.ymin = ay.minimum;
            w.ymax = ay.maximum;
        }
    }
    w
}

fn spawn_reader(path: std::path::PathBuf, st: Arc<Mutex<TouchState>>) {
    std::thread::spawn(move || loop {
        if let Ok(mut d) = evdev::Device::open(&path) {
            let name = d.name().unwrap_or("touch").to_string();
            let win = axis_window(&d);
            {
                let mut s = st.lock().unwrap();
                s.device = name.clone();
            }
            let mut slots: std::collections::BTreeMap<i32, (i32, i32)> = Default::default();
            let mut slot = 0i32;
            let mut legacy: Option<(i32, i32)> = None;
            let mut prev: Vec<(f32, f32)> = vec![];
            while let Ok(events) = d.fetch_events() {
                let mut moved = false;
                for ev in events {
                    use evdev::{AbsoluteAxisType, InputEventKind};
                    if let InputEventKind::AbsAxis(axis) = ev.kind() {
                        let v = ev.value();
                        match axis {
                            AbsoluteAxisType::ABS_MT_SLOT => slot = v,
                            AbsoluteAxisType::ABS_MT_POSITION_X => {
                                slots.entry(slot).or_insert((0, 0)).0 = v;
                                moved = true;
                            }
                            AbsoluteAxisType::ABS_MT_POSITION_Y => {
                                slots.entry(slot).or_insert((0, 0)).1 = v;
                                moved = true;
                            }
                            AbsoluteAxisType::ABS_X => {
                                legacy = Some((v, legacy.map(|p| p.1).unwrap_or(0)));
                                moved = true;
                            }
                            AbsoluteAxisType::ABS_Y => {
                                legacy = Some((v, legacy.map(|p| p.0).unwrap_or(0)));
                                moved = true;
                            }
                            _ => {}
                        }
                    }
                }
                if moved {
                    let pts: Vec<(f32, f32)> = if !slots.is_empty() {
                        slots
                            .values()
                            .map(|(x, y)| {
                                (
                                    ((x - win.xmin) as f32 / (win.xmax - win.xmin) as f32)
                                        .clamp(0., 1.),
                                    ((y - win.ymin) as f32 / (win.ymax - win.ymin) as f32)
                                        .clamp(0., 1.),
                                )
                            })
                            .collect()
                    } else if let Some((x, y)) = legacy {
                        vec![(
                            ((x - win.xmin) as f32 / (win.xmax - win.xmin) as f32).clamp(0., 1.),
                            ((y - win.ymin) as f32 / (win.ymax - win.ymin) as f32).clamp(0., 1.),
                        )]
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
        std::thread::sleep(Duration::from_secs(2));
    });
}

fn start_touch_monitor(st: Arc<Mutex<TouchState>>) {
    std::thread::spawn(move || loop {
        if let Ok(rd) = std::fs::read_dir(input_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("event"))
                    .unwrap_or(false)
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

fn start_llm(
    model: String,
    mood_rx: Receiver<(f32, f32)>,
) -> (Receiver<char>, Arc<Mutex<LlmHealth>>) {
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
            .timeout(Duration::from_secs(120))
            .build();
        let mut last_mood = (0.5f32, 0.5f32);
        loop {
            if let Ok(m) = mood_rx.try_recv() {
                last_mood = m;
            }
            let (warmth, energy) = last_mood;
            let mood_zh = if energy > 0.6 {
                if warmth > 0.55 {
                    "炽烈、奔涌"
                } else {
                    "凛冽、激荡"
                }
            } else if energy > 0.3 {
                if warmth > 0.55 {
                    "温暖、流动"
                } else {
                    "清冷、微澜"
                }
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
            // Cap streaming time so a glacially slow ollama (e.g. 0.04 tok/s)
            // can't hold the request thread hostage for half an hour; we
            // abandon partial output and resend the prompt on the next loop.
            let req_budget = Duration::from_secs(20);
            let req = agent
                .post(&format!("http://{host}/api/generate"))
                .send_json(body);
            match req {
                Ok(resp) => {
                    let reader = resp.into_reader();
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(reader).lines() {
                        if t0.elapsed() > req_budget {
                            break;
                        }
                        let Ok(line) = line else { break };
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
    (
        "静",
        "雾 月 夜 潮 呼吸 微光 深处 沉睡 鲸落 尘埃 影 钟摆 雨前 纸页 苔",
    ),
    (
        "动",
        "风 焰 河 奔 裂帛 星陨 心跳 浪尖 闪电 迁徙 鼓 惊鸟 火 渡口 弦",
    ),
    (
        "冷",
        "雪 蓝 冰 星 霜 铁 墨 深空 孤 井 石英 冬 海沟 玻璃 月背",
    ),
    (
        "暖",
        "灯 橘 麦 陶 体温 琥珀 黄昏 花信 茧 炊烟 蜜 绒 烛 岸 掌心",
    ),
];

fn local_char(warmth: f32, energy: f32, n: u64) -> char {
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
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
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
    let shot_path = format!("{state_dir}/screen.png");
    let mut last_shot = Instant::now();

    let font_path = std::env::var("INKFLOW_FONT")
        .unwrap_or_else(|_| "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc".into());
    let font = std::fs::read(&font_path)
        .ok()
        .and_then(|b| load_ttf_font_from_bytes(&b).ok());

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

    loop {
        let dt = get_frame_time().clamp(0.001, 0.05);
        tick += 1;

        // mood
        let t = start.elapsed().as_secs_f32();
        let (energy, warmth, contacts, dev) = {
            let mut s = touch.lock().unwrap();
            s.energy *= 0.965f32.powf(dt * 60.);
            // autonomous idle warmth drift — drive the target along a slow
            // sine so cool↔warm actually breathes when no touch is shaping it
            // (the earlier formula EMA'd toward a constant 0.5 and pinned
            // warmth there forever). Touch writes s.warmth directly in the
            // evdev thread, so contact still pulls the bias off-axis; once
            // the user lets go this gentle lerp relaxes back toward the
            // wandering target.
            let warm_target = 0.5 + 0.22 * (t * 0.045).sin();
            s.warmth = (s.warmth * 0.985 + warm_target * 0.015).clamp(0.2, 0.8);
            let stale = s.last.map(|l| l.elapsed().as_secs() > 2).unwrap_or(true);
            if stale {
                s.contacts.clear();
            }
            (s.energy, s.warmth, s.contacts.len(), s.device.clone())
        };
        // autonomous idle drift so the piece breathes alone
        let idle = ((t * 0.13).sin() * 0.5 + 0.5) * 0.25;
        let energy = energy.max(idle);

        // mouse = fallback/pointer touch (also test path)
        let (mx, my) = mouse_position();
        let (sw, sh) = (screen_width(), screen_height());
        let mouse_down = is_mouse_button_down(MouseButton::Left);
        let eff_warmth = if mouse_down { mx / sw } else { warmth };
        let eff_energy = if mouse_down { energy.max(0.55) } else { energy };

        if tick.is_multiple_of(60) {
            let _ = mood_tx.send((eff_warmth, eff_energy));
        }

        // pull LLM chars
        let mut llm_char: Option<char> = None;
        if let Ok(c) = llm_chars.try_recv() {
            llm_char = Some(c);
        }

        // spawn glyphs
        spawn_acc += dt * (3.0 + eff_energy * 11.0);
        while spawn_acc >= 1.0 {
            spawn_acc -= 1.0;
            let from_llm = llm_char.is_some();
            let ch = llm_char
                .take()
                .unwrap_or_else(|| local_char(eff_warmth, eff_energy, tick + glyphs.len() as u64));
            if ch.is_whitespace() {
                continue;
            }
            let (x, y, vx, vy) = if mouse_down {
                (
                    mx + (rand_fast(tick) - 0.5) * 60.,
                    my + (rand_fast(tick.wrapping_add(7)) - 0.5) * 60.,
                    (rand_fast(tick.wrapping_add(3)) - 0.5) * 30.,
                    -40. - eff_energy * 120.,
                )
            } else {
                let speed = 55. + eff_energy * 130.;
                (
                    rand_fast(tick.wrapping_add(11)) * sw,
                    sh + 20.,
                    (rand_fast(tick.wrapping_add(5)) - 0.5) * (10. + eff_energy * 40.),
                    -speed,
                )
            };
            // life matches actual screen-crossing time so glyphs stay visible
            let speed = vy.abs();
            let max_life = (sh + 40.) / speed + 1.5;
            glyphs.push(Glyph {
                ch,
                x,
                y,
                vx,
                vy,
                life: max_life,
                max_life,
                size: (if from_llm { 34. } else { 28. }) + eff_energy * 16.,
            });
            if glyphs.len() > 260 {
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
                    x: px,
                    y: py,
                    vx: ang.cos() * sp,
                    vy: ang.sin() * sp - 20.,
                    life: 1.5 + rand_fast(tick.wrapping_add(99)) * 2.,
                    max_life: 3.5,
                    r: 1.5 + rand_fast(tick.wrapping_add(77)) * 3.,
                });
            }
        }
        // ambient drifting particles
        if particles.len() < 90 && tick.is_multiple_of(8) {
            particles.push(Particle {
                x: rand_fast(tick.wrapping_add(41)) * sw,
                y: sh + 4.,
                vx: (rand_fast(tick.wrapping_add(43)) - 0.5) * 12.,
                vy: -8. - eff_energy * 20.,
                life: 4.,
                max_life: 6.,
                r: 1. + rand_fast(tick.wrapping_add(47)) * 2.,
            });
        }
        if particles.len() > 260 {
            particles.drain(0..particles.len() - 260);
        }

        // hue: cold blue 210deg .. warm amber 30deg, plus a slow autonomous drift
        // so the ambient palette breathes even when no touch is shaping it
        let hue_drift = (t * 0.025).sin() * 0.18; // ~250s full cool↔warm sweep
        let hue = (0.58 - eff_warmth * 0.5 + hue_drift).rem_euclid(1.0);

        // draw
        clear_background(Color::new(0.012, 0.012, 0.02, 1.));
        // soft nebula: two slow-drifting radial washes in complementary hues add
        // atmospheric depth to the void without ever competing with the glyphs.
        // Each is faked as 3 concentric circles with decreasing alpha, drifting on
        // its own Lissajous at a ~150-250s period, breathing the same warm/cool
        // axis as the foreground so colour and atmosphere stay in lock-step.
        let neb_a_alpha = 0.07 + 0.04 * (t * 0.05).sin();
        let neb_b_alpha = 0.05 + 0.035 * (t * 0.04 + 1.7).cos();
        let na_x = sw * (0.5 + 0.28 * (t * 0.018).sin());
        let na_y = sh * (0.5 + 0.20 * (t * 0.013).cos());
        for i in 0..3i32 {
            let mut c = hsl_to_rgb((hue + 0.5).rem_euclid(1.0), 0.55, 0.5);
            c.a = neb_a_alpha * (1.0 - i as f32 * 0.4);
            draw_circle(na_x, na_y, sw * (0.32 + 0.18 * i as f32), c);
        }
        let nb_x = sw * (0.5 + 0.28 * (t * 0.017).cos());
        let nb_y = sh * (0.5 + 0.20 * (t * 0.022).sin());
        for i in 0..3i32 {
            let mut c = hsl_to_rgb(hue, 0.6, 0.45);
            c.a = neb_b_alpha * (1.0 - i as f32 * 0.4);
            draw_circle(nb_x, nb_y, sw * (0.28 + 0.15 * i as f32), c);
        }
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
            // gentle horizontal breath so the stream feels like a slow wind, not a straight rain
            g.x += (t * 0.55 + g.y * 0.012).sin() * 6.0 * dt;
            g.life -= dt;
            let a = (g.life / g.max_life).clamp(0., 1.);
            // ease: hold bright, fade only near end of life
            let aeased = a * a * (3. - 2. * a);
            // soft birth fade-in: glyphs materialize over the first ~7% of life
            // so they ease into the stream instead of popping at full alpha.
            let elapsed = 1.0 - a;
            let birth = (elapsed / 0.07).clamp(0., 1.);
            let birth_eased = birth * birth * (3. - 2. * birth);
            let mut c = hsl_to_rgb(hue, 0.45, 0.85);
            c.a = birth_eased * (0.30 + aeased * 0.65);
            let params = TextParams {
                font: font.as_ref(),
                font_size: g.size as u16,
                color: c,
                ..Default::default()
            };
            draw_text_ex(g.ch.to_string(), g.x, g.y, params);
        }
        glyphs.retain(|g| g.life > 0. && g.y > -40.);

        // soft vignette
        draw_rectangle(0., 0., sw, 3., Color::new(0., 0., 0., 0.25));

        if last_tel.elapsed().as_secs() >= 10 {
            last_tel = Instant::now();
            let h = llm_health.lock().unwrap();
            append_jsonl(
                &tel_path,
                &Telemetry {
                    ts: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    fps: get_fps() as f32,
                    warmth: eff_warmth,
                    energy: eff_energy,
                    contacts: if mouse_down { 1 } else { contacts },
                    touch_device: if mouse_down {
                        "mouse".into()
                    } else {
                        dev.clone()
                    },
                    llm_ok: h.ok,
                    llm_toks_per_s: (h.toks_per_s * 100.).round() / 100.,
                    llm_model: h.model.clone(),
                    llm_last: h.last_text.clone(),
                    glyphs: glyphs.len(),
                    particles: particles.len(),
                },
            );
        }

        // in-app frame grab for AI vision feedback (xwd unreliable under GNOME/XWayland)
        if last_shot.elapsed().as_secs() >= 60 {
            last_shot = Instant::now();
            let sp = shot_path.clone();
            let data = get_screen_data(); // must be on main thread (GL context)
            let w = data.width as usize;
            let h = data.height as usize;
            let raw = data.bytes;
            std::thread::spawn(move || {
                let step = 2;
                let nw = w / step;
                let nh = h / step;
                let mut buf: Vec<u8> = Vec::with_capacity(nw * nh * 3);
                for y in 0..nh {
                    for x in 0..nw {
                        let i = ((y * step) * w + (x * step)) * 4;
                        if i + 2 < raw.len() {
                            buf.push(raw[i]);
                            buf.push(raw[i + 1]);
                            buf.push(raw[i + 2]);
                        }
                    }
                }
                let tmp = format!("{sp}.tmp.ppm");
                if let Ok(mut f) = std::fs::File::create(&tmp) {
                    use std::io::Write as _;
                    let _ = writeln!(f, "P6\n{nw} {nh}\n255");
                    let _ = f.write_all(&buf);
                }
                let _ = std::process::Command::new("ffmpeg")
                    .args(["-y", "-loglevel", "error", "-i", &tmp, &sp])
                    .status();
                let _ = std::fs::remove_file(&tmp);
            });
        }

        next_frame().await;
    }
}

fn rand_fast(seed: u64) -> f32 {
    let x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((x >> 33) as f32) / (1u64 << 31) as f32
}
